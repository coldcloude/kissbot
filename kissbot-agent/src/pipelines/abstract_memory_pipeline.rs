//! 摘要记忆 pipeline：一个会话一条 pipeline，按 channel（messenger + 接收方用户 + 群组）分别攒批；
//! 某 channel 的最后一条消息超过合批间隔即触发该批。
//!
//! 触发后先经摘要记忆定位 case（locate），再用「命中 case 的摘要 + case 结尾原文副本」重建会话上下文，
//! 最后把本批新消息交给 agentic loop。本批上行消息在 locate 之后才写入记忆，并带上所属 case 的 meta，
//! 摘要记忆组件下次归档时直接按 case 名落位；模型回复也带当前触发 case 的 meta，无需再次 locate。

use std::{
    collections::HashSet,
    sync::Arc,
};

use arcstr::ArcStr;
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Local};
use dashmap::{DashMap, Entry};
use kissbot_api::{
    CaseRecord, ChannelKey, ChannelRecord, IncomingMessageEvent, LocateMessage, LocateResponse,
    LocatedCaseRef, MemoryMeta,
};
use kissbot_llm::{Message, Session};
use tokio::sync::Mutex;
use tracing::warn;

use crate::{
    abstract_client::AbstractClient,
    config_manager::ConfigManager,
    configs::{ChannelBatchConfig, EffectiveMemoryRecoverConfig, MemoryRecoverConfig},
    message::{MessageContent, extract_content, pack_batch, pack_memory_messages},
    nexus::Nexus,
    pipeline::{AgentPipeline, IncomingMessageBatchProcessor, IncomingMessageProcessor, SystemPromptManager},
    pipelines::{
        AgenticLoop,
        channel_batch_processor::{ChannelBatchProducer, channel_batch_producer},
        reset_memory_system_prompt,
        tools_for_session,
    },
    types::{MemoryKey, Result, SessionKey, channel_request_from_incoming},
};

/// 某条消息在「通道三元组 + msg_id」下的去重键
type MessageKey = (ArcStr, ArcStr, ArcStr, ArcStr);

/// 摘要记忆 pipeline：每个 channel 一个批次生产者（DashMap 持有），channel 之间互不干扰。
pub struct AbstractMemoryPipeline {
    inner: Arc<AbstractMemoryPipelineInner>,
}

/// pipeline 的内部状态：批处理回调以 `Arc<Inner>` 交给各 channel 的消费者
struct AbstractMemoryPipelineInner {
    memory_key: Arc<MemoryKey>,
    /// 摘要记忆客户端（定位、case 摘要、case 原文尾部）
    client: Arc<AbstractClient>,
    /// agentic loop（与 default_memory 共用；本 pipeline 用压缩总结而非记忆回溯）
    agentic: Arc<AgenticLoop>,
    /// 各 channel 的批次生产者（按 ChannelKey 分桶）
    producers: DashMap<Arc<ChannelKey>, ChannelBatchProducer>,
    /// 同一会话的批次串行执行（不同 channel 的消费者可能并发触发，会话上下文只能串行改写）
    run_lock: Mutex<()>,
}

impl AbstractMemoryPipeline {
    /// 生产构造：从进程级配置构造摘要记忆客户端
    pub fn new(memory_key: Arc<MemoryKey>, session: Arc<Session>) -> Arc<Self> {
        Self::with_client(memory_key, session, Arc::new(AbstractClient::new()))
    }

    /// 显式客户端构造（单测注入空地址客户端，不触网）
    pub fn with_client(memory_key: Arc<MemoryKey>, session: Arc<Session>, client: Arc<AbstractClient>) -> Arc<Self> {
        let agentic = Arc::new(AgenticLoop::new(SessionKey::Memory(memory_key.clone()), session, false));
        Arc::new(Self {
            inner: Arc::new(AbstractMemoryPipelineInner {
                memory_key,
                client,
                agentic,
                producers: DashMap::new(),
                run_lock: Mutex::new(()),
            }),
        })
    }

    /// 入批：按 channel 找（或惰性创建）批次生产者；每个 channel 有自己的队列与消费者
    fn enqueue(&self, event: Arc<IncomingMessageEvent>, interval: u64) {
        let channel = Arc::new(event_channel(&event));
        match self.inner.producers.entry(channel.clone()) {
            Entry::Occupied(entry) => entry.get().send(event, interval),
            Entry::Vacant(entry) => {
                let (producer, mut consumer) = channel_batch_producer::<AbstractMemoryPipelineInner>(channel);
                // 弱引用：消费者任务不持有 pipeline，避免「pipeline → producer → 消费者 → pipeline」引用环
                consumer.processor = Some(Arc::downgrade(&self.inner));
                tokio::spawn(consumer.run());
                producer.send(event, interval);
                entry.insert(producer);
            }
        }
    }
}

#[async_trait]
impl SystemPromptManager for AbstractMemoryPipeline {
    async fn reset_system_prompt(&self) {
        reset_memory_system_prompt(self.inner.agentic.session_key(), self.inner.agentic.session()).await
    }
}

#[async_trait]
impl IncomingMessageProcessor for AbstractMemoryPipeline {
    async fn incoming_message(&self, event: Arc<IncomingMessageEvent>) {
        // 合批间隔与普通合批共用会话配置
        let interval = ConfigManager::get()
            .session_config::<ChannelBatchConfig, ChannelBatchConfig>(SessionKey::Memory(self.inner.memory_key.clone()))
            .await
            .channel_batch_interval_secs;
        self.enqueue(event, interval);
    }
}

#[async_trait]
impl AgentPipeline for AbstractMemoryPipeline {
    /// channel 不再绑定本会话：丢弃该 channel（messenger + 绑定用户）的全部分组生产者，待处理消息一并丢弃
    async fn channel_unbound(&self, messenger_id: ArcStr, self_user_id: ArcStr) {
        let keys: Vec<Arc<ChannelKey>> = self
            .inner
            .producers
            .iter()
            .filter(|entry| {
                let channel = entry.value().channel();
                channel.messenger_id.as_str() == messenger_id.as_str()
                    && channel.self_user_id.as_str() == self_user_id.as_str()
            })
            .map(|entry| entry.key().clone())
            .collect();
        for key in keys {
            if let Some((_, producer)) = self.inner.producers.remove(&key) {
                producer.shutdown();
            }
        }
    }
}

impl Drop for AbstractMemoryPipeline {
    fn drop(&mut self) {
        // pipeline 销毁：清理全部 channel 生产者，消费者任务随之退出（避免任务泄漏）
        let keys: Vec<Arc<ChannelKey>> = self.inner.producers.iter().map(|entry| entry.key().clone()).collect();
        for key in keys {
            if let Some((_, producer)) = self.inner.producers.remove(&key) {
                producer.shutdown();
            }
        }
    }
}

#[async_trait]
impl IncomingMessageBatchProcessor for AbstractMemoryPipelineInner {
    async fn incoming_message_batch(&self, events: Vec<Arc<IncomingMessageEvent>>) {
        if events.is_empty() {
            return;
        }
        // 同一会话串行：不同 channel 的批次可能同时到期，上下文重建与运行不能交错
        let _guard = self.run_lock.lock().await;
        let channel = event_channel(&events[0]);
        let memory_key = self.memory_key.as_ref();
        let messages: Vec<LocateMessage> = events.iter().map(|event| locate_message(event.as_ref())).collect();

        // 1. 定位本批消息涉及的 case（摘要记忆未配置/调用失败时降级：不重置上下文，消息不带 meta）
        let located = match self.client.locate(memory_key.agent_id.as_str(), memory_key.role_event.as_ref(), messages).await {
            Ok(located) => Some(located),
            Err(e) => {
                warn!(
                    "摘要记忆定位失败，降级为本批直接运行（channel={}:{}:{}）: {:?}",
                    channel.messenger_id, channel.self_user_id, channel.group_id, e
                );
                None
            }
        };

        // 2. 写入本批上行记录：定位成功的消息带上所属 case，摘要记忆组件下次归档时直接按 case 名落位
        let case_by_index = located.as_ref().map(case_by_message).unwrap_or_default();
        for (index, event) in events.iter().enumerate() {
            let meta = case_by_index
                .get(index)
                .cloned()
                .flatten()
                .map(|case_name| Arc::new(MemoryMeta::with_case_name(case_name)));
            let request = channel_request_from_incoming(memory_key, event.as_ref(), meta);
            Nexus::get().memory_store_client.push_channel_record(request).await;
        }

        // 3. 用命中的 case 摘要 + case 结尾原文副本重建上下文；出站回复带上当前触发 case
        match located {
            Some(located) => {
                let current_case = located.groups.last().map(|group| group.case_name.clone());
                self.agentic.set_outgoing_meta(
                    current_case.map(|case_name| Arc::new(MemoryMeta::with_case_name(case_name))),
                );
                match build_context(&self.memory_key, &self.client, &located.cases, &events).await {
                    Ok(messages) => {
                        let _ = self.agentic.session().context_archive_and_clear_cache_and_reset_messages(messages).await;
                    }
                    Err(e) => warn!("摘要记忆上下文重建失败，保留原上下文继续运行: {:?}", e),
                }
            }
            None => self.agentic.set_outgoing_meta(None),
        }

        // 4. 运行 agentic loop（本批新消息作为一条 user 消息）
        let content = pack_batch(&events);
        let tools = tools_for_session(self.agentic.session_key().clone()).await;
        let _ = self.agentic.run(content, &tools).await;
    }
}

/// 消息所属通道：messenger + 接收方用户（agent 在该通道绑定的身份）+ 群组；发送者 user_id 不参与
fn event_channel(event: &IncomingMessageEvent) -> ChannelKey {
    ChannelKey {
        messenger_id: event.incoming_message.messenger_id.clone(),
        self_user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
    }
}

/// 新消息 → 定位请求消息（本 pipeline 只处理上行消息，is_self 固定 0）
fn locate_message(event: &IncomingMessageEvent) -> LocateMessage {
    LocateMessage {
        msg_id: event.incoming_message.msg_id.clone(),
        time: event.incoming_message.time.clone(),
        user_name: event.incoming_message.user_name.clone(),
        is_self: 0,
        messenger_id: event.incoming_message.messenger_id.clone(),
        self_user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
        content: event.incoming_message.content.clone(),
    }
}

/// 定位分组 → 每条消息所属 case（下标与请求 messages 对齐；未被任何组覆盖的消息为 None）
fn case_by_message(located: &LocateResponse) -> Vec<Option<ArcStr>> {
    let total = located
        .groups
        .iter()
        .flat_map(|group| group.messages.iter().copied())
        .max()
        .map(|max| max + 1)
        .unwrap_or(0);
    let mut cases = vec![None; total];
    for group in &located.groups {
        for index in &group.messages {
            if let Some(slot) = cases.get_mut(*index) {
                *slot = Some(group.case_name.clone());
            }
        }
    }
    cases
}

/// 一条消息的去重键（通道三元组 + msg_id；msg_id 为空时返回 None，不参与去重）
fn message_key(messenger_id: &str, self_user_id: &str, group_id: &str, msg_id: &str) -> Option<MessageKey> {
    if msg_id.is_empty() {
        return None;
    }
    Some((
        ArcStr::from(messenger_id),
        ArcStr::from(self_user_id),
        ArcStr::from(group_id),
        ArcStr::from(msg_id),
    ))
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

/// 合并各 case 结尾的原文副本：只保留通道记录；各 case 内部按 (time, sn) 去重
/// （sn 在各 case 自己的记录文件里独立编号，跨 case 不可比）
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
/// 并与本批新消息按「通道三元组 + msg_id」去重
fn filter_history(records: &[Arc<ChannelRecord>], cutoff: &str, new_keys: &HashSet<MessageKey>) -> Vec<MessageContent> {
    let mut sorted: Vec<&Arc<ChannelRecord>> = records.iter().collect();
    sorted.sort_by(|a, b| (a.time.as_str(), a.sn).cmp(&(b.time.as_str(), b.sn)));
    let mut history = Vec::new();
    for record in sorted {
        if record.time.as_str() < cutoff {
            continue; // 时间窗外的记录不取
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
        summaries.iter().map(|summary| format!("- {}", summary)).collect::<Vec<_>>().join("\n")
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

/// 用摘要记忆重建上下文：命中 case 的摘要 + 各 case 结尾原文副本 → 时间窗与去重 → 组装消息
async fn build_context(
    memory_key: &Arc<MemoryKey>,
    client: &Arc<AbstractClient>,
    cases: &[Arc<LocatedCaseRef>],
    events: &[Arc<IncomingMessageEvent>],
) -> Result<Vec<Message>> {
    let agent_id = memory_key.agent_id.as_str();
    let role_event = memory_key.role_event.as_ref();
    let cfg = ConfigManager::get()
        .memory_session_config::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(memory_key.clone())
        .await;
    let mut summaries = Vec::new();
    let mut tails: Vec<Vec<Arc<CaseRecord>>> = Vec::new();
    for case in cases {
        let node = client.case_summary(agent_id, role_event, case.case_name.as_str()).await?;
        if node.summary.as_str().is_empty() {
            continue; // 摘要为空的历史 case 不进上下文
        }
        summaries.push(node.summary.clone());
        let tail = client
            .case_records_tail(agent_id, role_event, case.case_name.as_str(), cfg.memory_count as u64)
            .await?;
        tails.push(tail.records);
    }
    let records = merge_tail_records(&tails);
    let cutoff = (Local::now() - ChronoDuration::seconds(cfg.memory_time_secs as i64))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let new_keys: HashSet<MessageKey> = events.iter().filter_map(|event| new_message_key(event.as_ref())).collect();
    let history = filter_history(&records, &cutoff, &new_keys);
    Ok(compose_context(&summaries, &history))
}

#[cfg(test)]
mod tests {
    use super::*;

    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;
    use kissbot_api::{LocatedGroup, RoleEvent};

    use crate::types::Error;

    /// 构造测试用上行消息事件（通道由 messenger / self_user / group 指定）
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
        ev_in("web", "self", "g1", name, text, time)
    }

    /// 构造测试用通道原文副本（msg_id 为空：不参与去重）
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
            meta: None,
        })
    }

    fn mc(user_name: &str, text: &str, is_self: bool) -> MessageContent {
        MessageContent {
            user_name: ArcStr::from(user_name),
            content: vec![ArcStr::from(text)],
            is_self,
        }
    }

    fn pipeline() -> Arc<AbstractMemoryPipeline> {
        AbstractMemoryPipeline::with_client(
            Arc::new(MemoryKey {
                agent_id: ArcStr::from("a1"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            }),
            Arc::new(Session::new("", ArcStr::from("test-session"))),
            Arc::new(AbstractClient::with_url(ArcStr::new(), ArcStr::new())),
        )
    }

    #[test]
    fn event_channel_uses_recipient_user_and_group() {
        let e = ev("u1", "a", "2026-09-23 10:00:00");
        assert_eq!(event_channel(&e), ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1")));
    }

    #[test]
    fn locate_message_maps_recipient_to_self_user() {
        let e = ev_with_id("web", "self", "g1", "u1", "a", "2026-09-23 10:00:00", "m7");
        let lm = locate_message(&e);
        assert_eq!(lm.self_user_id.as_str(), "self");
        assert_eq!(lm.user_name.as_str(), "u1");
        assert_eq!(lm.is_self, 0, "只处理上行消息");
        assert_eq!(lm.msg_id.as_str(), "m7");
        assert_eq!(lm.channel(), ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1")));
    }

    #[test]
    fn case_by_message_aligns_request_indices() {
        let located = LocateResponse {
            cases: vec![],
            groups: vec![
                Arc::new(LocatedGroup { messages: vec![0, 2], case_name: ArcStr::from("role-a"), is_current: false }),
                Arc::new(LocatedGroup { messages: vec![1], case_name: ArcStr::from("role-b"), is_current: true }),
            ],
        };
        assert_eq!(
            case_by_message(&located),
            vec![Some(ArcStr::from("role-a")), Some(ArcStr::from("role-b")), Some(ArcStr::from("role-a"))]
        );
        assert!(case_by_message(&LocateResponse::empty()).is_empty(), "没有分组时没有 case");
    }

    #[test]
    fn new_message_key_uses_channel_and_msg_id() {
        let e = ev_with_id("web", "self", "g1", "u1", "hello", "2026-09-23 10:00:00", "m7");
        assert_eq!(new_message_key(&e), message_key("web", "self", "g1", "m7"));
        // 内容 / 时间 / 发送者都不参与去重键
        let other = ev_with_id("web", "self", "g1", "u2", "完全不同的内容", "2099-01-01 00:00:00", "m7");
        assert_eq!(new_message_key(&other), new_message_key(&e));
        let empty = ev_with_id("web", "self", "g1", "u2", "x", "2099-01-01 00:00:00", "");
        assert_eq!(new_message_key(&empty), None);
        assert_eq!(message_key("web", "self", "g1", ""), None);
    }

    #[test]
    fn filter_history_windows_time_and_dedups_new_messages() {
        let records = vec![
            rec_with_id("u1", "old", 0, "2026-09-23 09:00:00", 1, "m-old"),
            rec_with_id("u1", "dup", 0, "2026-09-23 10:00:01", 2, "m-dup"),
            rec_with_id("agent", "reply", 1, "2026-09-23 10:00:02", 3, ""),
            rec_with_id("u2", "new", 0, "2026-09-23 10:00:03", 4, "m-new"),
            rec_with_id("u1", "dup", 0, "2026-09-23 10:00:04", 5, ""),
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
        let e1 = vec![
            Arc::new(CaseRecord::Channel(rec_with_id("u1", "a", 0, "2026-09-23 10:00:00", 1, ""))),
            Arc::new(CaseRecord::Channel(rec_with_id("u1", "a", 0, "2026-09-23 10:00:00", 1, ""))),
            Arc::new(CaseRecord::Think(Arc::new(kissbot_api::ThinkRecord {
                reasoning_content: Some(ArcStr::from("推理")),
                thinking: None,
                key: ArcStr::from("k1"),
                time: ArcStr::from("2026-09-23 10:00:01"),
                sn: 1,
            }))),
        ];
        let e2 = vec![Arc::new(CaseRecord::Channel(rec_with_id("u2", "b", 0, "2026-09-23 10:00:00", 1, "")))];
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
        assert!(compose_context(&[], &[]).is_empty());
        let only_summary = compose_context(&[ArcStr::from("专案A")], &[]);
        assert_eq!(only_summary.len(), 1);
        assert!(matches!(&only_summary[0], Message::User { content } if content.contains("专案A")));
    }

    #[tokio::test]
    async fn enqueue_creates_one_producer_per_channel() {
        let pipeline = pipeline();
        // 间隔取大值：用例内不触发 flush（flush 需要进程级 Nexus/配置单例）
        pipeline.enqueue(ev("u1", "a", "2026-09-23 10:00:00"), 3600);
        pipeline.enqueue(ev("u2", "b", "2026-09-23 10:00:01"), 3600);
        pipeline.enqueue(ev_in("web", "self", "g2", "u3", "c", "2026-09-23 10:00:02"), 3600);
        assert_eq!(pipeline.inner.producers.len(), 2, "同 channel 共用一个生产者，不同 channel 分开");
        let g1 = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"));
        let g2 = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g2"));
        assert!(pipeline.inner.producers.contains_key(&g1));
        assert!(pipeline.inner.producers.contains_key(&g2));
    }

    #[tokio::test]
    async fn channel_unbound_removes_all_groups_of_that_channel() {
        let pipeline = pipeline();
        pipeline.enqueue(ev_in("web", "self1", "g1", "u1", "a", "2026-09-23 10:00:00"), 3600);
        pipeline.enqueue(ev_in("web", "self1", "g2", "u1", "b", "2026-09-23 10:00:01"), 3600);
        pipeline.enqueue(ev_in("web", "self2", "g1", "u2", "c", "2026-09-23 10:00:02"), 3600);
        assert_eq!(pipeline.inner.producers.len(), 3);

        // 只清理 (web, self1) 下的全部 group，其他绑定用户不受影响
        pipeline.channel_unbound(ArcStr::from("web"), ArcStr::from("self1")).await;
        assert_eq!(pipeline.inner.producers.len(), 1);
        assert!(pipeline.inner.producers.contains_key(&ChannelKey::new(
            ArcStr::from("web"), ArcStr::from("self2"), ArcStr::from("g1"))));
    }

    #[tokio::test]
    async fn drop_clears_all_producers() {
        let pipeline = pipeline();
        pipeline.enqueue(ev("u1", "a", "2026-09-23 10:00:00"), 3600);
        let inner = pipeline.inner.clone();
        assert_eq!(inner.producers.len(), 1);
        drop(pipeline);
        assert!(inner.producers.is_empty(), "pipeline 销毁时清理全部 channel 生产者");
    }

    #[tokio::test]
    async fn build_context_fails_without_abstract_service() {
        let client = Arc::new(AbstractClient::with_url(ArcStr::new(), ArcStr::new()));
        let memory_key = Arc::new(MemoryKey {
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
        });
        let err = build_context(
            &memory_key,
            &client,
            &[Arc::new(LocatedCaseRef { case_name: ArcStr::from("role-a"), is_current: true })],
            &[ev("u1", "a", "2026-09-23 10:00:00")],
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::AbstractError(_)), "未配置摘要记忆应返回 AbstractError: {:?}", err);
    }
}
