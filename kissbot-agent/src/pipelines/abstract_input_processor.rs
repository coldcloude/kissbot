//! 摘要记忆输入处理器：按 channel（messenger + 接收方用户 + 群组）攒批，每秒定时检测，
//! 某 channel 的最后一条消息超时即触发该批：先经摘要记忆（定位事件 → 事件摘要 + 事件结尾原文副本）
//! 重建会话上下文，再把本批新消息交给 pipeline 运行。摘要记忆未配置或调用失败时降级为普通合批。

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;
use kissbot_api::{ChannelKey, ChannelRecord, IncomingMessageEvent, LocateMessage};
use kissbot_llm::{Message, Session};
use tokio::sync::Notify;

use crate::{
    abstract_client::AbstractClient,
    message::{MessageContent, extract_content, pack_memory_messages},
    types::SessionKey,
};

/// 消息去重键：时间 + 发送者名 + 文本 + 是否本人（同一条消息在两个来源里等价）
type MessageKey = (String, String, String, bool);

/// 单个 channel 的待处理批次
struct PendingBatch {
    events: Vec<Arc<IncomingMessageEvent>>,
    /// 最后一条消息到达时间（超时判定基准）
    last_at: Instant,
}

pub struct AbstractAgentInputProcessor {
    /// 绑定的会话
    session_key: Arc<SessionKey>,
    /// 会话实例（装配时注入；触发时用于重置上下文）
    session: Arc<Session>,
    /// 摘要记忆客户端（装配时注入；单测可传空地址实例）
    client: Arc<AbstractClient>,
    /// 各 channel 的待处理批次（accept 写入，tick 任务取走）
    batches: Arc<DashMap<ChannelKey, PendingBatch>>,
    /// 会话销毁通知（Drop 时 notify_one → tick 任务退出）
    notify: Arc<Notify>,
}

/// 消息所属通道：messenger + 接收方用户（agent 在该通道绑定的身份）+ 群组；发送者 user_id 不参与
fn event_channel(event: &IncomingMessageEvent) -> ChannelKey {
    ChannelKey {
        messenger_id: event.incoming_message.messenger_id.clone(),
        self_user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
    }
}

/// 已超时的 channel 列表：最后一条消息到达时间距现在已超过 timeout
fn due_channels(batches: &DashMap<ChannelKey, PendingBatch>, timeout: Duration) -> Vec<ChannelKey> {
    batches
        .iter()
        .filter(|entry| entry.value().last_at.elapsed() >= timeout)
        .map(|entry| entry.key().clone())
        .collect()
}

/// 新消息 → 定位请求消息（输入处理器只处理上行消息，is_self 固定 0）
fn locate_message(event: &IncomingMessageEvent) -> LocateMessage {
    LocateMessage {
        time: event.incoming_message.time.clone(),
        user_name: event.incoming_message.user_name.clone(),
        is_self: 0,
        messenger_id: event.incoming_message.messenger_id.clone(),
        self_user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
        content: event.incoming_message.content.clone(),
    }
}

/// 一条消息的去重键（时间 + 发送者 + 文本 + is_self 全同即视为同一条）
fn message_key(time: &str, user_name: &str, text: &str, is_self: bool) -> MessageKey {
    (time.to_string(), user_name.to_string(), text.to_string(), is_self)
}

/// 新消息的去重键（与通道记录的可比字段对齐：时间 / 发送者 / 文本 / is_self）
fn new_message_key(event: &IncomingMessageEvent) -> MessageKey {
    let mc = extract_content(event.incoming_message.user_name.clone(), &event.incoming_message.content, 0);
    let text = join_text(&mc);
    message_key(event.incoming_message.time.as_str(), mc.user_name.as_str(), &text, false)
}

/// 文本拼接（Multi 拆出的多个 Text 段以 \n 连接，与 pack_batch 一致）
fn join_text(mc: &MessageContent) -> String {
    mc.content.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
}

/// 历史记录过滤与去重：只留时间窗内（time >= cutoff）且文本非空的通道记录，
/// 事件之间按 (time, sn) 去重，并与本批新消息去重（查出的消息和新消息做去重）
fn filter_history(
    records: &[Arc<ChannelRecord>],
    cutoff: &str,
    new_keys: &HashSet<MessageKey>,
) -> Vec<MessageContent> {
    let mut sorted: Vec<&Arc<ChannelRecord>> = records.iter().collect();
    sorted.sort_by(|a, b| (a.time.as_str(), a.sn).cmp(&(b.time.as_str(), b.sn)));
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    let mut history = Vec::new();
    for record in sorted {
        if record.time.as_str() < cutoff {
            continue; // 时间窗外的记录不取（只取最后一段时间）
        }
        if !seen.insert((record.time.as_str().to_string(), record.sn)) {
            continue; // 事件之间/尾部重叠的同一条记录
        }
        let mc = extract_content(record.user_name.clone(), &record.content, record.is_self);
        if mc.content.is_empty() {
            continue; // 非文本记录跳过
        }
        let text = join_text(&mc);
        if new_keys.contains(&message_key(record.time.as_str(), record.user_name.as_str(), &text, record.is_self > 0)) {
            continue; // 与本批新消息重复：本批会进 pipeline，历史里去掉
        }
        history.push(mc);
    }
    history
}

/// 组装重置用的上下文：摘要块置首（并入首条 User，避免连续两条 User），其后为历史消息
fn compose_context(summaries: &[String], history: &[MessageContent]) -> Vec<Message> {
    let mut messages = pack_memory_messages(history);
    if summaries.is_empty() {
        return messages;
    }
    let block = format!(
        "【相关事件摘要】\n{}",
        summaries.iter().map(|s| format!("- {}", s)).collect::<Vec<_>>().join("\n")
    );
    if matches!(messages.first(), Some(Message::User { .. })) {
        if let Some(Message::User { content }) = messages.first_mut() {
            *content = Arc::new(format!("{}\n\n{}", block, content.as_str()));
        }
    } else {
        messages.insert(0, Message::User { content: Arc::new(block) });
    }
    messages
}

impl AbstractAgentInputProcessor {
    /// 只搭状态、不启动 tick 任务（生产构造与单测共用；单测不启动可避开进程级单例）
    fn assemble(session_key: Arc<SessionKey>, session: Arc<Session>, client: Arc<AbstractClient>) -> Self {
        Self {
            session_key,
            session,
            client,
            batches: Arc::new(DashMap::new()),
            notify: Arc::new(Notify::new()),
        }
    }

    /// 入批：按 channel 分桶并刷新最后一条到达时间（accept 与单测共用）
    fn enqueue(&self, event: Arc<IncomingMessageEvent>) {
        let key = event_channel(&event);
        let now = Instant::now();
        let mut batch = self
            .batches
            .entry(key)
            .or_insert_with(|| PendingBatch { events: Vec::new(), last_at: now });
        batch.events.push(event);
        batch.last_at = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;

    use crate::types::Mode;

    /// 构造测试用上行消息事件（通道由 messenger / self_user / group 指定）
    fn ev_in(messenger: &str, self_user: &str, group: &str, name: &str, text: &str, time: &str) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: Arc::new(self_user.into()),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: Arc::new("m".into()),
                messenger_id: Arc::new(messenger.into()),
                user_id: Arc::new("u".into()),
                group_id: Arc::new(group.into()),
                messenger_name: Arc::new("".into()),
                user_name: Arc::new(name.into()),
                group_name: Arc::new("".into()),
                content: Content::Text(Arc::new(text.into())),
                time: Arc::new(time.into()),
            }),
        })
    }

    fn ev(name: &str, text: &str, time: &str) -> Arc<IncomingMessageEvent> {
        ev_in("web", "self", "g1", name, text, time)
    }

    /// 构造测试用通道原文副本
    fn rec(user: &str, text: &str, is_self: usize, time: &str, sn: u64) -> Arc<ChannelRecord> {
        Arc::new(ChannelRecord {
            user_id: Arc::new(user.into()),
            self_user_id: Arc::new("self".into()),
            messenger_id: Arc::new("web".into()),
            group_id: Arc::new("g1".into()),
            is_self,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(user.into()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new(text.into())),
            time: Arc::new(time.into()),
            sn,
        })
    }

    fn mc(user_name: &str, text: &str, is_self: bool) -> MessageContent {
        MessageContent {
            user_name: Arc::new(user_name.into()),
            content: vec![Arc::new(text.into())],
            is_self,
        }
    }

    #[test]
    fn event_channel_uses_recipient_user_and_group() {
        // 通道三元组 = messenger + 接收方用户（self_user_id）+ 群组；发送者 user_id 不参与
        let e = ev("u1", "a", "2026-09-23 10:00:00");
        assert_eq!(event_channel(&e), ChannelKey::new("web", "self", "g1"));
    }

    #[test]
    fn locate_message_maps_recipient_to_self_user() {
        let e = ev("u1", "a", "2026-09-23 10:00:00");
        let lm = locate_message(&e);
        assert_eq!(lm.self_user_id.as_str(), "self");
        assert_eq!(lm.user_name.as_str(), "u1");
        assert_eq!(lm.is_self, 0, "输入处理器只处理上行消息");
        assert_eq!(lm.channel(), ChannelKey::new("web", "self", "g1"));
    }

    #[test]
    fn new_message_key_matches_record_identity() {
        let e = ev("u1", "hello", "2026-09-23 10:00:00");
        assert_eq!(new_message_key(&e), message_key("2026-09-23 10:00:00", "u1", "hello", false));
    }

    #[test]
    fn due_channels_only_returns_timed_out() {
        let batches = DashMap::new();
        batches.insert(
            ChannelKey::new("web", "self", "g1"),
            PendingBatch { events: vec![], last_at: Instant::now() },
        );
        batches.insert(
            ChannelKey::new("web", "self", "g2"),
            PendingBatch { events: vec![], last_at: Instant::now() - Duration::from_secs(10) },
        );
        let due = due_channels(&batches, Duration::from_secs(3));
        assert_eq!(due, vec![ChannelKey::new("web", "self", "g2")], "只有超时 channel 被触发");
    }

    #[tokio::test]
    async fn enqueue_buckets_by_channel() {
        let processor = test_processor();
        processor.enqueue(ev("u1", "a", "2026-09-23 10:00:00"));
        processor.enqueue(ev("u2", "b", "2026-09-23 10:00:01"));
        processor.enqueue(ev_in("web", "self", "g2", "u3", "c", "2026-09-23 10:00:02"));
        let g1 = ChannelKey::new("web", "self", "g1");
        let g2 = ChannelKey::new("web", "self", "g2");
        assert_eq!(processor.batches.get(&g1).unwrap().events.len(), 2, "同 channel 合入一批");
        assert_eq!(processor.batches.get(&g2).unwrap().events.len(), 1, "不同 channel 分开攒");
    }

    #[test]
    fn filter_history_windows_time_channel_and_dedups_new_messages() {
        let records = vec![
            rec("u1", "old", 0, "2026-09-23 09:00:00", 1),      // 时间窗外
            rec("u1", "dup", 0, "2026-09-23 10:00:01", 2),      // 与本批新消息重复
            rec("agent", "reply", 1, "2026-09-23 10:00:02", 3), // 本人回复：保留
            rec("u2", "new", 0, "2026-09-23 10:00:03", 4),
            rec("u2", "new", 0, "2026-09-23 10:00:03", 4),      // 同 (time, sn)：去重
        ];
        let new_keys = HashSet::from([message_key("2026-09-23 10:00:01", "u1", "dup", false)]);
        let history = filter_history(&records, "2026-09-23 10:00:00", &new_keys);
        assert_eq!(history.len(), 2, "窗外 1 条 + 重复 1 条 + 本批重复 1 条都被剔除");
        assert!(history[0].is_self, "本人回复保留");
        assert_eq!(history[0].content[0].as_str(), "reply");
        assert!(!history[1].is_self);
        assert_eq!(history[1].content[0].as_str(), "new");
    }

    #[test]
    fn compose_context_puts_summary_first_and_keeps_history() {
        let history = vec![mc("u1", "hi", false), mc("agent", "yo", true)];
        let messages = compose_context(&["事件A".to_string(), "事件B".to_string()], &history);
        assert_eq!(messages.len(), 2, "摘要并入首条 User，不新增消息");
        match &messages[0] {
            Message::User { content } => {
                assert!(content.starts_with("【相关事件摘要】"), "摘要块置首: {}", content);
                assert!(content.contains("- 事件A") && content.contains("- 事件B"));
                assert!(content.ends_with("u1: hi"), "原首条 User 内容保留");
            }
            other => panic!("首条应为 User: {:?}", other),
        }
        assert!(matches!(&messages[1], Message::Assistant { .. }));
    }

    #[test]
    fn compose_context_without_history_or_summaries() {
        // 无摘要无历史 → 空上下文
        assert!(compose_context(&[], &[]).is_empty());
        // 只有摘要 → 单条 User
        let only_summary = compose_context(&["事件A".to_string()], &[]);
        assert_eq!(only_summary.len(), 1);
        assert!(matches!(&only_summary[0], Message::User { content } if content.contains("事件A")));
    }

    fn test_processor() -> AbstractAgentInputProcessor {
        AbstractAgentInputProcessor::assemble(
            Arc::new(SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role }),
            Arc::new(Session::new("", "test-session")),
            Arc::new(AbstractClient::with_url(String::new(), String::new())),
        )
    }
}
