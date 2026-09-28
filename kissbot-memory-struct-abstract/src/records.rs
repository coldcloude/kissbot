//! 原始记录读取与 Case 原文副本读写。
//!
//! 读取侧按共享记忆文件（`{root}/{agent}/memory-store/{year}-{记忆后缀}/…`）正序读取四类记录，
//! 并给出每条记录的通道归属（通道记录按自身身份，思考/工具记录按 key 关联的占位通道记录）；
//! Case 原文副本按类型分文件（`{kind}-records.jsonl`，行内容与记忆存储的记录格式一致，不再按日期分文件）。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kissbot_api::{
    CaseCursor, CaseRecord, ChannelKey, ChannelRecord, Content, RecordKind, ThinkRecord,
    ToolCallRecord, ToolResultRecord, Watermark,
};
use kai_file::LineReader;
use serde_json::Value;
use tracing::warn;

use crate::error::Result;

/// 一条原始记录（读取阶段带类型；落盘为记忆存储的记录格式）
#[derive(Debug, Clone)]
pub struct RecordEntry {
    pub kind: RecordKind,
    pub time: Arc<String>,
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
    /// 记忆存储中的记录 JSON（case 原文副本的落盘格式）
    pub fn to_value(&self) -> Result<Value> {
        Ok(match &self.body {
            RecordBody::Channel(r) => serde_json::to_value(r.as_ref())?,
            RecordBody::Think(r) => serde_json::to_value(r.as_ref())?,
            RecordBody::ToolCall(r) => serde_json::to_value(r.as_ref())?,
            RecordBody::ToolResult(r) => serde_json::to_value(r.as_ref())?,
        })
    }

    /// 思考/工具记录关联详情用的 key（通道记录返回其内容关联的 key）
    pub fn linked_key(&self) -> Option<&str> {
        match &self.body {
            RecordBody::Channel(r) => placeholder_key(r.as_ref()),
            RecordBody::Think(r) => Some(r.key.as_str()),
            RecordBody::ToolCall(r) => Some(r.key.as_str()),
            RecordBody::ToolResult(r) => Some(r.key.as_str()),
        }
    }

    /// 记录自身的通道身份（仅通道记录有）
    pub fn channel_key(&self) -> Option<ChannelKey> {
        match &self.body {
            RecordBody::Channel(r) => Some(ChannelKey::from(r.as_ref())),
            _ => None,
        }
    }
}

/// 占位通道记录关联的详情 key（内容为 Think / ToolCall / ToolResult）
fn placeholder_key(record: &ChannelRecord) -> Option<&str> {
    match &record.content {
        Content::Think(key) | Content::ToolCall(key) | Content::ToolResult(key) => Some(key.as_str()),
        _ => None,
    }
}

impl PartialEq for RecordEntry {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.time == other.time && self.sn == other.sn
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
pub fn case_record_path(case_dir: &Path, kind: RecordKind) -> PathBuf {
    case_dir.join(format!("{}-records.jsonl", kind.file_stem()))
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
    kind: RecordKind,
    from_date: &str,
) -> Result<Vec<(String, PathBuf)>> {
    let store_root = root.join(agent_id).join("memory-store");
    let mut year_dirs = match tokio::fs::read_dir(&store_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let prefix = format!("{}-records-", kind.file_stem());
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
pub async fn list_store_suffixes(root: &Path, agent_id: &str, role: &str) -> Result<BTreeSet<String>> {
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
            suffixes.insert(suffix.to_string());
        }
    }
    Ok(suffixes)
}

/// 共享记忆文件的全量视图：记录与逐条对应的通道归属
#[derive(Debug, Clone, Default)]
pub struct StoreView {
    /// 四类记录（按 (time, sn) 升序）
    pub records: Vec<RecordEntry>,
    /// 与 `records` 一一对应的通道归属；`None` 表示未被任何通道认领
    pub channels: Vec<Option<ChannelKey>>,
}

impl StoreView {
    /// 记录中出现过的全部通道（按三元组升序）
    pub fn known_channels(&self) -> BTreeSet<ChannelKey> {
        self.channels.iter().flatten().cloned().collect()
    }

    /// 某通道水位之后的待归档记录（按 (time, sn) 升序）
    pub fn pending_for(&self, channel: &ChannelKey, watermark: &Watermark) -> Vec<RecordEntry> {
        self.records
            .iter()
            .zip(&self.channels)
            .filter(|(_, of)| of.as_ref() == Some(channel))
            .filter(|(record, _)| {
                let point = watermark.point(record.kind);
                (record.time.as_str(), record.sn) > (point.time.as_str(), point.sn)
            })
            .map(|(record, _)| record.clone())
            .collect()
    }
}

/// 读取某记忆后缀的共享记忆文件全部四类记录（按 (time, sn) 升序）并标注通道归属。
///
/// 不做水位过滤：按通道判定批次需要看到全部记录——思考/工具记录的归属依赖同 key 的占位通道记录，
/// 该占位记录可能已被归档到更早的水位之下；水位过滤由 [`StoreView::pending_for`] 按通道完成。
pub async fn read_store_records(root: &Path, agent_id: &str, suffix: &str) -> Result<StoreView> {
    let from_date = kissbot_api::WATERMARK_ZERO_TIME[..10].to_string();
    let mut entries = Vec::new();
    for kind in RecordKind::ALL {
        let files = list_store_files(root, agent_id, suffix, kind, &from_date).await?;
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
                let (time, sn) = match value_time_sn(&value) {
                    Some(v) => v,
                    None => continue,
                };
                match parse_entry(kind, time, sn, &value) {
                    Ok(entry) => entries.push(entry),
                    Err(e) => warn!("[abstract] skip malformed record in {}: {}", path.display(), e),
                }
            }
        }
    }
    entries.sort();
    let channels = attribute_channels(&entries);
    Ok(StoreView { records: entries, channels })
}

/// 通道归属：通道记录按自身身份；思考/工具记录按 key 找到同 key 的思考/工具占位通道记录，
/// 取其通道身份（找不到占位记录的记录未被认领，不进入任何通道的批次）。
pub fn attribute_channels(records: &[RecordEntry]) -> Vec<Option<ChannelKey>> {
    let mut owners: HashMap<&str, ChannelKey> = HashMap::new();
    for entry in records {
        if let (Some(key), Some(channel)) = (entry.linked_key(), entry.channel_key()) {
            owners.entry(key).or_insert(channel);
        }
    }
    records
        .iter()
        .map(|entry| match entry.kind {
            // 通道记录自身的身份即归属（含思考/工具占位记录）
            RecordKind::Channel => entry.channel_key(),
            // 思考/工具记录按 key 认领
            _ => entry.linked_key().and_then(|key| owners.get(key).cloned()),
        })
        .collect()
}

fn parse_entry(kind: RecordKind, time: String, sn: u64, value: &Value) -> Result<RecordEntry> {
    let time = Arc::new(time);
    let body = match kind {
        RecordKind::Channel => RecordBody::Channel(Arc::new(serde_json::from_value::<ChannelRecord>(value.clone())?)),
        RecordKind::Think => RecordBody::Think(Arc::new(serde_json::from_value::<ThinkRecord>(value.clone())?)),
        RecordKind::ToolCall => RecordBody::ToolCall(Arc::new(serde_json::from_value::<ToolCallRecord>(value.clone())?)),
        RecordKind::ToolResult => RecordBody::ToolResult(Arc::new(serde_json::from_value::<ToolResultRecord>(value.clone())?)),
    };
    Ok(RecordEntry { kind, time, sn, body })
}

/// 从记录 JSON 取 (time, sn)
pub fn value_time_sn(value: &Value) -> Option<(String, u64)> {
    let time = value.get("time")?.as_str()?.to_string();
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
    let mut buffers: BTreeMap<RecordKind, String> = BTreeMap::new();
    for entry in entries {
        let line = serde_json::to_string(&entry.to_value()?)?;
        let buffer = buffers.entry(entry.kind).or_default();
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
            Ordering::Equal => record.kind() != after.record_type || record.sn() > after.sn,
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
    for kind in RecordKind::ALL {
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

fn parse_case_record(kind: RecordKind, text: &str) -> Result<CaseRecord> {
    Ok(match kind {
        RecordKind::Channel => CaseRecord::Channel(Arc::new(serde_json::from_str::<ChannelRecord>(text)?)),
        RecordKind::Think => CaseRecord::Think(Arc::new(serde_json::from_str::<ThinkRecord>(text)?)),
        RecordKind::ToolCall => CaseRecord::ToolCall(Arc::new(serde_json::from_str::<ToolCallRecord>(text)?)),
        RecordKind::ToolResult => CaseRecord::ToolResult(Arc::new(serde_json::from_str::<ToolResultRecord>(text)?)),
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
            let payload = if !r.tool_error.is_null() { &r.tool_error } else { &r.tool_result };
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
            let payload = if !r.tool_error.is_null() { &r.tool_error } else { &r.tool_result };
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

/// 渲染公共消息内容
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

    fn channel_entry(time: &str, sn: u64, is_self: usize) -> RecordEntry {
        RecordEntry {
            kind: RecordKind::Channel,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
                user_id: Arc::new("u1".into()),
                self_user_id: Arc::new("self".into()),
                messenger_id: Arc::new("web".into()),
                group_id: Arc::new("g1".into()),
                is_self,
                msg_id: Arc::new(String::new()),
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new("alice".into()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("你好".into())),
                time: Arc::new(time.to_string()),
                sn,
            })),
        }
    }

    fn think_entry(time: &str, sn: u64) -> RecordEntry {
        RecordEntry {
            kind: RecordKind::Think,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Think(Arc::new(ThinkRecord {
                reasoning_content: Some(Arc::new("推理".into())),
                thinking: None,
                key: Arc::new("k1".into()),
                time: Arc::new(time.to_string()),
                sn,
            })),
        }
    }

    /// 思考/工具占位通道记录：内容为详情 key，身份与 agent 的回复通道一致
    fn placeholder_entry(time: &str, sn: u64, key: &str, self_user: &str, group: &str) -> RecordEntry {
        RecordEntry {
            kind: RecordKind::Channel,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
                user_id: Arc::new(self_user.to_string()),
                self_user_id: Arc::new(self_user.to_string()),
                messenger_id: Arc::new("web".into()),
                group_id: Arc::new(group.to_string()),
                is_self: 1,
                msg_id: Arc::new(String::new()),
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Think(Arc::new(key.to_string())),
                time: Arc::new(time.to_string()),
                sn,
            })),
        }
    }

    #[test]
    fn attribute_channels_links_think_tool_by_placeholder_key() {
        let think = |time: &str, sn: u64, key: &str| RecordEntry {
            kind: RecordKind::Think,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Think(Arc::new(ThinkRecord {
                reasoning_content: None,
                thinking: Some(Arc::new("思考".into())),
                key: Arc::new(key.to_string()),
                time: Arc::new(time.to_string()),
                sn,
            })),
        };
        let records = vec![
            channel_entry("2026-09-22 10:00:00", 1, 0),
            placeholder_entry("2026-09-22 10:00:01", 2, "k1", "self", "g1"),
            think("2026-09-22 10:00:01", 2, "k1"),
            // 另一个通道的思考记录：按自己的占位记录归属
            placeholder_entry("2026-09-22 10:05:00", 3, "k2", "self2", "g2"),
            think("2026-09-22 10:05:00", 3, "k2"),
            // 找不到占位记录：未被认领
            think("2026-09-22 10:06:00", 4, "k3"),
        ];
        let channels = attribute_channels(&records);
        let web_g1 = ChannelKey::new("web", "self", "g1");
        let web_g2 = ChannelKey::new("web", "self2", "g2");
        assert_eq!(channels[0].as_ref(), Some(&web_g1), "通道记录按自身身份");
        assert_eq!(channels[1].as_ref(), Some(&web_g1), "占位记录自身也是该通道");
        assert_eq!(channels[2].as_ref(), Some(&web_g1), "思考记录按 key 认领");
        assert_eq!(channels[3].as_ref(), Some(&web_g2));
        assert_eq!(channels[4].as_ref(), Some(&web_g2));
        assert!(channels[5].is_none(), "无占位记录的思考记录不归属任何通道");
    }

    #[test]
    fn store_view_pending_filters_by_channel_and_watermark() {
        let think = |time: &str, sn: u64, key: &str| RecordEntry {
            kind: RecordKind::Think,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Think(Arc::new(ThinkRecord {
                reasoning_content: None,
                thinking: Some(Arc::new("思考".into())),
                key: Arc::new(key.to_string()),
                time: Arc::new(time.to_string()),
                sn,
            })),
        };
        let records = vec![
            channel_entry("2026-09-22 10:00:00", 1, 0),
            placeholder_entry("2026-09-22 10:00:01", 2, "k1", "self", "g1"),
            think("2026-09-22 10:00:01", 2, "k1"),
            placeholder_entry("2026-09-22 10:05:00", 3, "k2", "self2", "g2"),
            think("2026-09-22 10:05:00", 3, "k2"),
        ];
        let channels = attribute_channels(&records);
        let view = StoreView { records, channels };
        let web_g1 = ChannelKey::new("web", "self", "g1");
        let web_g2 = ChannelKey::new("web", "self2", "g2");
        assert_eq!(view.known_channels().iter().cloned().collect::<Vec<_>>(), vec![web_g1.clone(), web_g2.clone()]);

        // 无水位：该通道的全部记录（含按 key 认领的思考记录）
        let all = view.pending_for(&web_g1, &Watermark::default());
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().map(|e| e.sn).collect::<Vec<_>>(), vec![1, 2, 2]);

        // 通道水位推进过首条记录、思考水位未动：占位记录与其后的思考记录仍在
        let mut partial = Watermark::default();
        partial.channel.time = Arc::new("2026-09-22 10:00:01".into());
        partial.channel.sn = 1;
        assert_eq!(view.pending_for(&web_g1, &partial).len(), 2);

        // 两类水位都推进到末尾：该通道清空；另一通道不受影响
        let mut watermark = Watermark::default();
        watermark.channel.time = Arc::new("2026-09-22 10:00:01".into());
        watermark.channel.sn = 2;
        watermark.think.time = Arc::new("2026-09-22 10:00:01".into());
        watermark.think.sn = 2;
        assert!(view.pending_for(&web_g1, &watermark).is_empty());
        assert_eq!(view.pending_for(&web_g2, &Watermark::default()).len(), 2);
    }

    #[test]
    fn render_entry_by_kind() {
        assert_eq!(render_entry(&channel_entry("2026-09-22 13:48:37", 1, 0)), "[2026-09-22 13:48:37] alice: 你好");
        assert_eq!(render_entry(&think_entry("2026-09-22 13:48:38", 1)), "[2026-09-22 13:48:38] (思考) 推理");

        let result = RecordEntry {
            kind: RecordKind::ToolResult,
            time: Arc::new("2026-09-22 13:48:40".into()),
            sn: 1,
            body: RecordBody::ToolResult(Arc::new(ToolResultRecord {
                tool_result: Arc::new(serde_json::json!({"v": 1})),
                tool_error: Arc::new(Value::Null),
                key: Arc::new("k1".into()),
                time: Arc::new("2026-09-22 13:48:40".into()),
                sn: 1,
            })),
        };
        assert!(render_entry(&result).contains("(工具结果)"));
    }

    #[test]
    fn case_record_path_is_per_kind_without_date() {
        let dir = Path::new("/root/0/memory-struct-abstract/dev/cases/e12");
        assert_eq!(case_record_path(dir, RecordKind::Channel), dir.join("channel-records.jsonl"));
        assert_eq!(case_record_path(dir, RecordKind::ToolCall), dir.join("tool-call-records.jsonl"));
    }

    fn case_records_fixture() -> Vec<CaseRecord> {
        vec![
            CaseRecord::Channel(Arc::new(ChannelRecord {
                user_id: Arc::new("u1".into()), self_user_id: Arc::new("self".into()),
                messenger_id: Arc::new("web".into()), group_id: Arc::new("g1".into()),
                is_self: 0, msg_id: Arc::new(String::new()),
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new("alice".into()), group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("问".into())),
                time: Arc::new("2026-09-22 10:00:00".into()), sn: 1,
            })),
            CaseRecord::Think(Arc::new(ThinkRecord {
                reasoning_content: Some(Arc::new("推理".into())), thinking: None,
                key: Arc::new("k1".into()),
                time: Arc::new("2026-09-22 10:00:00".into()), sn: 1,
            })),
            CaseRecord::Channel(Arc::new(ChannelRecord {
                user_id: Arc::new("self".into()), self_user_id: Arc::new("self".into()),
                messenger_id: Arc::new("web".into()), group_id: Arc::new("g1".into()),
                is_self: 1, msg_id: Arc::new(String::new()),
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new("bot".into()), group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("答".into())),
                time: Arc::new("2026-09-22 10:01:00".into()), sn: 2,
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
        assert_eq!(rest[0].kind(), RecordKind::Think, "同一时刻的其他类型记录视为在其后");
        assert_eq!(rest[1].kind(), RecordKind::Channel);

        // 游标为思考记录（10:00:00 sn1）：通道记录 sn1 视为在其后（跨类型不比 sn）
        let after = CaseCursor::of(&records[1]);
        let rest = records_after(&records, &after);
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].kind(), RecordKind::Channel);
        assert_eq!(rest[0].sn(), 1);

        // 游标为最后一条：之后为空
        assert!(records_after(&records, &CaseCursor::of(&records[2])).is_empty());
        // 游标晚于全部记录：之后为空
        let cursor = CaseCursor { time: Arc::new("2099-01-01 00:00:00".into()), sn: 1, record_type: RecordKind::Channel };
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
        assert_eq!(records[0].kind(), RecordKind::Think);
        assert_eq!(records[1].kind(), RecordKind::Channel);
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
        assert_eq!(value_time_sn(&value), Some(("2026-09-22 13:48:37".to_string(), 7)));
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

        let files = list_store_files(root, "0", "dev", RecordKind::Channel, "2026-09-22").await.unwrap();
        assert_eq!(files.len(), 1, "只取不早于起始日期且匹配类型/后缀的文件");
        assert_eq!(files[0].0, "2026-09-22");
        assert!(files[0].1.ends_with("2026-dev/channel-records-2026-09-22.jsonl"));

        let files = list_store_files(root, "0", "dev", RecordKind::Channel, "2026-09-01").await.unwrap();
        assert_eq!(files.len(), 2, "起始日期之后的所有日期文件");
        assert_eq!(files[0].0, "2026-09-21", "按日期升序");

        let empty = list_store_files(root, "none", "dev", RecordKind::Channel, "2026-09-01").await.unwrap();
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
            ["dev", "dev-3f2a-c3d4", "dev-e1"].into_iter().map(String::from).collect(),
            "同角色的全部后缀（含事件模式），dev2 / other 不属于本角色"
        );
        assert!(list_store_suffixes(root, "0", "none").await.unwrap().is_empty());
        assert!(list_store_suffixes(root, "9", "dev").await.unwrap().is_empty(), "没有存储目录时不报错");
    }

}
