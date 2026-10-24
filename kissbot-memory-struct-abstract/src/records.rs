//! 原始记录读取与 Case 原文副本读写。
//!
//! 读取侧按共享记忆文件（`{root}/{agent}/memory-store/{year}-{记忆后缀}/…`）正序读取通道记录，
//! 再按占位内容引用的详情时间与 key 取出对应的思考/工具记录：详情先于占位写入，读取方按引用给出的
//! 时间定位到当天的记录文件、按 key 取出详情。
//! Case 原文副本按类型分文件（`{kind}-records.jsonl`，行内容与记忆存储的记录格式一致，不再按日期分文件）。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::{
    CaseCursor, CaseRecord, ChannelKey, ChannelRecord, Content, DetailRef, ThinkRecord,
    ToolCallRecord, ToolResultRecord, Watermark, RECORD_CHANNEL, RECORD_KINDS, RECORD_THINK,
    RECORD_TOOL_CALL, RECORD_TOOL_RESULT,
};
use kai_file::LineReader;
use serde_json::Value;
use tracing::warn;

use crate::error::Result;

/// 一条原始记录（类型由 body 判定；落盘为记忆存储的记录格式）
#[derive(Debug, Clone)]
pub struct RecordEntry {
    pub time: ArcStr,
    pub sn: u64,
    pub body: RecordBody,
}

#[derive(Debug, Clone)]
pub enum RecordBody {
    Channel(Arc<ChannelRecord>),
    Think(Arc<ThinkRecord>),
    ToolCall(Arc<ToolCallRecord>),
    ToolResult(Arc<ToolResultRecord>),
}

impl RecordEntry {
    /// 记录类型段（由 body 决定）
    pub fn kind(&self) -> &'static str {
        match &self.body {
            RecordBody::Channel(_) => RECORD_CHANNEL,
            RecordBody::Think(_) => RECORD_THINK,
            RecordBody::ToolCall(_) => RECORD_TOOL_CALL,
            RecordBody::ToolResult(_) => RECORD_TOOL_RESULT,
        }
    }

    /// 记忆存储中的记录 JSON（case 原文副本的落盘格式）
    pub fn to_value(&self) -> Result<Value> {
        Ok(match &self.body {
            RecordBody::Channel(r) => serde_json::to_value(r.as_ref())?,
            RecordBody::Think(r) => serde_json::to_value(r.as_ref())?,
            RecordBody::ToolCall(r) => serde_json::to_value(r.as_ref())?,
            RecordBody::ToolResult(r) => serde_json::to_value(r.as_ref())?,
        })
    }

    /// 记录自身的通道身份（仅通道记录有）
    pub fn channel_key(&self) -> Option<ChannelKey> {
        match &self.body {
            RecordBody::Channel(r) => Some(ChannelKey::from(r.as_ref())),
            _ => None,
        }
    }

    /// 记录 meta 里给出的摘要记忆 case 名（没有则返回 None）。
    ///
    /// 这是摘要记忆的使用方在写入记忆时填入的：有该信息时直接按 case 名定位，不再走分组/逐层判定。
    pub fn meta_case_name(&self) -> Option<&ArcStr> {
        match &self.body {
            RecordBody::Channel(r) => r.meta.as_ref().and_then(|meta| meta.case_name()),
            _ => None,
        }
    }

    /// 详情记录（思考/工具）的 key：占位引用按它找到详情（通道记录没有 key）
    pub fn key(&self) -> Option<&ArcStr> {
        match &self.body {
            RecordBody::Think(r) => Some(&r.key),
            RecordBody::ToolCall(r) => Some(&r.key),
            RecordBody::ToolResult(r) => Some(&r.key),
            RecordBody::Channel(_) => None,
        }
    }
}

/// 占位内容引用的详情记录类型与详情引用（Multi 内递归收集）
fn placeholder_refs(content: &Content) -> Vec<(&'static str, &DetailRef)> {
    match content {
        Content::Think(detail) => vec![(RECORD_THINK, detail.as_ref())],
        Content::ToolCall(detail) => vec![(RECORD_TOOL_CALL, detail.as_ref())],
        Content::ToolResult(detail) => vec![(RECORD_TOOL_RESULT, detail.as_ref())],
        Content::Multi(items) => items.iter().flat_map(|item| placeholder_refs(item)).collect(),
        _ => Vec::new(),
    }
}

/// 详情引用给出的记录日期（`YYYY-MM-DD`）；时间格式不完整时返回 None
fn detail_date(time: &str) -> Option<&str> {
    let bytes = time.as_bytes();
    (bytes.len() >= 10 && bytes[4] == b'-' && bytes[7] == b'-').then(|| &time[..10])
}

impl PartialEq for RecordEntry {
    fn eq(&self, other: &Self) -> bool {
        self.kind() == other.kind() && self.time == other.time && self.sn == other.sn
    }
}

impl Eq for RecordEntry {}

impl PartialOrd for RecordEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RecordEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.time.as_str(), self.sn).cmp(&(other.time.as_str(), other.sn))
    }
}

/// Case 原文副本文件：`{case_dir}/{kind}-records.jsonl`
pub fn case_record_path(case_dir: &Path, kind: &str) -> PathBuf {
    case_dir.join(format!("{kind}-records.jsonl"))
}

/// Case 目录：`{root}/{agent}/memory-struct-abstract/{角色名}/cases/{case_name}`
pub fn case_dir(role_dir: &Path, case_name: &str) -> PathBuf {
    role_dir.join("cases").join(case_name)
}

/// 枚举某记忆后缀在记忆存储中已存在的记录文件（按日期升序，只取 `date >= from_date`）。
///
/// 直接枚举 `memory-store` 下的年目录，避免按日期逐个探测不存在的文件；
/// 不设上界：记录时间可能晚于本机当前日期，上界过滤会漏读。
pub async fn list_store_files(
    root: &Path,
    agent_id: &str,
    suffix: &str,
    kind: &str,
    from_date: &str,
) -> Result<Vec<(String, PathBuf)>> {
    let store_root = root.join(agent_id).join("memory-store");
    let mut year_dirs = match tokio::fs::read_dir(&store_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let prefix = format!("{kind}-records-");
    let mut files = Vec::new();
    while let Some(entry) = year_dirs.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        // 年目录形如 "2026-dev"：前 4 位数字 + '-' + 记忆后缀
        if dir_name.len() < 5 || !dir_name.as_bytes()[..4].iter().all(|b| b.is_ascii_digit()) || &dir_name[4..5] != "-" {
            continue;
        }
        if &dir_name[5..] != suffix {
            continue;
        }
        let mut dir_entries = tokio::fs::read_dir(entry.path()).await?;
        while let Some(file_entry) = dir_entries.next_entry().await? {
            let file_name = file_entry.file_name().to_string_lossy().to_string();
            let Some(date) = file_name.strip_prefix(&prefix).and_then(|n| n.strip_suffix(".jsonl")) else {
                continue;
            };
            if date < from_date {
                continue;
            }
            files.push((date.to_string(), file_entry.path()));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// 枚举某角色在记忆存储中出现过的记忆后缀（`{year}-{后缀}` 的后缀段）。
///
/// 服务启动恢复时用：磁盘上的原始记录与水位的关系未知，这些后缀全部按待处理处理。
pub async fn list_store_suffixes(root: &Path, agent_id: &str, role: &str) -> Result<BTreeSet<ArcStr>> {
    let store_root = root.join(agent_id).join("memory-store");
    let mut year_dirs = match tokio::fs::read_dir(&store_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e.into()),
    };
    let mut suffixes = BTreeSet::new();
    while let Some(entry) = year_dirs.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        // 年目录形如 "2026-dev-e1"：前 4 位数字 + '-' + 记忆后缀
        if dir_name.len() < 5 || !dir_name.as_bytes()[..4].iter().all(|b| b.is_ascii_digit()) || &dir_name[4..5] != "-" {
            continue;
        }
        let suffix = &dir_name[5..];
        // 目录名形如 `{角色名}` 或 `{角色名}-{事件标识}`：已知角色名做前缀匹配，不还原 RoleEvent
        if suffix == role || suffix.strip_prefix(role).is_some_and(|rest| rest.starts_with('-')) {
            suffixes.insert(ArcStr::from(suffix));
        }
    }
    Ok(suffixes)
}

/// 共享记忆文件的全量视图：通道消息 + 每条消息被其内容引用的思考/工具记录
#[derive(Debug, Clone, Default)]
pub struct StoreView {
    /// 全部通道消息（按通道记录 `(time, sn)` 升序）
    pub messages: Vec<StoreMessage>,
}

/// 一条通道消息：通道记录 + 内容中占位引用到的思考/工具记录
///
/// 思考/工具记录不是独立的对话消息：它们在原文里以占位引用的形式出现在所属消息的 `Multi` 内容中，
/// 分组与归档都以消息为单位（一整条 multi 消息合成一条），因此详情归属到引用它的那条消息。
#[derive(Debug, Clone)]
pub struct StoreMessage {
    /// 通道归属（含思考/工具的通道归属：按占位引用）
    pub channel: ChannelKey,
    /// 通道记录本身（`content` 中的思考/工具为占位引用）
    pub record: RecordEntry,
    /// 该消息内容引用到的思考/工具记录（按 `(time, sn)` 升序）
    pub details: Vec<RecordEntry>,
}

/// 水位之后的待归档消息：消息本身与详情各自按类型水位判定
#[derive(Debug, Clone)]
pub struct PendingMessage {
    /// 消息原文（含详情），用于分组与落位分析
    pub message: StoreMessage,
    /// 待归档记录：通道记录本身（过了通道水位时）+ 过了各自类型水位的详情记录
    pub pending: Vec<RecordEntry>,
}

impl StoreView {
    /// 记录中出现过的全部通道（按三元组升序）
    pub fn known_channels(&self) -> BTreeSet<Arc<ChannelKey>> {
        self.messages.iter().map(|message| Arc::new(message.channel.clone())).collect()
    }

    /// 某通道水位之后的待归档消息（按通道记录 `(time, sn)` 升序）
    ///
    /// 一条消息只要还有任一记录（通道记录或某条详情）没过水位就进批次：分组按消息进行，
    /// 待归档记录由 [`PendingMessage::pending`] 逐条给出，水位只按这些记录推进。
    pub fn pending_for(&self, channel: &ChannelKey, watermark: &Watermark) -> Vec<PendingMessage> {
        self.messages
            .iter()
            .filter(|message| message.channel == *channel)
            .filter_map(|message| {
                let mut pending = Vec::new();
                let point = watermark.point(message.record.kind());
                if (message.record.time.as_str(), message.record.sn) > (point.time.as_str(), point.sn) {
                    pending.push(message.record.clone());
                }
                for detail in &message.details {
                    let point = watermark.point(detail.kind());
                    if (detail.time.as_str(), detail.sn) > (point.time.as_str(), point.sn) {
                        pending.push(detail.clone());
                    }
                }
                (!pending.is_empty()).then(|| PendingMessage { message: message.clone(), pending })
            })
            .collect()
    }
}

/// 读取某记忆后缀的共享记忆文件视图（按通道记录 `(time, sn)` 升序）。
///
/// 先读全部通道记录并收集占位内容引用的详情（类型 + 详情时间 + key → 所属消息），再按详情时间定位到
/// 当天的思考/工具记录文件、按 key 取出详情并挂到引用它的消息上。详情先于占位写入，因此读到占位时
/// 详情必然已落盘；没有被任何占位引用的思考/工具记录不属于任何通道，不参与读取。
///
/// 不做水位过滤：按通道判定批次需要看到全部记录——消息可能已被归档到更早的水位之下而它的详情还没；
/// 水位过滤由 [`StoreView::pending_for`] 按通道完成。
pub async fn read_store_records(root: &Path, agent_id: &str, suffix: &str) -> Result<StoreView> {
    let from_date = kissbot_api::WATERMARK_ZERO_TIME[..10].to_string();
    let mut messages: Vec<StoreMessage> = Vec::new();
    // (详情类型段, 详情日期, key) → 引用它的消息下标
    let mut refs: HashMap<(&'static str, ArcStr, ArcStr), usize> = HashMap::new();

    // 1. 全部通道记录：身份即归属，内容中的占位引用登记到 refs
    let files = list_store_files(root, agent_id, suffix, RECORD_CHANNEL, &from_date).await?;
    for (_, path) in files {
        let mut reader = LineReader::new(&path, None, None).await?;
        while let Some(line) = reader.next_line().await? {
            let text = line.line.trim();
            if text.is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(text) {
                Ok(value) => value,
                Err(e) => {
                    // 写入侧可能正在追加（半行）或历史脏数据：跳过不影响后续记录
                    warn!("[abstract] skip unparsable record line in {}: {}", path.display(), e);
                    continue;
                }
            };
            let record: ChannelRecord = match serde_json::from_value(value) {
                Ok(record) => record,
                Err(e) => {
                    warn!("[abstract] skip malformed channel record in {}: {}", path.display(), e);
                    continue;
                }
            };
            let channel = ChannelKey::from(&record);
            for (kind, detail) in placeholder_refs(&record.content) {
                if let Some(date) = detail_date(detail.time.as_str()) {
                    refs.entry((kind, ArcStr::from(date), detail.key.clone()))
                        .or_insert(messages.len());
                }
            }
            let entry = RecordEntry {
                time: record.time.clone(),
                sn: record.sn,
                body: RecordBody::Channel(Arc::new(record)),
            };
            messages.push(StoreMessage { channel, record: entry, details: Vec::new() });
        }
    }

    // 2. 按 (详情类型段, 日期) 分组读取被引用的详情记录，挂到引用它的消息上
    let mut by_file: BTreeMap<(&'static str, ArcStr), HashMap<ArcStr, usize>> = BTreeMap::new();
    for ((kind, date, key), owner) in refs {
        by_file.entry((kind, date)).or_default().insert(key, owner);
    }
    for ((kind, date), keys) in by_file {
        let path = store_record_path(root, agent_id, suffix, kind, &date);
        if !path.exists() {
            warn!("[abstract] detail record file missing: {}", path.display());
            continue;
        }
        let mut reader = LineReader::new(&path, None, None).await?;
        while let Some(line) = reader.next_line().await? {
            let text = line.line.trim();
            if text.is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(text) {
                Ok(value) => value,
                Err(e) => {
                    warn!("[abstract] skip unparsable record line in {}: {}", path.display(), e);
                    continue;
                }
            };
            let Some(owner) = value.get("key").and_then(Value::as_str).and_then(|key| keys.get(key)).copied() else {
                continue;
            };
            let Some((time, sn)) = value_time_sn(&value) else {
                continue;
            };
            match parse_entry(kind, time, sn, &value) {
                Ok(entry) => messages[owner].details.push(entry),
                Err(e) => warn!("[abstract] skip malformed record in {}: {}", path.display(), e),
            }
        }
    }

    for message in &mut messages {
        message.details.sort();
    }
    messages.sort_by(|a, b| a.record.cmp(&b.record));
    Ok(StoreView { messages })
}

/// 记忆存储中某类型某日期的记录文件：`{root}/{agent}/memory-store/{year}-{后缀}/{stem}-records-{date}.jsonl`
fn store_record_path(root: &Path, agent_id: &str, suffix: &str, kind: &str, date: &str) -> PathBuf {
    let year = &date[..4];
    root.join(agent_id)
        .join("memory-store")
        .join(format!("{}-{}", year, suffix))
        .join(format!("{kind}-records-{date}.jsonl"))
}

fn parse_entry(kind: &str, time: ArcStr, sn: u64, value: &Value) -> Result<RecordEntry> {
    let body = match kind {
        RECORD_CHANNEL => RecordBody::Channel(Arc::new(serde_json::from_value::<ChannelRecord>(value.clone())?)),
        RECORD_THINK => RecordBody::Think(Arc::new(serde_json::from_value::<ThinkRecord>(value.clone())?)),
        RECORD_TOOL_CALL => RecordBody::ToolCall(Arc::new(serde_json::from_value::<ToolCallRecord>(value.clone())?)),
        RECORD_TOOL_RESULT => RecordBody::ToolResult(Arc::new(serde_json::from_value::<ToolResultRecord>(value.clone())?)),
        other => unreachable!("未知的记录类型段: {other}"),
    };
    Ok(RecordEntry { time, sn, body })
}

/// 从记录 JSON 取 (time, sn)
pub fn value_time_sn(value: &Value) -> Option<(ArcStr, u64)> {
    let time = ArcStr::from(value.get("time")?.as_str()?);
    let sn = value.get("sn")?.as_u64()?;
    Some((time, sn))
}

/// 把批次记录追加为 case 的原文副本（按记录类型分文件，保持记忆存储的记录格式）。
///
/// 同类型记录合并为一次写入，并等待写入真正完成：`tokio::fs::File` 的 `write_all` 可能只把数据
/// 交给后台阻塞任务就返回，随后同一进程内的读取（摘要重算、case 查询）会看到不完整的内容。
pub async fn append_case_records(dir: &Path, entries: &[RecordEntry]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    tokio::fs::create_dir_all(dir).await?;
    use tokio::io::AsyncWriteExt;
    let mut buffers: BTreeMap<&'static str, String> = BTreeMap::new();
    for entry in entries {
        let line = serde_json::to_string(&entry.to_value()?)?;
        let buffer = buffers.entry(entry.kind()).or_default();
        buffer.push_str(&line);
        buffer.push('\n');
    }
    for (kind, buffer) in buffers {
        let path = case_record_path(dir, kind);
        let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await?;
        file.write_all(buffer.as_bytes()).await?;
        file.flush().await?;
    }
    Ok(())
}

/// 取游标之后的原文副本：同类型比较 `(time, sn)`；同一时刻的其他类型记录视为在其后
/// （各类型文件的 `sn` 相互独立、跨类型不可比——与归档批次末尾的同一条规则）。
/// 调用方把上一条记录的 `time` / `sn` / `record_type` 原样回传即可连续分页。
pub fn records_after(records: &[CaseRecord], after: &CaseCursor) -> Vec<CaseRecord> {
    records
        .iter()
        .filter(|record| match record.time().cmp(after.time.as_str()) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => record.kind() != after.record_type.as_str() || record.sn() > after.sn,
        })
        .cloned()
        .collect()
}

/// 取结尾若干条原文副本（保持升序）
pub fn records_tail(records: &[CaseRecord], count: u64) -> Vec<CaseRecord> {
    let count = (count as usize).min(records.len());
    records[records.len() - count..].to_vec()
}

/// 读取 case 的全部原文副本（按 (time, sn) 升序）
pub async fn read_case_records(dir: &Path) -> Result<Vec<CaseRecord>> {
    let mut records = Vec::new();
    for kind in RECORD_KINDS {
        let path = case_record_path(dir, kind);
        if !path.exists() {
            continue;
        }
        let mut reader = LineReader::new(&path, None, None).await?;
        while let Some(line) = reader.next_line().await? {
            let text = line.line.trim();
            if text.is_empty() {
                continue;
            }
            match parse_case_record(kind, text) {
                Ok(record) => records.push(record),
                Err(e) => warn!("[abstract] skip unparsable case record in {}: {}", path.display(), e),
            }
        }
    }
    records.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    Ok(records)
}

fn parse_case_record(kind: &str, text: &str) -> Result<CaseRecord> {
    Ok(match kind {
        RECORD_CHANNEL => CaseRecord::Channel(Arc::new(serde_json::from_str::<ChannelRecord>(text)?)),
        RECORD_THINK => CaseRecord::Think(Arc::new(serde_json::from_str::<ThinkRecord>(text)?)),
        RECORD_TOOL_CALL => CaseRecord::ToolCall(Arc::new(serde_json::from_str::<ToolCallRecord>(text)?)),
        RECORD_TOOL_RESULT => CaseRecord::ToolResult(Arc::new(serde_json::from_str::<ToolResultRecord>(text)?)),
        other => unreachable!("未知的记录类型段: {other}"),
    })
}

/// 删除 case 目录下全部原文副本（重建时使用）
pub async fn clear_case_records(dir: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

// ========== 渲染（送模型的文本形态） ==========

/// 渲染一条读取阶段的记录
pub fn render_entry(entry: &RecordEntry) -> String {
    match &entry.body {
        RecordBody::Channel(r) => format!("[{}] {}: {}", entry.time, r.user_name, render_content(&r.content)),
        RecordBody::Think(r) => format!("[{}] (思考) {}", entry.time, think_text(r)),
        RecordBody::ToolCall(r) => format!("[{}] (工具调用) {} {}", entry.time, r.tool_name, r.tool_params),
        RecordBody::ToolResult(r) => {
            let payload = serde_json::to_string(&r.tool_result).unwrap_or_else(|_| String::new());
            format!("[{}] (工具结果) {}", entry.time, payload)
        }
    }
}

/// 渲染一条 case 原文副本（类型来自所在文件）
pub fn render_case_record(record: &CaseRecord) -> String {
    match record {
        CaseRecord::Channel(r) => format!("[{}] {}: {}", r.time, r.user_name, render_content(&r.content)),
        CaseRecord::Think(r) => format!("[{}] (思考) {}", r.time, think_text(r)),
        CaseRecord::ToolCall(r) => format!("[{}] (工具调用) {} {}", r.time, r.tool_name, r.tool_params),
        CaseRecord::ToolResult(r) => {
            let payload = serde_json::to_string(&r.tool_result).unwrap_or_else(|_| String::new());
            format!("[{}] (工具结果) {}", r.time, payload)
        }
    }
}

fn think_text(record: &ThinkRecord) -> &str {
    record
        .reasoning_content
        .as_ref()
        .map(|s| s.as_str())
        .or_else(|| record.thinking.as_ref().map(|s| s.as_str()))
        .unwrap_or("")
}

/// 一条消息的原文文本：内容里的思考/工具占位按引用取出详情、用标签内嵌，整条消息合成一个 String。
///
/// 标签约定：`reasoning_content` → `<reasoning_content>…</reasoning_content>`、
/// `thinking` → `<think>…</think>`、工具调用 → `<tool_call>…</tool_call>`、
/// 工具结果 → `<tool_result>…</tool_result>`。引用不到详情（详情文件缺失）时该占位渲染为空。
pub fn synthesize_content(content: &Content, details: &[RecordEntry]) -> String {
    match content {
        Content::Multi(items) => items
            .iter()
            .map(|item| synthesize_content(item, details))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Content::Think(detail) => detail_text(details, RECORD_THINK, detail.key.as_str()),
        Content::ToolCall(detail) => detail_text(details, RECORD_TOOL_CALL, detail.key.as_str()),
        Content::ToolResult(detail) => detail_text(details, RECORD_TOOL_RESULT, detail.key.as_str()),
        other => render_content(other),
    }
}

/// 占位引用对应的详情文本（按类型 + key 找详情记录）
fn detail_text(details: &[RecordEntry], kind: &str, key: &str) -> String {
    let Some(entry) = details.iter().find(|entry| entry.kind() == kind && entry.key().is_some_and(|k| k.as_str() == key)) else {
        return String::new();
    };
    match &entry.body {
        RecordBody::Think(r) => {
            let mut parts = Vec::new();
            if let Some(reasoning) = r.reasoning_content.as_ref().filter(|text| !text.is_empty()) {
                parts.push(format!("<reasoning_content>{reasoning}</reasoning_content>"));
            }
            if let Some(thinking) = r.thinking.as_ref().filter(|text| !text.is_empty()) {
                parts.push(format!("<think>{thinking}</think>"));
            }
            parts.join("\n")
        }
        RecordBody::ToolCall(r) => format!("<tool_call>{} {}</tool_call>", r.tool_name, r.tool_params),
        RecordBody::ToolResult(r) => {
            let payload = serde_json::to_string(&r.tool_result).unwrap_or_else(|_| String::new());
            format!("<tool_result>{}</tool_result>", payload)
        }
        RecordBody::Channel(_) => String::new(),
    }
}

/// 渲染公共消息内容（思考/工具占位无法内嵌时退化为 JSON，供摘要等不解析详情的场景使用）
pub fn render_content(content: &Content) -> String {
    match content {
        Content::Text(text) => text.as_str().to_string(),
        Content::Multi(items) => items.iter().map(render_content).collect::<Vec<_>>().join("\n"),
        Content::AttachmentInfo(info) => format!("[附件 {}]", info.file_name),
        Content::AttachmentInfoResponse(resp) => format!("[附件 {} key={}]", resp.info.file_name, resp.key),
        other => serde_json::to_string(other).unwrap_or_else(|_| String::from("[未知内容]")),
    }
}

/// 当前时间字符串（`YYYY-MM-DD HH:MM:SS`）
pub fn now_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_api::{ContentBlock, RECORD_CHANNEL_ARC};

    fn channel_entry(time: &str, sn: u64, is_self: usize) -> RecordEntry {
        RecordEntry {
            time: ArcStr::from(time),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
                meta: None,
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self"),
                messenger_id: ArcStr::from("web"),
                group_id: ArcStr::from("g1"),
                is_self,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::from("alice"),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("你好")),
                time: ArcStr::from(time),
                sn,
            })),
        }
    }

    fn think_entry(time: &str, sn: u64) -> RecordEntry {
        RecordEntry {
            time: ArcStr::from(time),
            sn,
            body: RecordBody::Think(Arc::new(ThinkRecord {
                reasoning_content: Some(ArcStr::from("推理")),
                thinking: None,
                key: ArcStr::from("k1"),
                time: ArcStr::from(time),
                sn,
            })),
        }
    }

    #[test]
    fn placeholder_refs_walks_multi_and_reports_detail_kind() {
        let think = Content::Think(Arc::new(DetailRef::new(ArcStr::from("k1"), ArcStr::from("2026-09-22 10:00:00"))));
        let tool_call = Content::ToolCall(Arc::new(DetailRef::new(ArcStr::from("k2"), ArcStr::from("2026-09-22 10:00:01"))));
        let tool_result = Content::ToolResult(Arc::new(DetailRef::new(ArcStr::from("k2"), ArcStr::from("2026-09-22 10:00:02"))));
        let content = Content::Multi(Arc::new(vec![think, tool_call, tool_result]));
        let refs = placeholder_refs(&content);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].0, RECORD_THINK);
        assert_eq!(refs[1].0, RECORD_TOOL_CALL);
        assert_eq!(refs[2].0, RECORD_TOOL_RESULT);
        assert_eq!(refs[1].1.key.as_str(), "k2");
        assert_eq!(detail_date(refs[2].1.time.as_str()), Some("2026-09-22"));
        assert!(placeholder_refs(&Content::Text(ArcStr::from("普通消息"))).is_empty());
    }

    /// 往共享记忆文件追加一行记录（模拟 memory-store 的落盘）
    async fn append_store_line(root: &Path, agent_id: &str, suffix: &str, stem: &str, record: Value, time: &str) {
        let date = &time[..10];
        let year = &time[..4];
        let dir = root.join(agent_id).join("memory-store").join(format!("{}-{}", year, suffix));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{}-records-{}.jsonl", stem, date)))
            .await
            .unwrap();
        file.write_all(serde_json::to_string(&record).unwrap().as_bytes()).await.unwrap();
        file.write_all(b"\n").await.unwrap();
        file.flush().await.unwrap();
    }

    fn channel_value(channel: &ChannelKey, time: &str, sn: u64, content: Value) -> Value {
        serde_json::json!({
            "msg_id": "",
            "user_id": channel.self_user_id,
            "self_user_id": channel.self_user_id,
            "messenger_id": channel.messenger_id,
            "group_id": channel.group_id,
            "is_self": 1,
            "messenger_name": "",
            "user_name": "",
            "group_name": "",
            "content": content,
            "time": time,
            "sn": sn,
        })
    }

    fn detail_ref_value(kind: &str, key: &str, time: &str) -> Value {
        serde_json::json!({"msg_type": kind, "data": {"key": key, "time": time}})
    }

    #[tokio::test]
    async fn read_store_records_resolves_details_by_placeholder_time_and_key() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let channel = ChannelKey::new(ArcStr::from("web"), ArcStr::from("selfA"), ArcStr::from("gA"));
        // 占位记录自身的日期与详情日期可以不同：详情按其时间落在 09-22，占位记录写在 09-23 凌晨
        append_store_line(root, "0", "dev", "channel", channel_value(&channel, "2026-09-22 23:00:00", 1, serde_json::to_value(Content::Text(ArcStr::from("问"))).unwrap()), "2026-09-22 23:00:00").await;
        let multi = serde_json::json!({"msg_type": "Multi", "data": [
            detail_ref_value("Think", "k1", "2026-09-22 23:59:59"),
            detail_ref_value("ToolCall", "k4", "2026-09-22 10:00:00"),
            detail_ref_value("ToolResult", "k4", "2026-09-22 10:00:05"),
        ]});
        append_store_line(root, "0", "dev", "channel", channel_value(&channel, "2026-09-22 23:00:01", 2, multi), "2026-09-22 23:00:01").await;
        let cross_day = channel_value(&channel, "2026-09-23 00:00:02", 1, detail_ref_value("Think", "k2", "2026-09-22 23:59:59"));
        append_store_line(root, "0", "dev", "channel", cross_day, "2026-09-23 00:00:02").await;

        // 09-22 的详情文件：k1/k2 被引用，k3 是孤儿；k4 的调用与结果分属两个文件
        append_store_line(root, "0", "dev", "think", serde_json::json!({"reasoning_content": "思考1", "thinking": null, "key": "k1", "time": "2026-09-22 23:59:59", "sn": 1}), "2026-09-22 23:59:59").await;
        append_store_line(root, "0", "dev", "think", serde_json::json!({"reasoning_content": "思考2", "thinking": null, "key": "k2", "time": "2026-09-22 23:59:59", "sn": 2}), "2026-09-22 23:59:59").await;
        append_store_line(root, "0", "dev", "think", serde_json::json!({"reasoning_content": "孤儿", "thinking": null, "key": "k3", "time": "2026-09-22 23:59:58", "sn": 3}), "2026-09-22 23:59:58").await;
        append_store_line(root, "0", "dev", "tool-call", serde_json::json!({"tool_name": "t", "tool_params": {}, "key": "k4", "time": "2026-09-22 10:00:00", "sn": 1}), "2026-09-22 10:00:00").await;
        append_store_line(root, "0", "dev", "tool-result", serde_json::json!({"tool_result": null, "key": "k4", "time": "2026-09-22 10:00:05", "sn": 1}), "2026-09-22 10:00:05").await;

        let view = read_store_records(root, "0", "dev").await.unwrap();
        assert_eq!(view.messages.len(), 3, "三条通道消息（思考/工具不单独成条）");
        assert!(view.messages.iter().all(|message| message.channel == channel));
        assert_eq!(view.known_channels().len(), 1);
        let detail_keys = |message: &StoreMessage| -> Vec<String> {
            message.details.iter().filter_map(|entry| entry.key().map(|key| key.as_str().to_string())).collect()
        };
        assert!(view.messages[0].details.is_empty(), "第一条消息没有详情");
        assert_eq!(detail_keys(&view.messages[1]), vec!["k4", "k4", "k1"], "按 (time, sn) 挂在该消息上（含跨日期）");
        assert_eq!(detail_keys(&view.messages[2]), vec!["k2"], "详情日期与消息日期可以不同");
        // 原文合成：占位按引用取出详情并用标签内嵌
        let text = synthesize_content(
            match &view.messages[1].record.body {
                RecordBody::Channel(record) => &record.content,
                _ => unreachable!(),
            },
            &view.messages[1].details,
        );
        assert!(text.contains("<tool_call>t {}</tool_call>"), "工具调用内嵌: {}", text);
        assert!(text.contains("<tool_result>null</tool_result>"), "工具结果内嵌: {}", text);
        assert!(text.contains("<reasoning_content>思考1</reasoning_content>"), "思考内容内嵌: {}", text);
        assert!(!text.contains("k1"), "不再露出占位引用的 JSON");
    }

    /// 构造一条消息（通道记录 + 详情），时间取通道记录
    fn store_message(time: &str, sn: u64, channel: &ChannelKey, details: Vec<RecordEntry>) -> StoreMessage {
        let mut record = channel_entry(time, sn, 0);
        let RecordBody::Channel(body) = &mut record.body else { unreachable!() };
        let body = Arc::make_mut(body);
        body.messenger_id = channel.messenger_id.clone();
        body.self_user_id = channel.self_user_id.clone();
        body.group_id = channel.group_id.clone();
        StoreMessage { channel: channel.clone(), record, details }
    }

    #[test]
    fn store_view_pending_filters_by_channel_and_watermark() {
        let web_g1 = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"));
        let web_g2 = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self2"), ArcStr::from("g2"));
        let view = StoreView {
            messages: vec![
                store_message("2026-09-22 10:00:00", 1, &web_g1, vec![think_entry("2026-09-22 10:00:01", 2)]),
                store_message("2026-09-22 10:05:00", 3, &web_g2, Vec::new()),
            ],
        };
        assert_eq!(
            view.known_channels().iter().map(|channel| channel.as_ref().clone()).collect::<Vec<_>>(),
            vec![web_g1.clone(), web_g2.clone()]
        );

        // 无水位：该通道的全部消息，每条消息带上自己的待归档记录
        let all = view.pending_for(&web_g1, &Watermark::default());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].pending.iter().map(|entry| entry.sn).collect::<Vec<_>>(), vec![1, 2]);

        // 通道水位推进过通道记录、思考水位未动：消息仍在，只剩思考记录待归档
        let mut partial = Watermark::default();
        partial.channel.time = ArcStr::from("2026-09-22 10:00:00");
        partial.channel.sn = 1;
        let pending = view.pending_for(&web_g1, &partial);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].pending.len(), 1);
        assert_eq!(pending[0].pending[0].kind(), RECORD_THINK);

        // 两类水位都推进到末尾：该通道清空；另一通道不受影响
        let mut watermark = Watermark::default();
        watermark.channel.time = ArcStr::from("2026-09-22 10:00:00");
        watermark.channel.sn = 1;
        watermark.think.time = ArcStr::from("2026-09-22 10:00:01");
        watermark.think.sn = 2;
        assert!(view.pending_for(&web_g1, &watermark).is_empty());
        assert_eq!(view.pending_for(&web_g2, &Watermark::default()).len(), 1);
    }

    #[test]
    fn synthesize_content_inlines_think_and_tool_placeholders() {
        let think = Content::Think(Arc::new(DetailRef::new(ArcStr::from("k1"), ArcStr::from("2026-09-22 10:00:00"))));
        let call = Content::ToolCall(Arc::new(DetailRef::new(ArcStr::from("k2"), ArcStr::from("2026-09-22 10:00:01"))));
        let result = Content::ToolResult(Arc::new(DetailRef::new(ArcStr::from("k2"), ArcStr::from("2026-09-22 10:00:02"))));
        let content = Content::Multi(Arc::new(vec![think, call, result, Content::Text(ArcStr::from("回复"))]));

        let thinking = RecordEntry {
            time: ArcStr::from("2026-09-22 10:00:00"),
            sn: 1,
            body: RecordBody::Think(Arc::new(ThinkRecord {
                reasoning_content: None,
                thinking: Some(ArcStr::from("想法")),
                key: ArcStr::from("k1"),
                time: ArcStr::from("2026-09-22 10:00:00"),
                sn: 1,
            })),
        };
        let call_entry = RecordEntry {
            time: ArcStr::from("2026-09-22 10:00:01"),
            sn: 2,
            body: RecordBody::ToolCall(Arc::new(ToolCallRecord {
                tool_name: ArcStr::from("t"),
                tool_params: Arc::new(serde_json::json!({"x": 1})),
                key: ArcStr::from("k2"),
                time: ArcStr::from("2026-09-22 10:00:01"),
                sn: 2,
            })),
        };
        let result_entry = RecordEntry {
            time: ArcStr::from("2026-09-22 10:00:02"),
            sn: 3,
            body: RecordBody::ToolResult(Arc::new(ToolResultRecord {
                tool_result: None,
                key: ArcStr::from("k2"),
                time: ArcStr::from("2026-09-22 10:00:02"),
                sn: 3,
            })),
        };
        let text = synthesize_content(&content, &[thinking, call_entry, result_entry]);
        assert_eq!(
            text,
            "<think>想法</think>\n<tool_call>t {\"x\":1}</tool_call>\n<tool_result>null</tool_result>\n回复"
        );

        // 引用不到详情：占位渲染为空（不露 JSON），多层 Multi 递归合成
        assert_eq!(synthesize_content(&content, &[]), "回复");
        assert_eq!(synthesize_content(&Content::Text(ArcStr::from("你好")), &[]), "你好");
    }

    #[test]
    fn render_entry_by_kind() {
        assert_eq!(render_entry(&channel_entry("2026-09-22 13:48:37", 1, 0)), "[2026-09-22 13:48:37] alice: 你好");
        assert_eq!(render_entry(&think_entry("2026-09-22 13:48:38", 1)), "[2026-09-22 13:48:38] (思考) 推理");

        let result = RecordEntry {
            time: ArcStr::from("2026-09-22 13:48:40"),
            sn: 1,
            body: RecordBody::ToolResult(Arc::new(ToolResultRecord {
                tool_result: Some(Arc::new(vec![ContentBlock::text(ArcStr::from("v1"))])),
                key: ArcStr::from("k1"),
                time: ArcStr::from("2026-09-22 13:48:40"),
                sn: 1,
            })),
        };
        assert!(render_entry(&result).contains("(工具结果)"));
    }

    #[test]
    fn case_record_path_is_per_kind_without_date() {
        let dir = Path::new("/root/0/memory-struct-abstract/dev/cases/e12");
        assert_eq!(case_record_path(dir, RECORD_CHANNEL), dir.join("channel-records.jsonl"));
        assert_eq!(case_record_path(dir, RECORD_TOOL_CALL), dir.join("tool-call-records.jsonl"));
    }

    fn case_records_fixture() -> Vec<CaseRecord> {
        vec![
            CaseRecord::Channel(Arc::new(ChannelRecord {
                meta: None,
                user_id: ArcStr::from("u1"), self_user_id: ArcStr::from("self"),
                messenger_id: ArcStr::from("web"), group_id: ArcStr::from("g1"),
                is_self: 0, msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::from("alice"), group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("问")),
                time: ArcStr::from("2026-09-22 10:00:00"), sn: 1,
            })),
            CaseRecord::Think(Arc::new(ThinkRecord {
                reasoning_content: Some(ArcStr::from("推理")), thinking: None,
                key: ArcStr::from("k1"),
                time: ArcStr::from("2026-09-22 10:00:00"), sn: 1,
            })),
            CaseRecord::Channel(Arc::new(ChannelRecord {
                meta: None,
                user_id: ArcStr::from("self"), self_user_id: ArcStr::from("self"),
                messenger_id: ArcStr::from("web"), group_id: ArcStr::from("g1"),
                is_self: 1, msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::from("bot"), group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("答")),
                time: ArcStr::from("2026-09-22 10:01:00"), sn: 2,
            })),
        ]
    }

    #[test]
    fn records_after_uses_cursor_rule() {
        let records = case_records_fixture();

        // 游标为第一条（通道记录 10:00:00 sn1）：同类型的 sn1 不算在其后，同一时刻的思考记录算在其后
        let after = CaseCursor::of(&records[0]);
        let rest = records_after(&records, &after);
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].kind(), RECORD_THINK, "同一时刻的其他类型记录视为在其后");
        assert_eq!(rest[1].kind(), RECORD_CHANNEL);

        // 游标为思考记录（10:00:00 sn1）：通道记录 sn1 视为在其后（跨类型不比 sn）
        let after = CaseCursor::of(&records[1]);
        let rest = records_after(&records, &after);
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].kind(), RECORD_CHANNEL);
        assert_eq!(rest[0].sn(), 1);

        // 游标为最后一条：之后为空
        assert!(records_after(&records, &CaseCursor::of(&records[2])).is_empty());
        // 游标晚于全部记录：之后为空
        let cursor = CaseCursor { time: ArcStr::from("2099-01-01 00:00:00"), sn: 1, record_type: RECORD_CHANNEL_ARC.clone() };
        assert!(records_after(&records, &cursor).is_empty());
    }

    #[test]
    fn records_tail_keeps_last_ascending() {
        let records = case_records_fixture();
        assert!(records_tail(&records, 0).is_empty());
        let last = records_tail(&records, 1);
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].time(), "2026-09-22 10:01:00");
        assert_eq!(records_tail(&records, 10).len(), 3, "超过总数时返回全部");
        assert_eq!(records_tail(&records, 2)[0].time(), "2026-09-22 10:00:00", "升序返回");
    }

    #[tokio::test]
    async fn append_and_read_case_records_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cases/e1");
        let entries = vec![
            channel_entry("2026-09-22 10:00:01", 2, 1),
            think_entry("2026-09-22 10:00:00", 1),
            channel_entry("2026-09-22 10:00:02", 3, 0),
        ];
        append_case_records(&dir, &entries).await.unwrap();

        // 每类一个文件，行内容与记忆存储一致（不含类型标签）
        let channel_raw = tokio::fs::read_to_string(dir.join("channel-records.jsonl")).await.unwrap();
        assert_eq!(channel_raw.lines().count(), 2);
        assert!(!channel_raw.contains("record_type"));
        let think_raw = tokio::fs::read_to_string(dir.join("think-records.jsonl")).await.unwrap();
        assert_eq!(think_raw.lines().count(), 1);
        assert!(!dir.join("tool-call-records.jsonl").exists());

        // 读回按 (time, sn) 升序，类型来自文件
        let records = read_case_records(&dir).await.unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].kind(), RECORD_THINK);
        assert_eq!(records[1].kind(), RECORD_CHANNEL);
        assert_eq!(records[1].time(), "2026-09-22 10:00:01");
        assert_eq!(records[2].sn(), 3);
        assert!(render_case_record(&records[0]).contains("(思考) 推理"));
        let value = serde_json::to_value(&records[1]).unwrap();
        assert_eq!(value["record_type"], "channel");
    }

    #[tokio::test]
    async fn append_case_records_accumulates_across_batches() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cases/e1");
        append_case_records(&dir, &[channel_entry("2026-09-22 10:00:00", 1, 1)]).await.unwrap();
        append_case_records(&dir, &[channel_entry("2026-09-22 10:00:01", 2, 0)]).await.unwrap();
        let records = read_case_records(&dir).await.unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn entries_sort_by_time_then_sn() {
        let mut entries = vec![
            channel_entry("2026-09-22 10:00:01", 1, 0),
            channel_entry("2026-09-22 10:00:00", 2, 0),
            channel_entry("2026-09-22 10:00:00", 1, 0),
        ];
        entries.sort();
        assert_eq!(entries[0].sn, 1);
        assert_eq!(entries[1].sn, 2);
        assert_eq!(entries[2].time.as_str(), "2026-09-22 10:00:01");
    }

    #[test]
    fn to_value_keeps_memory_store_record_shape() {
        let entry = channel_entry("2026-09-22 13:48:37", 7, 1);
        let value = entry.to_value().unwrap();
        assert_eq!(value["is_self"], 1);
        assert_eq!(value["sn"], 7);
        assert_eq!(value["content"]["msg_type"], "Text");
        assert_eq!(value_time_sn(&value), Some((ArcStr::from("2026-09-22 13:48:37"), 7)));
    }

    #[test]
    fn case_dir_layout() {
        let dir = case_dir(Path::new("/root/0/memory-struct-abstract/dev"), "event-e12");
        assert_eq!(dir, PathBuf::from("/root/0/memory-struct-abstract/dev/cases/event-e12"));
    }

    #[tokio::test]
    async fn list_store_files_matches_suffix_and_date_range() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // 记忆存储布局：{root}/{agent}/memory-store/{year}-{记忆后缀}/{stem}-records-{date}.jsonl
        let dir = root.join("0").join("memory-store").join("2026-dev");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        for name in ["channel-records-2026-09-21.jsonl", "channel-records-2026-09-22.jsonl", "think-records-2026-09-22.jsonl"] {
            tokio::fs::write(dir.join(name), b"").await.unwrap();
        }
        // 后缀更长的目录不应误匹配（"dev-x" 不是 "dev"）
        let other = root.join("0").join("memory-store").join("2026-dev-x");
        tokio::fs::create_dir_all(&other).await.unwrap();
        tokio::fs::write(other.join("channel-records-2026-09-22.jsonl"), b"").await.unwrap();

        let files = list_store_files(root, "0", "dev", RECORD_CHANNEL, "2026-09-22").await.unwrap();
        assert_eq!(files.len(), 1, "只取不早于起始日期且匹配类型/后缀的文件");
        assert_eq!(files[0].0, "2026-09-22");
        assert!(files[0].1.ends_with("2026-dev/channel-records-2026-09-22.jsonl"));

        let files = list_store_files(root, "0", "dev", RECORD_CHANNEL, "2026-09-01").await.unwrap();
        assert_eq!(files.len(), 2, "起始日期之后的所有日期文件");
        assert_eq!(files[0].0, "2026-09-21", "按日期升序");

        let empty = list_store_files(root, "none", "dev", RECORD_CHANNEL, "2026-09-01").await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn list_store_suffixes_keeps_same_role_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for name in ["2026-dev", "2026-dev-e1", "2026-dev-3f2a-c3d4", "2026-dev2", "2026-other", "not-a-date"] {
            tokio::fs::create_dir_all(root.join("0").join("memory-store").join(name)).await.unwrap();
        }

        let suffixes = list_store_suffixes(root, "0", "dev").await.unwrap();
        assert_eq!(
            suffixes,
            ["dev", "dev-3f2a-c3d4", "dev-e1"].into_iter().map(ArcStr::from).collect(),
            "同角色的全部后缀（含事件模式），dev2 / other 不属于本角色"
        );
        assert!(list_store_suffixes(root, "0", "none").await.unwrap().is_empty());
        assert!(list_store_suffixes(root, "9", "dev").await.unwrap().is_empty(), "没有存储目录时不报错");
    }

}
