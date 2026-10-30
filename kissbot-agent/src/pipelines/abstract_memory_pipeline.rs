//! 摘要记忆 pipeline：一个 pipeline 身份（agent + role_event）一条 pipeline，按 channel
//! （messenger + 接收方用户 + 群组）分别攒批；某 channel 的最后一条消息超过合批间隔即触发该批。
//!
//! 批次处理只做 locate 与本批上行记录的写入，随后把消息按 case 并入各自的待执行任务并立即返回，下一批
//! 可以紧接着处理；待执行任务由另一个队列的 worker 串行执行：每个 case 一次 run，每个 case 维护独立
//! session，run 开头用该 case 的「摘要 + 结尾原文副本」重置该 session 的上下文，再加上本次新增的消息。
//! 同一 case 至多一个待执行任务，排队等待执行期间又来的消息并入同一次 run；已开始执行的 run 不受影响，
//! 其后的消息形成下一轮。还没轮到的 case 的待执行任务不受其他 case 执行的影响，继续与后续批次的消息合并。
//!
//! 本批上行消息在 locate 之后才写入记忆，并带上所属 case 的 meta，摘要记忆组件下次归档时直接按 case
//! 名落位；模型回复也带当前 run 所属 case 的 meta，无需再次 locate。
//!
//! 本 pipeline 与 `default_memory` 共用 pipelines.rs 中的发送、工具调用、提示词等函数，
//! 但批次接入、locate、case 拆分与带 meta 的记忆写入都在本文件内完成。

use std::{collections::{HashMap, LinkedList, hash_map}, sync::{Arc, Weak}};

use arc_swap::ArcSwapOption;
use arcstr::ArcStr;
use async_trait::async_trait;
use dashmap::DashMap;
use kissbot_api::{
    ChannelKey, ChannelRecord, Content, IncomingMessageEvent, LocateMessage, MemoryMeta,
};
use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, Session, ToolConfig};
use tokio::sync::mpsc;
use tracing::warn;

use crate::{
    config_manager::ConfigManager, configs::{ChannelBatchConfig, EffectiveMemoryRecoverConfig, GroupSetConfig, MemoryRecoverConfig, OutChannelConfig}, message::{MessageContent, extract_content, pack_batch, pack_memory_messages}, nexus::Nexus, pipeline::{AgentPipeline, IncomingMessageBatchProcessor, IncomingMessageProcessor}, pipelines::{
        BatchProducer, MAX_TOOL_ROUNDS, bind_outgoing, call_tools_with_memory, command_parts, create_session, get_ego_system_prompt_or_default, import_session_config_from_agent_role, outgoing_channel, outgoing_content, parse_model_command, retrieve_assistant_message, retrieve_tool_messages, send_messages, send_outgoing, set_session_model_config, switch_model, tools_for_session, unbind_outgoing,
    }, types::{
        Error, MemoryKey, Result, build_think_record, channel_request_from_incoming,
    },
};

/// 摘要记忆 pipeline：每个 channel 一个批次生产者（DashMap 持有），channel 之间互不干扰；
/// 每个 case 维护独立 session（懒创建）。
pub struct AbstractMemoryPipeline {
    memory_key: Arc<MemoryKey>,
    /// case（None = locate 不可用的降级任务）→ session；session_id 由 case_name 推导
    sessions: DashMap<Option<ArcStr>, Arc<Session>>,
    /// /model 设置的模型覆盖（新建 case session 时应用）
    model_override: ArcSwapOption<(ArcStr, ArcStr)>,
    /// 各 channel 的批次生产者（按 ChannelKey 分桶）
    producers: DashMap<Arc<ChannelKey>, BatchProducer>,
    /// 批次队列：各 channel 消费者把批次投递到这里，由唯一 worker 串行 locate 并写入上行记录
    batch_tx: mpsc::UnboundedSender<Vec<Arc<IncomingMessageEvent>>>,
    /// 待执行任务表：case 名（None 表示 locate 不可用的降级任务）→ 尚未开始 run 的消息。
    /// 同一 case 的消息在这里合并，run 开始时整条取走；执行期间到达的消息形成下一轮
    pending: DashMap<Option<ArcStr>, Vec<Arc<IncomingMessageEvent>>>,
    /// case 队列：只投递 case 名（同一 case 至多一个），worker 按到达顺序取 case 执行
    case_tx: mpsc::UnboundedSender<Option<ArcStr>>,
}

impl AbstractMemoryPipeline {
    pub fn new(memory_key: Arc<MemoryKey>) -> Arc<Self> {
        let (batch_tx, batch_rx) = mpsc::unbounded_channel();
        let (case_tx, case_rx) = mpsc::unbounded_channel();
        let pipeline = Arc::new(Self {
            memory_key,
            sessions: DashMap::new(),
            model_override: ArcSwapOption::new(None),
            producers: DashMap::new(),
            batch_tx,
            pending: DashMap::new(),
            case_tx,
        });
        // worker 只持 Weak：pipeline 销毁 → sender 关闭 → 队列 worker 退出
        tokio::spawn(run_batch_queue(Arc::downgrade(&pipeline), batch_rx));
        tokio::spawn(run_case_queue(Arc::downgrade(&pipeline), case_rx));
        pipeline
    }

    /// case 的 session id：memory-abstract-{pipeline 基准 id}-{case_name}；None 用 -none。
    /// `-none` 同时作为 pipeline 入口配置与无 case 批次使用的基准 session。
    fn case_session_id(&self, case_name: Option<&ArcStr>) -> ArcStr {
        ArcStr::from(format!(
            "memory-abstract-{}-{}",
            self.memory_key.encode(),
            case_name.map(|name| name.as_str()).unwrap_or("none"),
        ))
    }

    /// 取（必要时创建）case 对应的 session；session_id 由 case_name 推导，不额外保存
    async fn ensure_session(&self, case_name: &Option<ArcStr>) -> Arc<Session> {
        if let Some(entry) = self.sessions.get(case_name) {
            return entry.value().clone();
        }
        if case_name.is_none() {
            return self.ensure_base_session().await;
        }

        // 先确保基准 session，进程重启后从它的已存配置恢复 /model 的 pipeline 级覆盖
        let _ = self.ensure_base_session().await;
        let session = self.create_case_session(case_name.clone()).await;
        let session_id = self.case_session_id(case_name.as_ref());
        if let Some(over) = self.model_override.load_full() {
            let _ = set_session_model_config(session_id.clone(), over.0.clone(), over.1.clone()).await;
        } else {
            let base_id = self.case_session_id(None);
            let cfg = ConfigManager::get();
            let base = cfg.session_config::<LLMConfig, EffectiveLLMConfig>(base_id).await;
            let case = cfg.session_config::<LLMConfig, EffectiveLLMConfig>(session_id.clone()).await;
            if base.provider != case.provider || base.model != case.model {
                let _ = set_session_model_config(session_id, base.provider.clone(), base.model.clone()).await;
            }
        }
        session
    }

    /// 基准（无 case）session：承担 pipeline 入口配置与无 case 批次
    async fn ensure_base_session(&self) -> Arc<Session> {
        if let Some(entry) = self.sessions.get(&None) {
            return entry.value().clone();
        }
        let base_session = self.create_case_session(None).await;
        if let Some(over) = self.model_override.load_full() {
            let base_id = self.case_session_id(None);
            let _ = set_session_model_config(base_id, over.0.clone(), over.1.clone()).await;
        }
        base_session
    }

    /// 创建并登记一个 case session（新建时应用当前系统提示词）
    async fn create_case_session(&self, case_name: Option<ArcStr>) -> Arc<Session> {
        let session_id = self.case_session_id(case_name.as_ref());
        // 从 agent role 配置导入
        import_session_config_from_agent_role::<LLMConfig, EffectiveLLMConfig>(&self.memory_key, &session_id).await;
        import_session_config_from_agent_role::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(&self.memory_key, &session_id).await;
        import_session_config_from_agent_role::<ChannelBatchConfig, ChannelBatchConfig>(&self.memory_key, &session_id).await;
        import_session_config_from_agent_role::<OutChannelConfig, OutChannelConfig>(&self.memory_key, &session_id).await;
        import_session_config_from_agent_role::<GroupSetConfig, GroupSetConfig>(&self.memory_key, &session_id).await;
        // 建立 session
        let session = create_session(session_id.clone()).await;
        // 先登记再应用提示词：/model 并发写入时能看到这个 session
        let session = self.sessions.entry(case_name)
            .or_insert(session)
            .value()
            .clone();
        let prompt = get_ego_system_prompt_or_default(self.memory_key.as_ref()).await;
        session.set_system_message(prompt).await;
        session
    }

    /// 入批：按 channel 找（或惰性创建）批次生产者；每个 channel 有自己的队列与消费者。
    ///
    /// 消费者只持 `Weak<Self>`：pipeline 销毁时强引用归零，`Drop` 清理生产者，消费者随 producer 退出。
    fn enqueue(self: Arc<Self>, event: Arc<IncomingMessageEvent>, interval: u64) {
        let channel = Arc::new(event_channel(&event));
        match self.producers.entry(channel) {
            dashmap::Entry::Occupied(entry) => entry.get().send(event, interval),
            dashmap::Entry::Vacant(entry) => {
                let (producer, mut consumer) = crate::pipelines::channel_batch_processor::<AbstractMemoryPipeline>();
                consumer.processor = Arc::downgrade(&self);
                tokio::spawn(consumer.run());
                producer.send(event, interval);
                entry.insert(producer);
            }
        }
    }

    /// 把一个 batch 的消息并入所属 case 的待执行任务：该 case 还没有待执行任务时同时投递 case 名，
    /// 已有（还没开始 run）时只合并消息，不再重复投递
    fn enqueue_case(&self, case_name: Option<ArcStr>, events: impl IntoIterator<Item = Arc<IncomingMessageEvent>>) {
        match self.pending.entry(case_name.clone()) {
            dashmap::Entry::Occupied(mut entry) => {
                entry.get_mut().extend(events);
            }
            dashmap::Entry::Vacant(entry) => {
                entry.insert(events.into_iter().collect());
                let _ = self.case_tx.send(case_name);
            }
        };
    }
}

#[async_trait]
impl IncomingMessageProcessor for AbstractMemoryPipeline {
    async fn incoming_message(self: Arc<Self>, event: Arc<IncomingMessageEvent>) {
        // 合批间隔与普通合批共用配置；入口阶段还没有 case，用基准（无 case）session 读取
        let base_id = self.case_session_id(None);
        let _ = self.ensure_session(&None).await;
        let interval = ConfigManager::get().session_config::<ChannelBatchConfig, ChannelBatchConfig>(base_id).await
            .channel_batch_interval_secs;
        self.enqueue(event, interval);
    }
}

#[async_trait]
impl IncomingMessageBatchProcessor for AbstractMemoryPipeline {
    async fn incoming_message_batch(self: Arc<Self>, events: Vec<Arc<IncomingMessageEvent>>) {
        if events.is_empty() {
            return;
        }
        // 批次投递到串行队列：由唯一 worker 按到达顺序 locate 并写入上行记录
        if self.batch_tx.send(events).is_err() {
            warn!("摘要记忆 pipeline 已销毁，丢弃本批消息");
        }
    }
}

/// 批次队列 worker：顺序 locate 各通道批次（同一会话的记忆写入只能串行），pipeline 销毁后队列清空即退出
async fn run_batch_queue(
    pipeline: Weak<AbstractMemoryPipeline>,
    mut rx: mpsc::UnboundedReceiver<Vec<Arc<IncomingMessageEvent>>>,
) {
    while let Some(events) = rx.recv().await {
        let Some(pipeline) = pipeline.upgrade() else { break };
        pipeline.process_batch(events).await;
    }
}

/// case 队列 worker：按到达顺序一次取一个 case，执行它当前全部待执行消息，完成后取下一个。
///
/// 取 case 时整条取走该 case 的待执行消息：执行期间到达的同 case 消息留在任务表里形成下一轮；
/// 还没轮到的 case 的任务表条目不动，继续与后续批次的消息合并。
async fn run_case_queue(pipeline: Weak<AbstractMemoryPipeline>, mut rx: mpsc::UnboundedReceiver<Option<ArcStr>>) {
    while let Some(case_name) = rx.recv().await {
        let Some(pipeline) = pipeline.upgrade() else { break };
        let Some((_, events)) = pipeline.pending.remove(&case_name) else { continue };
        pipeline.run_case(case_name, events).await;
    }
}

impl AbstractMemoryPipeline {
    /// 处理一个批次：locate → 写上行记录（带 case meta）→ 按消息所属 case 并入待执行任务，随即返回。
    ///
    /// case 的实际执行在 case 队列里串行进行，因此本批入队后即可处理下一批。
    async fn process_batch(&self, events: Vec<Arc<IncomingMessageEvent>>) {
        let nexus = Nexus::get();
        let client = nexus.abstract_client.clone();
        let channel = Arc::new(event_channel(&events[0]));
        let memory_key = self.memory_key.as_ref();
        let messages: Vec<LocateMessage> = events.iter().map(|event| locate_message(event.as_ref())).collect();

        // 1. 定位本批消息涉及的 case（摘要记忆未配置/调用失败时降级：整批作为一个无 case 任务）
        let mut case_map = HashMap::new();
        if let Ok(located) =
            client.locate(memory_key, channel, messages).await
        {
            for group in located.groups.into_iter() {
                for msg_id in group.messages.iter() {
                    case_map.insert(msg_id.clone(), group.case_name.clone());
                }
            }
        };

        // 2. 写入本批上行记录：定位成功的消息带上所属 case，摘要记忆组件下次归档时直接按 case 名落位
        let mut case_order = LinkedList::new();
        let mut case_msgs: HashMap<Option<ArcStr>, LinkedList<Arc<IncomingMessageEvent>>> = HashMap::new();
        for event in events.into_iter() {
            let case_name = case_map.get(&event.incoming_message.msg_id).cloned();
            // 带 meta 加入记忆
            let meta = case_name.as_ref().map(|n| Arc::new(MemoryMeta::with_case_name(n.clone())));
            let request = channel_request_from_incoming(memory_key, event.as_ref(), meta);
            nexus.memory_store_client.push_channel_record(request).await;
            // 加入 case 分析
            match case_msgs.entry(case_name) {
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().push_back(event);
                },
                hash_map::Entry::Vacant(entry) => {
                    let mut msgs = LinkedList::new();
                    msgs.push_back(event);
                    case_order.push_back(entry.key().clone());
                    entry.insert(msgs);
                },
            };
        }

        // 3. 按消息所属 case 入队：同 case 的消息合并为一个任务、一次 run，不同 case 各自 run
        for case_name in case_order.into_iter() {
            if let Some(msgs) = case_msgs.remove(&case_name) {
                self.enqueue_case(case_name, msgs);
            }
        }
    }

    /// 执行一个 case 任务：run 开头用该 case 的摘要与结尾原文副本重建该 case 的 session 上下文，
    /// 再运行 agentic loop；本次运行的出站记录带该 case 的 meta
    async fn run_case(&self, case_name: Option<ArcStr>, events: Vec<Arc<IncomingMessageEvent>>) {
        let client = Nexus::get().abstract_client.clone();
        let session_id = self.case_session_id(case_name.as_ref());
        let session = self.ensure_session(&case_name).await;
        let outgoing_meta = match &case_name {
            Some(case_name) => {
                // 上下文 = 该 case 的摘要 + 结尾 `memory_recover.memory_count` 条原文副本
                let rebuilt = async {
                    let cfg = ConfigManager::get().session_config::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(session_id.clone()).await;
                    let node = client.case_summary(self.memory_key.as_ref(), case_name.clone()).await?;
                    let tail = client.case_records_tail(self.memory_key.as_ref(), case_name.clone(), cfg.memory_count as u64).await?;
                    Ok::<_, Error>(compose_context(node.summary.as_str(), &record_contents(&tail.records)))
                }.await;
                match rebuilt {
                    Ok(messages) => {
                        let _ = session.context_archive_and_clear_cache_and_reset_messages(messages).await;
                    }
                    Err(e) => {
                        // 不沿用上一个 case 的上下文：清空后只用本次新增消息运行
                        warn!("摘要记忆上下文重建失败，清空上下文后继续运行（case={}）: {:?}", case_name, e);
                        let _ = session.context_archive_and_clear_cache_and_reset_messages(Vec::new()).await;
                    }
                }
                Some(Arc::new(MemoryMeta::with_case_name(case_name.clone())))
            }
            None => None,
        };
        let content = pack_batch(&events);
        let tools = tools_for_session(session_id.clone()).await;
        let _ = self.run(session_id, &session, content, &tools, outgoing_meta).await;
    }
}

#[async_trait]
impl AgentPipeline for AbstractMemoryPipeline {
    async fn reset_system_prompt(&self) {
        let sessions: Vec<Arc<Session>> = self.sessions.iter().map(|entry| entry.value().clone()).collect();
        if sessions.is_empty() {
            return;
        }
        let prompt = get_ego_system_prompt_or_default(self.memory_key.as_ref()).await;
        for session in sessions {
            session.set_system_message(prompt.clone()).await;
        }
    }

    async fn execute_command(&self, channel_id: ArcStr, content: ArcStr) -> Option<Result<ArcStr>> {
        let parts = command_parts(&content);
        match parts.first().map(|p| p.as_str()) {
            Some("/model") => Some(match parse_model_command(&parts) {
                Ok((provider, model)) => self.apply_model_command(provider, model).await,
                Err(e) => Err(e),
            }),
            // 摘要记忆的 pipeline 级配置（含 outgoing）存基准（无 case）session
            Some("/bind-outgoing") => Some(bind_outgoing(self.case_session_id(None), &channel_id, &parts).await),
            Some("/unbind-outgoing") => Some(unbind_outgoing(self.case_session_id(None), &parts).await),
            _ => None,
        }
    }

    /// channel 不再绑定本 pipeline：丢弃该 channel（messenger + 绑定用户）的全部分组生产者，待处理消息一并丢弃
    async fn channel_unbound(&self, messenger_id: ArcStr, self_user_id: ArcStr) {
        let keys: Vec<Arc<ChannelKey>> = self
            .producers
            .iter()
            .filter(|entry| {
                entry.key().messenger_id.as_str() == messenger_id.as_str()
                    && entry.key().self_user_id.as_str() == self_user_id.as_str()
            })
            .map(|entry| entry.key().clone())
            .collect();
        for key in keys {
            // 移除即销毁 producer：trigger channel 关闭，消费者退出；未到期的待处理消息随之丢弃
            self.producers.remove(&key);
        }
    }
}

impl AbstractMemoryPipeline {
    /// `/model`：校验后写入基准 session 与已有 case session，并保存覆盖供新建 case session 应用
    async fn apply_model_command(&self, provider: ArcStr, model: ArcStr) -> Result<ArcStr> {
        let _ = self.ensure_session(&None).await;
        let mut session_ids = vec![self.case_session_id(None)];
        session_ids.extend(self.sessions.iter().map(|entry| self.case_session_id(entry.key().as_ref())));
        session_ids.sort();
        session_ids.dedup();
        let reply = switch_model(session_ids, provider.clone(), model.clone()).await?;
        self.model_override.store(Some(Arc::new((provider, model))));
        Ok(reply)
    }

    async fn run_once(
        &self,
        session_id: ArcStr,
        session: &Session,
        round: usize,
        messages: Vec<Message>,
        tools: &Vec<Arc<ToolConfig>>,
        pending_contents: &mut Vec<Content>,
        outgoing_meta: Option<Arc<MemoryMeta>>,
    ) -> Result<bool> {
        // 摘要记忆的上下文已由 case 重建，不做超阈值判断
        let mut res = send_messages(
            session_id,
            session,
            messages,
            tools,
        ).await?;
        // 思考内容累积到本次 run（reasoning_content + thinking 双字段）；随最终回复的通道记录一起落盘
        if res.reasoning_content.is_some() || res.thinking.is_some() {
            let record = build_think_record(res.reasoning_content.clone(), res.thinking.clone());
            pending_contents.push(Content::Think(Arc::new(record)));
        }
        // 保存回复的消息
        let _ = session.context_append(vec![retrieve_assistant_message(&mut res)]).await;
        // 遍历执行 tool call
        let mut next = false;
        let errors = if let Some(tool_calls) = res.tool_calls.take() {
            let (tool_calls, tool_contents, errors) = call_tools_with_memory(tool_calls).await;
            pending_contents.extend(tool_contents);
            if !tool_calls.is_empty() {
                next = true;
                let tool_messages = retrieve_tool_messages(tool_calls, false);
                session.context_append(tool_messages).await?;
            }
            errors
        } else {
            vec![]
        };
        // 输出到 out_channel（pipeline 级配置存基准 session）：只有本轮有文本才发送，
        // 本轮及之前累积的思考/工具内容与文本一起作为下行记录写入
        let base_id = self.case_session_id(None);
        if let Some(out_channel) = outgoing_channel(&base_id).await {
            if let Some(content) = outgoing_content(pending_contents, res.content.take()) {
                send_outgoing(self.memory_key.as_ref(), out_channel.as_ref(), content, outgoing_meta.clone()).await;
            }
        }
        if !errors.is_empty() {
            return Err(Error::MultiError(errors));
        }
        // force end
        if round >= MAX_TOOL_ROUNDS {
            next = false;
        }
        Ok(next)
    }

    async fn run(&self, session_id: ArcStr, session: &Session, message: Message, tools: &Vec<Arc<ToolConfig>>, outgoing_meta: Option<Arc<MemoryMeta>>) -> Result<()> {
        let mut pending_contents = Vec::new();
        let mut round = 1 as usize;
        let mut next = self.run_once(session_id.clone(), session, round, vec![message], tools, &mut pending_contents, outgoing_meta.clone()).await?;
        while next {
            round = round + 1;
            next = self.run_once(session_id.clone(), session, round, vec![], tools, &mut pending_contents, outgoing_meta.clone()).await?;
        }
        Ok(())
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

/// 新消息 → 定位请求消息（通道由请求给出；本 pipeline 只处理上行消息，is_self 固定 0）
fn locate_message(event: &IncomingMessageEvent) -> LocateMessage {
    LocateMessage {
        msg_id: event.incoming_message.msg_id.clone(),
        time: event.incoming_message.time.clone(),
        user_id: event.incoming_message.user_id.clone(),
        user_name: event.incoming_message.user_name.clone(),
        is_self: 0,
        content: event.incoming_message.content.clone(),
    }
}

/// 原文副本 → 对话消息内容：通道记录即原文副本（思考/工具内容内嵌在 content 里），顺序与 client 返回一致
fn record_contents(records: &[Arc<ChannelRecord>]) -> Vec<MessageContent> {
    records
        .iter()
        .map(|record| extract_content(&record.content, record.user_name.clone(), record.is_self))
        .collect()
}

/// 组装一个 case 的运行上下文：摘要块置首（并入首条 User，避免连续两条 User），其后为原文副本消息
fn compose_context(summary: &str, history: &[MessageContent]) -> Vec<Message> {
    let mut messages = pack_memory_messages(history);
    if summary.is_empty() {
        return messages;
    }
    let block = format!("【相关专案摘要】\n- {}", summary);
    if matches!(messages.first(), Some(Message::User { .. })) {
        if let Some(Message::User { content }) = messages.first_mut() {
            *content = ArcStr::from(format!("{}\n\n{}", block, content.as_str()));
        }
    } else {
        messages.insert(0, Message::User { content: ArcStr::from(block) });
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;
    use kissbot_api::{ChannelRecord, RoleEvent};

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

    /// 构造测试用通道原文副本
    fn rec(user: &str, text: &str, is_self: usize, time: &str, sn: u64) -> Arc<ChannelRecord> {
        Arc::new(ChannelRecord {
            user_id: ArcStr::from(user),
            self_user_id: ArcStr::from("self"),
            messenger_id: ArcStr::from("web"),
            group_id: ArcStr::from("g1"),
            is_self,
            msg_id: ArcStr::new(),
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
        AbstractMemoryPipeline::new(
            Arc::new(MemoryKey {
                agent_id: ArcStr::from("a1"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            }),
        )
    }

    #[tokio::test]
    async fn case_session_ids_are_prefixed_and_case_scoped() {
        let p = pipeline();
        assert_eq!(p.case_session_id(Some(&ArcStr::from("role-weather"))).as_str(),
            "memory-abstract-a1-r1-role-weather");
        assert_eq!(p.case_session_id(None).as_str(), "memory-abstract-a1-r1-none");
    }

    #[test]
    fn event_channel_uses_recipient_user_and_group() {
        let e = ev("u1", "a", "2026-09-23 10:00:00");
        assert_eq!(event_channel(&e), ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1")));
    }

    #[test]
    fn locate_message_keeps_locator_fields() {
        let e = ev_with_id("web", "self", "g1", "u1", "a", "2026-09-23 10:00:00", "m7");
        let lm = locate_message(&e);
        assert_eq!(lm.user_name.as_str(), "u1");
        assert_eq!(lm.is_self, 0, "只处理上行消息");
        assert_eq!(lm.msg_id.as_str(), "m7");
        assert_eq!(lm.time.as_str(), "2026-09-23 10:00:00");
        assert_eq!(lm.content, Content::Text(ArcStr::from("a")));
    }

    #[tokio::test]
    async fn enqueue_case_merges_waiting_case_into_one_run() {
        // 用例内不 await：spawn 出来的 case worker 不会被执行（只要不影响待执行任务表的断言）
        let pipeline = pipeline();
        pipeline.enqueue_case(Some(ArcStr::from("role-a")), vec![ev("u1", "a1", "2026-09-23 10:00:00")]);
        pipeline.enqueue_case(Some(ArcStr::from("role-a")), vec![ev("u1", "a2", "2026-09-23 10:00:01")]);
        pipeline.enqueue_case(Some(ArcStr::from("role-b")), vec![ev("u1", "b1", "2026-09-23 10:00:02")]);
        assert_eq!(pipeline.pending.len(), 2, "同 case 只有一个待执行任务，不同 case 分开");
        let a = pipeline.pending.get(&Some(ArcStr::from("role-a"))).unwrap();
        assert_eq!(a.len(), 2, "排队中的同 case 消息合并为一次 run");
        drop(a);
        // worker 取 case 时只取走被取到的 case 的消息，其他 case 的任务表条目不动、继续等待合并
        let (_, b) = pipeline.pending.remove(&Some(ArcStr::from("role-b"))).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(pipeline.pending.len(), 1);
        assert!(pipeline.pending.contains_key(&Some(ArcStr::from("role-a"))), "其他 case 继续等待合并");
    }

    #[tokio::test]
    async fn enqueue_case_keeps_ungrouped_task_separate() {
        let pipeline = pipeline();
        pipeline.enqueue_case(None, vec![ev("u1", "a", "2026-09-23 10:00:00")]);
        pipeline.enqueue_case(None, vec![ev("u1", "b", "2026-09-23 10:00:01")]);
        pipeline.enqueue_case(Some(ArcStr::from("role-a")), vec![ev("u1", "c", "2026-09-23 10:00:02")]);
        assert_eq!(pipeline.pending.len(), 2, "无 case 的降级任务与 case 任务分开");
        assert_eq!(pipeline.pending.get(&None).unwrap().len(), 2, "降级任务同样按 key 合并");
    }

    #[test]
    fn record_contents_keeps_channel_records_in_returned_order() {
        let records = vec![
            rec("u1", "second", 0, "2026-09-23 10:00:05", 2),
            rec("agent", "first", 1, "2026-09-23 10:00:01", 1),
        ];
        let contents = record_contents(&records);
        assert_eq!(contents.len(), 2, "通道记录即原文副本，保持 client 返回的顺序");
        assert_eq!(contents[0].content[0].as_str(), "second", "保持 client 返回的顺序，不重排");
        assert_eq!(contents[1].content[0].as_str(), "first");
        assert!(contents[1].is_self);
    }

    #[test]
    fn compose_context_puts_summary_first_and_keeps_history() {
        let history = vec![mc("u1", "hi", false), mc("agent", "yo", true)];
        let messages = compose_context("专案A", &history);
        assert_eq!(messages.len(), 2, "摘要并入首条 User，不新增消息");
        match &messages[0] {
            Message::User { content } => {
                assert!(content.starts_with("【相关专案摘要】"), "摘要块置首: {}", content);
                assert!(content.contains("- 专案A"));
                assert!(content.ends_with("u1: hi"), "原首条 User 内容保留");
            }
            other => panic!("首条应为 User: {:?}", other),
        }
        assert!(matches!(&messages[1], Message::Assistant { .. }));
    }

    #[test]
    fn compose_context_without_history_or_summaries() {
        assert!(compose_context("", &[]).is_empty());
        let only_summary = compose_context("专案A", &[]);
        assert_eq!(only_summary.len(), 1);
        assert!(matches!(&only_summary[0], Message::User { content } if content.contains("专案A")));
        // 没有摘要时直接用原文副本消息
        let only_history = compose_context("", &[mc("u1", "hi", false)]);
        assert_eq!(only_history.len(), 2, "User + 补的空 Assistant");
        assert!(!matches!(&only_history[0], Message::User { content } if content.contains("专案摘要")));
    }

    #[tokio::test]
    async fn enqueue_creates_one_producer_per_channel() {
        let pipeline = pipeline();
        // 间隔取大值：用例内不触发 flush（flush 需要进程级 Nexus/配置单例）
        pipeline.clone().enqueue(ev("u1", "a", "2026-09-23 10:00:00"), 3600);
        pipeline.clone().enqueue(ev("u2", "b", "2026-09-23 10:00:01"), 3600);
        pipeline.clone().enqueue(ev_in("web", "self", "g2", "u3", "c", "2026-09-23 10:00:02"), 3600);
        assert_eq!(pipeline.producers.len(), 2, "同 channel 共用一个生产者，不同 channel 分开");
        let g1 = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"));
        let g2 = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g2"));
        assert!(pipeline.producers.contains_key(&g1));
        assert!(pipeline.producers.contains_key(&g2));
    }

    #[tokio::test]
    async fn channel_unbound_removes_all_groups_of_that_channel() {
        let pipeline = pipeline();
        pipeline.clone().enqueue(ev_in("web", "self1", "g1", "u1", "a", "2026-09-23 10:00:00"), 3600);
        pipeline.clone().enqueue(ev_in("web", "self1", "g2", "u1", "b", "2026-09-23 10:00:01"), 3600);
        pipeline.clone().enqueue(ev_in("web", "self2", "g1", "u2", "c", "2026-09-23 10:00:02"), 3600);
        assert_eq!(pipeline.producers.len(), 3);

        // 只清理 (web, self1) 下的全部 group，其他绑定用户不受影响
        pipeline.channel_unbound(ArcStr::from("web"), ArcStr::from("self1")).await;
        assert_eq!(pipeline.producers.len(), 1);
        assert!(pipeline.producers.contains_key(&ChannelKey::new(
            ArcStr::from("web"), ArcStr::from("self2"), ArcStr::from("g1"))));
    }

    #[tokio::test]
    async fn drop_releases_pipeline_without_consumer_cycle() {
        let pipeline = pipeline();
        pipeline.clone().enqueue(ev("u1", "a", "2026-09-23 10:00:00"), 3600);
        let weak = Arc::downgrade(&pipeline);
        drop(pipeline);
        // 批次消费者只持 Weak：最后一个强引用释放后 pipeline 立即销毁（不会形成引用环）
        assert!(weak.upgrade().is_none(), "pipeline 销毁后不应再被消费者强持有");
    }
}
