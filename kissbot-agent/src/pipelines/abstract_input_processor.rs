//! 摘要记忆输入处理器：按 channel（messenger + 接收方用户 + 群组）攒批，每秒定时检测，
//! 某 channel 的最后一条消息超时即触发该批：先经摘要记忆（定位 case → case 摘要 + case 结尾原文副本）
//! 重建会话上下文，再把本批新消息交给 pipeline 运行。摘要记忆未配置或调用失败时降级为普通合批。

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use arcstr::ArcStr;
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Local};
use dashmap::DashMap;
use kissbot_api::{ChannelKey, ChannelRecord, CaseRecord, IncomingMessageEvent, LocateMessage};
use kissbot_llm::{Message, Session};
use tokio::sync::Notify;
use tracing::warn;

use crate::{
    abstract_client::AbstractClient,
    config_manager::ConfigManager,
    configs::{ChannelBatchConfig, EffectiveMemoryRecoverConfig, MemoryRecoverConfig},
    message::{MessageContent, extract_content, pack_batch, pack_memory_messages},
    nexus::Nexus,
    pipeline::{AgentInputProcessor, AgentTrigger},
    types::{Result, SessionKey},
};

/// tick 间隔（秒）：每秒定时检测是否有 channel 已超时
const TICK_INTERVAL_SECS: u64 = 1;

/// 单个 channel 的待处理批次
struct PendingBatch {
    cases: Vec<Arc<IncomingMessageEvent>>,
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
        msg_id: event.incoming_message.msg_id.clone(),
        messenger_id: event.incoming_message.messenger_id.clone(),
        self_user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
        content: event.incoming_message.content.clone(),
    }
}

/// 一条消息的去重键：通道三元组 + msg_id（同一条消息在两个来源里等价）
type MessageKey = (ArcStr, ArcStr, ArcStr, ArcStr);

/// 去重键：msg_id 为空（思考/工具记录、旧记录）时返回 None，不参与去重、原样保留
fn message_key(messenger_id: &str, self_user_id: &str, group_id: &str, msg_id: &str) -> Option<MessageKey> {
    if msg_id.is_empty() {
        return None;
    }
    Some((ArcStr::from(messenger_id), ArcStr::from(self_user_id), ArcStr::from(group_id), ArcStr::from(msg_id)))
}

/// 新消息的去重键
fn new_message_key(event: &IncomingMessageEvent) -> Option<MessageKey> {
    message_key(
        event.incoming_message.messenger_id.as_str(),
        event.recipient_user_id.as_str(),
        event.incoming_message.group_id.as_str(),
        event.incoming_message.msg_id.as_str(),
    )
}

/// 通道记录的去重键
fn record_message_key(record: &ChannelRecord) -> Option<MessageKey> {
    message_key(
        record.messenger_id.as_str(),
        record.self_user_id.as_str(),
        record.group_id.as_str(),
        record.msg_id.as_str(),
    )
}

/// 合并各 case 结尾的原文副本：只保留通道记录；各 case **内部**按 (time, sn) 去重
/// （sn 在各 case 自己的记录文件里独立编号，跨 case 不可比，因此不能跨 case 去重）
fn merge_tail_records(tails: &[Vec<Arc<CaseRecord>>]) -> Vec<Arc<ChannelRecord>> {
    let mut records = Vec::new();
    for tail in tails {
        let mut seen: HashSet<(ArcStr, u64)> = HashSet::new();
        for record in tail {
            if let CaseRecord::Channel(channel_record) = record.as_ref() {
                if seen.insert((channel_record.time.clone(), channel_record.sn)) {
                    records.push(channel_record.clone());
                }
            }
        }
    }
    records
}

/// 历史记录过滤：只留时间窗内（time >= cutoff）且文本非空的记录，按 (time, sn) 升序，
/// 并与本批新消息按「通道三元组 + msg_id」去重（msg_id 为空的记录不参与去重，原样保留）
fn filter_history(
    records: &[Arc<ChannelRecord>],
    cutoff: &str,
    new_keys: &HashSet<MessageKey>,
) -> Vec<MessageContent> {
    let mut sorted: Vec<&Arc<ChannelRecord>> = records.iter().collect();
    sorted.sort_by(|a, b| (a.time.as_str(), a.sn).cmp(&(b.time.as_str(), b.sn)));
    let mut history = Vec::new();
    for record in sorted {
        if record.time.as_str() < cutoff {
            continue; // 时间窗外的记录不取（只取最后一段时间）
        }
        let mc = extract_content(record.user_name.clone(), &record.content, record.is_self);
        if mc.content.is_empty() {
            continue; // 非文本记录跳过
        }
        if record_message_key(record).map(|key| new_keys.contains(&key)).unwrap_or(false) {
            continue; // 与本批新消息重复：本批会进 pipeline，历史里去掉
        }
        history.push(mc);
    }
    history
}

/// 组装重置用的上下文：摘要块置首（并入首条 User，避免连续两条 User），其后为历史消息
fn compose_context(summaries: &[ArcStr], history: &[MessageContent]) -> Vec<Message> {
    let mut messages = pack_memory_messages(history);
    if summaries.is_empty() {
        return messages;
    }
    let block = format!(
        "【相关专案摘要】\n{}",
        summaries.iter().map(|s| format!("- {}", s)).collect::<Vec<_>>().join("\n")
    );
    if matches!(messages.first(), Some(Message::User { .. })) {
        if let Some(Message::User { content }) = messages.first_mut() {
            *content = ArcStr::from(format!("{}\n\n{}", block, content.as_str()));
        }
    } else {
        messages.insert(0, Message::User { content: ArcStr::from(block) });
    }
    messages
}

impl AbstractAgentInputProcessor {
    /// 生产构造：从进程级配置构造摘要记忆客户端，并启动 tick 任务（装配期在异步上下文中调用）
    pub fn new(session_key: Arc<SessionKey>, session: Arc<Session>) -> Self {
        Self::with_client(session_key, session, Arc::new(AbstractClient::new()))
    }

    /// 显式客户端构造：搭状态并启动 tick 任务
    pub fn with_client(session_key: Arc<SessionKey>, session: Arc<Session>, client: Arc<AbstractClient>) -> Self {
        let processor = Self::assemble(session_key, session, client);
        tokio::spawn(tick_loop(
            processor.session_key.clone(),
            processor.session.clone(),
            processor.client.clone(),
            processor.batches.clone(),
            processor.notify.clone(),
        ));
        processor
    }

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
            .or_insert_with(|| PendingBatch { cases: Vec::new(), last_at: now });
        batch.cases.push(event);
        batch.last_at = now;
    }
}

/// tick 任务：每秒检测一次，逐个触发已超时 channel 的批次（顺序执行 → 同会话上下文不并发写）；
/// 会话销毁（notify）时退出
async fn tick_loop(
    session_key: Arc<SessionKey>,
    session: Arc<Session>,
    client: Arc<AbstractClient>,
    batches: Arc<DashMap<ChannelKey, PendingBatch>>,
    notify: Arc<Notify>,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = notify.notified() => break,
            _ = ticker.tick() => {}
        }
        if batches.is_empty() {
            continue; // 空闲时不做配置查询
        }
        // 超时阈值取会话配置（与普通合批同一项配置）
        let timeout = Duration::from_secs(
            ConfigManager::get()
                .session_config::<ChannelBatchConfig, ChannelBatchConfig>(&session_key)
                .await
                .channel_batch_interval_secs,
        );
        for channel in due_channels(&batches, timeout) {
            let Some((_, batch)) = batches.remove(&channel) else { continue };
            flush_batch(&session_key, &session, &client, &channel, batch.cases).await;
        }
    }
}

/// 触发一个 channel 的批次：摘要记忆重建上下文（失败降级为不重置）→ 本批新消息进 pipeline
async fn flush_batch(
    session_key: &Arc<SessionKey>,
    session: &Arc<Session>,
    client: &Arc<AbstractClient>,
    channel: &ChannelKey,
    events: Vec<Arc<IncomingMessageEvent>>,
) {
    match build_context(session_key, client, &events).await {
        Ok(messages) => session.context_archive_and_clear_cache_and_reset_messages(messages).await,
        Err(e) => warn!(
            "摘要记忆重建上下文失败，降级为普通合批（channel={}:{}:{}）: {:?}",
            channel.messenger_id, channel.self_user_id, channel.group_id, e
        ),
    }
    let _ = Nexus::get().run_pipeline(session_key.as_ref(), pack_batch(&events)).await;
}

/// 用摘要记忆重建上下文：定位 case → 各 case 摘要 + case 结尾原文副本 → 时间窗与去重 → 组装消息
async fn build_context(
    session_key: &SessionKey,
    client: &Arc<AbstractClient>,
    events: &[Arc<IncomingMessageEvent>],
) -> Result<Vec<Message>> {
    let agent_id = session_key.agent_id.as_str();
    let role_event = session_key.role_event.as_ref();
    // 1. 定位本批消息涉及的 case
    let messages: Vec<LocateMessage> = events.iter().map(|e| locate_message(e.as_ref())).collect();
    let located = client.locate(agent_id, role_event, messages).await?;
    let cfg = ConfigManager::get()
        .session_config::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(session_key)
        .await;
    // 2. 逐个取 case 摘要与结尾若干条原文副本（都按 case 名查询，读取摘要不会重算摘要）；
    //    摘要为空的 case（历史遗留数据）跳过，不进上下文
    let mut summaries = Vec::new();
    let mut tails: Vec<Vec<Arc<CaseRecord>>> = Vec::new();
    for case in located.cases.iter() {
        let node = client.case_summary(agent_id, role_event, case.case_name.as_str()).await?;
        if node.summary.as_str().is_empty() {
            continue;
        }
        summaries.push(node.summary.clone());
        let tail = client
            .case_records_tail(agent_id, role_event, case.case_name.as_str(), cfg.memory_count as u64)
            .await?;
        tails.push(tail.records);
    }
    let records = merge_tail_records(&tails);
    // 4. 最后一段时间 + 与本批新消息去重
    let cutoff = (Local::now() - ChronoDuration::seconds(cfg.memory_time_secs as i64))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let new_keys: HashSet<MessageKey> = events.iter().filter_map(|e| new_message_key(e.as_ref())).collect();
    let history = filter_history(&records, &cutoff, &new_keys);
    // 5. 组装上下文（摘要在前，历史在后）
    Ok(compose_context(&summaries, &history))
}

impl AgentTrigger for AbstractAgentInputProcessor {}

#[async_trait]
impl AgentInputProcessor for AbstractAgentInputProcessor {
    async fn accept(&self, event: Arc<IncomingMessageEvent>) {
        self.enqueue(event);
    }
}

impl Drop for AbstractAgentInputProcessor {
    fn drop(&mut self) {
        // 会话销毁：通知 tick 任务退出（notify_one permit 语义：任务错过唤醒后下一轮 notified 立即完成）
        self.notify.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;
    use kissbot_api::RoleEvent;

    use crate::types::Error;

    /// 构造测试用上行消息事件（通道由 messenger / self_user / group 指定；此处「事件」指通道域的消息事件）
    fn ev_in(messenger: &str, self_user: &str, group: &str, name: &str, text: &str, time: &str) -> Arc<IncomingMessageEvent> {
        ev_with_id(messenger, self_user, group, name, text, time, "m")
    }

    fn ev_with_id(messenger: &str, self_user: &str, group: &str, name: &str, text: &str, time: &str, msg_id: &str) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: ArcStr::from(self_user),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: ArcStr::from(msg_id),
                messenger_id: ArcStr::from(messenger),
                user_id: ArcStr::from("u"),
                group_id: ArcStr::from(group),
                messenger_name: ArcStr::from(""),
                user_name: ArcStr::from(name),
                group_name: ArcStr::from(""),
                content: Content::Text(ArcStr::from(text)),
                time: ArcStr::from(time),
            }),
        })
    }

    fn ev(name: &str, text: &str, time: &str) -> Arc<IncomingMessageEvent> {
        ev_id(name, text, time, "m")
    }

    fn ev_id(name: &str, text: &str, time: &str, msg_id: &str) -> Arc<IncomingMessageEvent> {
        ev_with_id("web", "self", "g1", name, text, time, msg_id)
    }

    /// 构造测试用通道原文副本（msg_id 为空：不参与去重）
    fn rec(user: &str, text: &str, is_self: usize, time: &str, sn: u64) -> Arc<ChannelRecord> {
        rec_with_id(user, text, is_self, time, sn, "")
    }

    fn rec_with_id(user: &str, text: &str, is_self: usize, time: &str, sn: u64, msg_id: &str) -> Arc<ChannelRecord> {
        Arc::new(ChannelRecord {
            user_id: ArcStr::from(user),
            self_user_id: ArcStr::from("self"),
            messenger_id: ArcStr::from("web"),
            group_id: ArcStr::from("g1"),
            is_self,
            msg_id: ArcStr::from(msg_id),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::from(user),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from(text)),
            time: ArcStr::from(time),
            sn,
        })
    }

    fn mc(user_name: &str, text: &str, is_self: bool) -> MessageContent {
        MessageContent {
            user_name: ArcStr::from(user_name),
            content: vec![ArcStr::from(text)],
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
        let e = ev_id("u1", "a", "2026-09-23 10:00:00", "m7");
        let lm = locate_message(&e);
        assert_eq!(lm.self_user_id.as_str(), "self");
        assert_eq!(lm.user_name.as_str(), "u1");
        assert_eq!(lm.is_self, 0, "输入处理器只处理上行消息");
        assert_eq!(lm.msg_id.as_str(), "m7", "定位消息携带 msg_id 供与已存记录去重");
        assert_eq!(lm.channel(), ChannelKey::new("web", "self", "g1"));
    }

    #[test]
    fn new_message_key_uses_channel_and_msg_id() {
        let e = ev_id("u1", "hello", "2026-09-23 10:00:00", "m7");
        assert_eq!(new_message_key(&e), message_key("web", "self", "g1", "m7"));
        // 内容 / 时间 / 发送者都不参与去重键
        let other = ev_id("u2", "完全不同的内容", "2099-01-01 00:00:00", "m7");
        assert_eq!(new_message_key(&other), new_message_key(&e));
        // msg_id 为空不参与去重
        let empty = ev_id("u2", "x", "2099-01-01 00:00:00", "");
        assert_eq!(new_message_key(&empty), None);
        assert_eq!(message_key("web", "self", "g1", ""), None);
    }

    #[test]
    fn due_channels_only_returns_timed_out() {
        let batches = DashMap::new();
        batches.insert(
            ChannelKey::new("web", "self", "g1"),
            PendingBatch { cases: vec![], last_at: Instant::now() },
        );
        batches.insert(
            ChannelKey::new("web", "self", "g2"),
            PendingBatch { cases: vec![], last_at: Instant::now() - Duration::from_secs(10) },
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
        assert_eq!(processor.batches.get(&g1).unwrap().cases.len(), 2, "同 channel 合入一批");
        assert_eq!(processor.batches.get(&g2).unwrap().cases.len(), 1, "不同 channel 分开攒");
    }

    #[test]
    fn filter_history_windows_time_and_dedups_new_messages() {
        let records = vec![
            rec_with_id("u1", "old", 0, "2026-09-23 09:00:00", 1, "m-old"),   // 时间窗外
            rec_with_id("u1", "dup", 0, "2026-09-23 10:00:01", 2, "m-dup"),   // 与本批新消息同 msg_id：剔除
            rec_with_id("agent", "reply", 1, "2026-09-23 10:00:02", 3, ""),   // 本人回复：保留
            rec_with_id("u2", "new", 0, "2026-09-23 10:00:03", 4, "m-new"),
            rec_with_id("u1", "dup", 0, "2026-09-23 10:00:04", 5, ""),        // 内容同但 msg_id 为空：原样保留
        ];
        let new_keys = HashSet::from([message_key("web", "self", "g1", "m-dup").unwrap()]);
        let history = filter_history(&records, "2026-09-23 10:00:00", &new_keys);
        assert_eq!(history.len(), 3, "窗外 1 条与同 msg_id 的 1 条被剔除；msg_id 为空的记录保留");
        assert!(history[0].is_self, "本人回复保留");
        assert_eq!(history[0].content[0].as_str(), "reply");
        assert_eq!(history[1].content[0].as_str(), "new");
        assert_eq!(history[2].content[0].as_str(), "dup", "msg_id 为空时不去重");
    }

    #[test]
    fn merge_tail_records_keeps_channel_only_and_dedups_per_case() {
        // sn 在各 case 的记录文件里独立编号：case 内去重，跨 case 即使 (time, sn) 相同也各自保留
        let e1 = vec![
            Arc::new(CaseRecord::Channel(rec("u1", "a", 0, "2026-09-23 10:00:00", 1))),
            Arc::new(CaseRecord::Channel(rec("u1", "a", 0, "2026-09-23 10:00:00", 1))), // 同 case 内重复
            Arc::new(CaseRecord::Think(Arc::new(kissbot_api::ThinkRecord {
                reasoning_content: Some(ArcStr::from("推理")),
                thinking: None,
                key: ArcStr::from("k1"),
                time: ArcStr::from("2026-09-23 10:00:01"),
                sn: 1,
            }))),
        ];
        let e2 = vec![Arc::new(CaseRecord::Channel(rec("u2", "b", 0, "2026-09-23 10:00:00", 1)))];
        let merged = merge_tail_records(&[e1, e2]);
        assert_eq!(merged.len(), 2, "case 内去重、非通道记录跳过、跨 case 不去重");
        assert_eq!(merged[0].user_name.as_str(), "u1");
        assert_eq!(merged[1].user_name.as_str(), "u2");
    }

    #[test]
    fn compose_context_puts_summary_first_and_keeps_history() {
        let history = vec![mc("u1", "hi", false), mc("agent", "yo", true)];
        let messages = compose_context(&[ArcStr::from("专案A"), ArcStr::from("专案B")], &history);
        assert_eq!(messages.len(), 2, "摘要并入首条 User，不新增消息");
        match &messages[0] {
            Message::User { content } => {
                assert!(content.starts_with("【相关专案摘要】"), "摘要块置首: {}", content);
                assert!(content.contains("- 专案A") && content.contains("- 专案B"));
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
        let only_summary = compose_context(&[ArcStr::from("专案A")], &[]);
        assert_eq!(only_summary.len(), 1);
        assert!(matches!(&only_summary[0], Message::User { content } if content.contains("专案A")));
    }

    #[tokio::test]
    async fn accept_enqueues_through_trait_entry() {
        // accept（trait 入口）只入批：分桶与最后到达时间刷新见 enqueue
        let processor = test_processor();
        processor.accept(ev("u1", "a", "2026-09-23 10:00:00")).await;
        processor.accept(ev("u2", "b", "2026-09-23 10:00:01")).await;
        let g1 = ChannelKey::new("web", "self", "g1");
        assert_eq!(processor.batches.get(&g1).unwrap().cases.len(), 2);
    }

    #[tokio::test]
    async fn build_context_fails_without_abstract_service() {
        // 摘要记忆未配置：build_context 返回 Err（调用方据此降级为普通合批）
        let processor = test_processor();
        let err = build_context(
            &processor.session_key,
            &processor.client,
            &[ev("u1", "a", "2026-09-23 10:00:00")],
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::AbstractError(_)), "未配置摘要记忆应返回 AbstractError: {:?}", err);
    }

    fn test_processor() -> AbstractAgentInputProcessor {
        AbstractAgentInputProcessor::assemble(
            Arc::new(SessionKey { agent_id: ArcStr::from("a1"), role_event: Arc::new(RoleEvent::new("r1")) }),
            Arc::new(Session::new("", "test-session")),
            Arc::new(AbstractClient::with_url(String::new(), String::new())),
        )
    }
}
