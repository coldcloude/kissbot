//! 原始记录读取与 Case 原文副本读写。
//!
//! 记忆存储只有通道记录一类：思考/工具内容以 `Content::Think` / `ToolCall` / `ToolResult`
//! 内嵌在通道记录的 `content` 里，读取时不需要再按引用定位独立文件。
//! Case 原文副本单文件（`channel-records.jsonl`，行内容与记忆存储的记录格式一致，不再按日期分文件）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::{ChannelKey, ChannelRecord, Content, RecordPosition};
use kai_file::LineReader;
use tracing::warn;

use crate::error::Result;

/// 一条原始记录（记忆存储只有通道记录）
pub type RecordEntry = Arc<ChannelRecord>;

/// Case 原文副本文件：`{case_dir}/channel-records.jsonl`
pub const CASE_RECORD_FILE: &str = "channel-records.jsonl";

/// Case 原文副本文件：`{case_dir}/channel-records.jsonl`
pub fn case_record_path(case_dir: &Path) -> PathBuf {
    case_dir.join(CASE_RECORD_FILE)
}

/// Case 目录：`{role_dir}/cases/{目录路径}`，目录与 case 树一一对应：
/// 隐藏角色根 `[role]` → `role`，事件根 `[event]{event_id}` → `event-{event_id}`，
/// 角色 case 的普通模型名段 → `role/{模型名}`；子 case 就是子目录。
pub fn case_dir(role_dir: &Path, case_name: &str) -> PathBuf {
    let mut dir = role_dir.join("cases");
    let mut segments = case_name.split('/');
    let first = segments.next().unwrap_or("");
    if first == crate::case_name::ROLE_ROOT_CASE_NAME_STR {
        dir.push("role");
    } else if let Some(event_id) = first.strip_prefix(crate::case_name::EVENT_ROOT_PREFIX) {
        dir.push(format!("event-{}", event_id));
    } else {
        dir.push("role");
        dir.push(first);
    }
    for segment in segments {
        dir.push(segment);
    }
    dir
}

/// 枚举某记忆后缀在记忆存储中已存在的通道记录文件（按日期升序，只取 `date >= from_date`）。
///
/// 直接枚举 `memory-store` 下的年目录，避免按日期逐个探测不存在的文件；
/// 不设上界：记录时间可能晚于本机当前日期，上界过滤会漏读。
pub async fn list_store_files(
    root: &Path,
    agent_id: &str,
    suffix: &str,
    from_date: &str,
) -> Result<Vec<(String, PathBuf)>> {
    let store_root = root.join(agent_id).join("memory-store");
    let mut year_dirs = match tokio::fs::read_dir(&store_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let prefix = "channel-records-";
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
            let Some(date) = file_name.strip_prefix(prefix).and_then(|n| n.strip_suffix(".jsonl")) else {
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

/// 共享记忆文件的全量视图：全部通道消息（按通道记录 `(time, sn)` 升序）
#[derive(Debug, Clone, Default)]
pub struct StoreView {
    pub messages: Vec<StoreMessage>,
}

/// 一条通道消息：通道记录本身。思考/工具内容内嵌在 `record.content` 中，不单独成条。
#[derive(Debug, Clone)]
pub struct StoreMessage {
    /// 通道归属（记录自身的 `(messenger_id, self_user_id, group_id)`）
    pub channel: ChannelKey,
    /// 通道记录（`content` 中的思考/工具为内嵌记录）
    pub record: RecordEntry,
}

impl StoreView {
    /// 记录中出现过的全部通道（按三元组升序）
    pub fn known_channels(&self) -> BTreeSet<Arc<ChannelKey>> {
        self.messages.iter().map(|message| Arc::new(message.channel.clone())).collect()
    }

    /// 某通道水位之后的待归档消息（按通道记录 `(time, sn)` 升序）
    pub fn pending_for(&self, channel: &ChannelKey, watermark: &RecordPosition) -> Vec<StoreMessage> {
        self.messages
            .iter()
            .filter(|message| message.channel == *channel)
            .filter(|message| watermark.is_before(message.record.time.as_str(), message.record.sn))
            .cloned()
            .collect()
    }
}

/// 读取某记忆后缀的共享记忆文件视图（按通道记录 `(time, sn)` 升序）。
///
/// 不做水位过滤：按通道判定批次需要看到全部记录；水位过滤由 [`StoreView::pending_for`] 按通道完成。
pub async fn read_store_records(root: &Path, agent_id: &str, suffix: &str) -> Result<StoreView> {
    let from_date = kissbot_api::RECORD_ZERO_TIME[..10].to_string();
    let files = list_store_files(root, agent_id, suffix, &from_date).await?;
    let mut messages: Vec<StoreMessage> = Vec::new();
    for (_, path) in files {
        let mut reader = LineReader::new(&path, None, None).await?;
        while let Some(line) = reader.next_line().await? {
            let text = line.line.trim();
            if text.is_empty() {
                continue;
            }
            let record: ChannelRecord = match serde_json::from_str(text) {
                Ok(record) => record,
                Err(e) => {
                    // 写入侧可能正在追加（半行）、历史脏数据或旧格式：跳过不影响后续记录
                    warn!("[summary] skip unparsable channel record in {}: {}", path.display(), e);
                    continue;
                }
            };
            let channel = ChannelKey::from(&record);
            messages.push(StoreMessage { channel, record: Arc::new(record) });
        }
    }
    messages.sort_by(|a, b| (a.record.time.as_str(), a.record.sn).cmp(&(b.record.time.as_str(), b.record.sn)));
    Ok(StoreView { messages })
}

/// 把批次记录追加为 case 的原文副本（单文件、保持记忆存储的记录格式）。
///
/// 等待写入真正完成：`tokio::fs::File` 的 `write_all` 可能只把数据交给后台阻塞任务就返回，
/// 随后同一进程内的读取（摘要重算、case 查询）会看到不完整的内容。
pub async fn append_case_records(dir: &Path, entries: &[RecordEntry]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    tokio::fs::create_dir_all(dir).await?;
    use tokio::io::AsyncWriteExt;
    let mut buffer = String::new();
    for entry in entries {
        buffer.push_str(&serde_json::to_string(entry.as_ref())?);
        buffer.push('\n');
    }
    let path = case_record_path(dir);
    let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await?;
    file.write_all(buffer.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

/// 取游标之后的原文副本（按 `(time, sn)` 比较；游标与记录一一对应）。
/// 调用方把上一条记录的 `time` / `sn` 原样回传即可连续分页。
pub fn records_after(records: &[RecordEntry], after: &RecordPosition) -> Vec<RecordEntry> {
    records
        .iter()
        .filter(|record| (record.time.as_str(), record.sn) > (after.time.as_str(), after.sn))
        .cloned()
        .collect()
}

/// 取结尾若干条原文副本（保持升序）
pub fn records_tail(records: &[RecordEntry], count: u64) -> Vec<RecordEntry> {
    let count = (count as usize).min(records.len());
    records[records.len() - count..].to_vec()
}

/// 读取 case 的全部原文副本（按 `(time, sn)` 升序）
pub async fn read_case_records(dir: &Path) -> Result<Vec<RecordEntry>> {
    let path = case_record_path(dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    let mut reader = LineReader::new(&path, None, None).await?;
    while let Some(line) = reader.next_line().await? {
        let text = line.line.trim();
        if text.is_empty() {
            continue;
        }
        match serde_json::from_str::<ChannelRecord>(text) {
            Ok(record) => records.push(Arc::new(record)),
            Err(e) => warn!("[summary] skip unparsable case record in {}: {}", path.display(), e),
        }
    }
    records.sort_by(|a, b| (a.time.as_str(), a.sn).cmp(&(b.time.as_str(), b.sn)));
    Ok(records)
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
    format!("[{}] {}: {}", entry.time, entry.user_name, render_content(&entry.content))
}

fn think_text(record: &kissbot_api::ThinkRecord) -> String {
    let mut parts = Vec::new();
    if let Some(reasoning) = record.reasoning_content.as_ref().filter(|text| !text.is_empty()) {
        parts.push(format!("<reasoning_content>{reasoning}</reasoning_content>"));
    }
    if let Some(thinking) = record.thinking.as_ref().filter(|text| !text.is_empty()) {
        parts.push(format!("<think>{thinking}</think>"));
    }
    parts.join("\n")
}

/// 一条消息的原文文本：内容里的思考/工具记录按其在 `Multi` 中的顺序内嵌，整条消息合成一个 String。
///
/// 标签约定：`reasoning_content` → `<reasoning_content>…</reasoning_content>`、
/// `thinking` → `<think>…</think>`、工具调用 → `<tool_call>…</tool_call>`、
/// 工具结果 → `<tool_result>…</tool_result>`。
pub fn synthesize_content(content: &Content) -> String {
    match content {
        Content::Multi(items) => items
            .iter()
            .map(synthesize_content)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Content::Think(record) => think_text(record),
        Content::ToolCall(record) => format!("<tool_call>{} {}</tool_call>", record.tool_name, record.tool_params),
        Content::ToolResult(record) => {
            let payload = serde_json::to_string(&record.tool_result).unwrap_or_else(|_| String::new());
            format!("<tool_result>{}</tool_result>", payload)
        }
        other => render_content(other),
    }
}

/// 渲染公共消息内容（思考/工具内容按内嵌记录渲染，供摘要等场景使用）
pub fn render_content(content: &Content) -> String {
    match content {
        Content::Text(text) => text.as_str().to_string(),
        Content::Multi(items) => items.iter().map(synthesize_content).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n"),
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
    use kissbot_api::{ContentBlock, RoleEvent, ThinkRecord, ToolCallRecord, ToolResultRecord};

    fn channel_record(channel: &ChannelKey, time: &str, sn: u64, content: Content) -> RecordEntry {
        Arc::new(ChannelRecord {
            meta: None,
            user_id: ArcStr::from("u1"),
            self_user_id: channel.self_user_id.clone(),
            messenger_id: channel.messenger_id.clone(),
            group_id: channel.group_id.clone(),
            is_self: 1,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::from("alice"),
            group_name: ArcStr::new(),
            content,
            time: ArcStr::from(time),
            sn,
        })
    }

    fn channel_key() -> ChannelKey {
        ChannelKey::new(ArcStr::from("web"), ArcStr::from("selfA"), ArcStr::from("gA"))
    }

    fn detail_content() -> Content {
        Content::Multi(Arc::new(vec![
            Content::Think(Arc::new(ThinkRecord {
                reasoning_content: Some(ArcStr::from("思考1")),
                thinking: None,
            })),
            Content::ToolCall(Arc::new(ToolCallRecord {
                tool_name: ArcStr::from("t"),
                tool_params: Arc::new(serde_json::json!({})),
            })),
            Content::ToolResult(Arc::new(ToolResultRecord { tool_result: None })),
            Content::Text(ArcStr::from("回复")),
        ]))
    }

    /// 往共享记忆文件追加一行记录（模拟 memory-store 的落盘）
    async fn append_store_line(root: &Path, agent_id: &str, suffix: &str, record: &ChannelRecord, time: &str) {
        let date = &time[..10];
        let year = &time[..4];
        let dir = root.join(agent_id).join("memory-store").join(format!("{}-{}", year, suffix));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("channel-records-{}.jsonl", date)))
            .await
            .unwrap();
        file.write_all(serde_json::to_string(record).unwrap().as_bytes()).await.unwrap();
        file.write_all(b"\n").await.unwrap();
        file.flush().await.unwrap();
    }

    #[test]
    fn synthesize_content_inlines_detail_records() {
        let text = synthesize_content(&detail_content());
        assert_eq!(
            text,
            "<reasoning_content>思考1</reasoning_content>\n<tool_call>t {}</tool_call>\n<tool_result>null</tool_result>\n回复"
        );
        // 多层 Multi 递归合成
        let nested = Content::Multi(Arc::new(vec![
            Content::Text(ArcStr::from("a")),
            Content::Multi(Arc::new(vec![Content::Text(ArcStr::from("b"))])),
        ]));
        assert_eq!(synthesize_content(&nested), "a\nb");
        assert_eq!(synthesize_content(&Content::Text(ArcStr::from("你好"))), "你好");
    }

    #[test]
    fn render_entry_renders_channel_message() {
        let entry = channel_record(&channel_key(), "2026-09-22 13:48:37", 1, Content::Text(ArcStr::from("你好")));
        assert_eq!(render_entry(&entry), "[2026-09-22 13:48:37] alice: 你好");
        let with_details = channel_record(&channel_key(), "2026-09-22 13:48:38", 2, detail_content());
        let text = render_entry(&with_details);
        assert!(text.contains("(思考)") == false, "思考内容按标签内嵌，不再有独立记录渲染");
        assert!(text.contains("<reasoning_content>思考1</reasoning_content>"));
        assert!(text.contains("<tool_call>t {}</tool_call>"));
    }

    #[tokio::test]
    async fn read_store_records_keeps_channel_content_and_order() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let channel = channel_key();
        let first = channel_record(&channel, "2026-09-22 23:00:00", 1, Content::Text(ArcStr::from("问")));
        let second = channel_record(&channel, "2026-09-22 23:00:01", 2, detail_content());
        append_store_line(root, "0", "dev", first.as_ref(), "2026-09-22 23:00:00").await;
        append_store_line(root, "0", "dev", second.as_ref(), "2026-09-22 23:00:01").await;

        let view = read_store_records(root, "0", "dev").await.unwrap();
        assert_eq!(view.messages.len(), 2);
        assert!(view.messages.iter().all(|message| message.channel == channel));
        assert_eq!(view.known_channels().len(), 1);
        assert_eq!(view.messages[0].record.sn, 1);
        assert_eq!(synthesize_content(&view.messages[1].record.content), synthesize_content(&detail_content()));
    }

    #[test]
    fn store_view_pending_filters_by_channel_and_watermark() {
        let web_g1 = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"));
        let web_g2 = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self2"), ArcStr::from("g2"));
        let view = StoreView {
            messages: vec![
                StoreMessage { channel: web_g1.clone(), record: channel_record(&web_g1, "2026-09-22 10:00:00", 1, Content::Text(ArcStr::from("a"))) },
                StoreMessage { channel: web_g2.clone(), record: channel_record(&web_g2, "2026-09-22 10:05:00", 3, Content::Text(ArcStr::from("b"))) },
            ],
        };
        assert_eq!(
            view.known_channels().iter().map(|channel| channel.as_ref().clone()).collect::<Vec<_>>(),
            vec![web_g1.clone(), web_g2.clone()]
        );

        // 无水位：该通道的全部消息
        let all = view.pending_for(&web_g1, &RecordPosition::default());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].record.sn, 1);

        // 水位推进到该记录：该通道清空；另一通道不受影响
        let watermark = RecordPosition { time: ArcStr::from("2026-09-22 10:00:00"), sn: 1 };
        assert!(view.pending_for(&web_g1, &watermark).is_empty());
        assert_eq!(view.pending_for(&web_g2, &RecordPosition::default()).len(), 1);
    }

    #[test]
    fn case_record_path_is_single_channel_file() {
        let dir = Path::new("/root/0/memory-struct-summary/dev/cases/e12");
        assert_eq!(case_record_path(dir), dir.join("channel-records.jsonl"));
    }

    fn case_records_fixture() -> Vec<RecordEntry> {
        let channel = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"));
        vec![
            channel_record(&channel, "2026-09-22 10:00:00", 1, Content::Text(ArcStr::from("问"))),
            channel_record(&channel, "2026-09-22 10:01:00", 2, Content::Text(ArcStr::from("答"))),
            channel_record(&channel, "2026-09-22 10:02:00", 3, Content::Text(ArcStr::from("再问"))),
        ]
    }

    #[test]
    fn records_after_uses_cursor_rule() {
        let records = case_records_fixture();
        // 游标为第一条：同位置的记录不算在其后
        let after = RecordPosition::of(records[0].as_ref());
        let rest = records_after(&records, &after);
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].sn, 2);

        // 游标为最后一条：之后为空
        assert!(records_after(&records, &RecordPosition::of(records[2].as_ref())).is_empty());
        // 游标晚于全部记录：之后为空
        let cursor = RecordPosition { time: ArcStr::from("2099-01-01 00:00:00"), sn: 1 };
        assert!(records_after(&records, &cursor).is_empty());
    }

    #[test]
    fn records_tail_keeps_last_ascending() {
        let records = case_records_fixture();
        assert!(records_tail(&records, 0).is_empty());
        let last = records_tail(&records, 1);
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].sn, 3);
        assert_eq!(records_tail(&records, 10).len(), 3, "超过总数时返回全部");
        assert_eq!(records_tail(&records, 2)[0].sn, 2, "升序返回");
    }

    #[tokio::test]
    async fn append_and_read_case_records_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cases/e1");
        let channel = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"));
        let entries = vec![
            channel_record(&channel, "2026-09-22 10:00:01", 2, Content::Text(ArcStr::from("第二条"))),
            channel_record(&channel, "2026-09-22 10:00:00", 1, detail_content()),
        ];
        append_case_records(&dir, &entries).await.unwrap();

        // 单文件，行内容与记忆存储一致（不含类型标签）
        let raw = tokio::fs::read_to_string(dir.join("channel-records.jsonl")).await.unwrap();
        assert_eq!(raw.lines().count(), 2);
        assert!(!raw.contains("record_type"));

        // 读回按 (time, sn) 升序
        let records = read_case_records(&dir).await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].sn, 1);
        assert_eq!(records[1].sn, 2);
        assert_eq!(records[0].time, "2026-09-22 10:00:00");
        assert_eq!(records[0].content, detail_content());
    }

    #[tokio::test]
    async fn append_case_records_accumulates_across_batches() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cases/e1");
        let channel = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"));
        append_case_records(&dir, &[channel_record(&channel, "2026-09-22 10:00:00", 1, Content::Text(ArcStr::from("a")))]).await.unwrap();
        append_case_records(&dir, &[channel_record(&channel, "2026-09-22 10:00:01", 2, Content::Text(ArcStr::from("b")))]).await.unwrap();
        let records = read_case_records(&dir).await.unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn case_dir_layout() {
        let role = Path::new("/root/0/memory-struct-summary/dev");
        assert_eq!(
            case_dir(role, "weather_chat"),
            PathBuf::from("/root/0/memory-struct-summary/dev/cases/role/weather_chat")
        );
        assert_eq!(
            case_dir(role, "weather_chat/rain"),
            PathBuf::from("/root/0/memory-struct-summary/dev/cases/role/weather_chat/rain")
        );
        assert_eq!(
            case_dir(role, "[event]e12"),
            PathBuf::from("/root/0/memory-struct-summary/dev/cases/event-e12")
        );
        assert_eq!(
            case_dir(role, "[event]e12/topic"),
            PathBuf::from("/root/0/memory-struct-summary/dev/cases/event-e12/topic")
        );
        assert_eq!(
            case_dir(role, "[role]"),
            PathBuf::from("/root/0/memory-struct-summary/dev/cases/role")
        );
    }

    #[tokio::test]
    async fn list_store_files_matches_suffix_and_date_range() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // 记忆存储布局：{root}/{agent}/memory-store/{year}-{记忆后缀}/channel-records-{date}.jsonl
        let dir = root.join("0").join("memory-store").join("2026-dev");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        for name in ["channel-records-2026-09-21.jsonl", "channel-records-2026-09-22.jsonl"] {
            tokio::fs::write(dir.join(name), b"").await.unwrap();
        }
        // 后缀更长的目录不应误匹配（"dev-x" 不是 "dev"）
        let other = root.join("0").join("memory-store").join("2026-dev-x");
        tokio::fs::create_dir_all(&other).await.unwrap();
        tokio::fs::write(other.join("channel-records-2026-09-22.jsonl"), b"").await.unwrap();

        let files = list_store_files(root, "0", "dev", "2026-09-22").await.unwrap();
        assert_eq!(files.len(), 1, "只取不早于起始日期且匹配后缀的文件");
        assert_eq!(files[0].0, "2026-09-22");
        assert!(files[0].1.ends_with("2026-dev/channel-records-2026-09-22.jsonl"));

        let files = list_store_files(root, "0", "dev", "2026-09-01").await.unwrap();
        assert_eq!(files.len(), 2, "起始日期之后的所有日期文件");
        assert_eq!(files[0].0, "2026-09-21", "按日期升序");

        let empty = list_store_files(root, "none", "dev", "2026-09-01").await.unwrap();
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

    #[test]
    fn role_event_suffix_is_available_for_store_paths() {
        // 记录路径只按角色名（角色模式）或角色名-事件标识（事件模式）拼接
        let role = RoleEvent::new(ArcStr::from("dev"));
        assert_eq!(kissbot_memory::role_event_suffix(&role), "dev");
        let event = RoleEvent::with_event(ArcStr::from("dev"), ArcStr::from("e1"));
        assert_eq!(kissbot_memory::role_event_suffix(&event), "dev-e1");
        // 思考/工具记录内部嵌在 content 中：不需要额外的记录类型常量
        let _ = ContentBlock::text(ArcStr::from("t"));
    }
}
