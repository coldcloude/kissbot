pub mod abstract_memory_pipeline;
pub mod channel_batch_processor;
pub mod token_reducer;

pub use abstract_memory_pipeline::*;
pub use channel_batch_processor::*;
use dashmap::{DashMap, Entry};
use serde_json::Value;
pub use token_reducer::*;

use async_trait::async_trait;
use futures_util::future;
use kissbot_api::{Content, ContentBlock, IncomingMessageEvent, MemoryMeta, OutgoingMessage, RESERVED_AGENT_ID, ResourceContents};
use tokio::spawn;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arcstr::ArcStr;
use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, ModelResponse, Session, ToolCall, ToolConfig};
use tracing::warn;

use crate::configs::{ChannelBatchConfig, CompressConfig, EffectiveCompressConfig, GroupSetConfig, OutChannel, PipelineConfig};
use crate::memory_store_client::QueryChannelData;
use crate::message::{extract_content, pack_batch};
use crate::pipeline::{AgentPipeline, IncomingMessageBatchProcessor, IncomingMessageProcessor, TokenReducer};
use crate::station::tool_config_from_mcp;
use crate::types::build_memory_think;
use crate::{config_manager::ConfigManager, configs::{EffectiveMemoryRecoverConfig, MemoryRecoverConfig}, message::{MessageContent, pack_memory_messages}, nexus::Nexus, station::Station, types::{Error, MemoryKey, Result, SessionKey, build_memory_tool_call, build_memory_tool_result, channel_request_from_incoming, channel_request_from_outgoing}};

/// 默认系统提示词
pub const DEFAULT_SYSTEM_PROMPT: ArcStr = arcstr::literal!("你是一个通用智能体。使用工具完成用户任务");

/// 根据 agent_id 获取系统提示词（新建会话时由 MemoryEgoSystemPrompter 调用）：
/// 保留 agent（agent_id="0"）返回 None，由 prompter 回退默认提示词；其余走 ego REST（agent 元数据 + 个体识别 + 角色设定，
/// 失败静默跳过，全部失败回退默认提示词"你是 kissbot 智能助手"）；
/// 通过 ego_md 模块将 ego 结构转为 markdown，替代手写提示词片段
pub async fn get_ego_system_prompt(agent_id: ArcStr, role_name: ArcStr) -> Option<ArcStr> {
    if agent_id == RESERVED_AGENT_ID {
        return None;
    }

    let mut system_parts = vec![];

    let nexus = Nexus::get();

    // 1. agent 元数据（按 agent_id 查询）-> 身份 markdown
    if let Ok(Some(metadata)) = nexus.memory_ego_client.get_agent(agent_id.clone()).await {
        system_parts.push(crate::ego_md::build_ego_identity_md(&metadata));
    }
    // 2. 个体识别（按 agent_id 查询）-> 个体识别 markdown，并收集匹配个体名
    if let Ok(Some(individuals)) = nexus.memory_ego_client.get_individuals(agent_id.clone()).await {
        system_parts.push(crate::ego_md::build_ego_individual_recognition_md(&individuals, None));
    }
    // 3. 角色设定（按 agent_id + role_name 查询）-> 角色 markdown
    if !role_name.is_empty() {
        if let Ok(Some(role)) = nexus.memory_ego_client.get_role(agent_id, role_name).await {
            system_parts.push(crate::ego_md::build_role_play_md(&role, None));
        }
    }

    if system_parts.is_empty() {
        return None;
    }

    Some(ArcStr::from(system_parts.join("\n")))
}

/// 解析 query 响应分组 → 写入 BTreeMap：(time, sn) 为唯一键（sn 为同文件内唯一序号），
/// 同键重复记录只保留第一条（两查询共用 map → 并集自动去重，迭代天然按 (time, sn) 升序）
/// 文本提取走 extract_content（递归收集全部 Text 段）
fn parse_channel_groups(groups: QueryChannelData, map: &mut BTreeMap<(ArcStr, u64), MessageContent>) {
    for (_, records) in groups {
        for (_, rec) in records {
            let time = rec.time.clone();  // 键用（MessageContent 值不保留 time）
            let sn = rec.sn;
            let key = (time, sn);
            // 同 (time, sn) 已存在（两查询并集在 ln 处重叠）→ 提前跳过，免去 user_name clone 与文本提取
            if map.contains_key(&key) {
                continue;
            }
            map.insert(key, extract_content(rec.user_name, &rec.content, rec.is_self));
        }
    }
}

/// 记忆打包（两次查询并集）→ 交替的 User/Assistant 消息：
/// ① RecentQuery 最近 N 条（无时间参数）→ ln = 最旧一条 time；
/// ② QueryRequest 时间范围 [M, ln]（M = min(时间窗起点, ln)，无 limit，M == ln 时退化为单点取 ln 同时间组）；
/// 结果 = ① ∪ ②（两次解析共用同一 BTreeMap，键 (time, sn) → 去重 + 天然时间正序），升序打包。
/// 任一步失败（时间窗计算 / 查询）均返回空上下文（不阻塞消息发送）。
pub async fn build_context_from_memory_store(memory_key: Arc<MemoryKey>) -> Vec<Message> {
    let cfg = ConfigManager::get().memory_session_config::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(memory_key.clone()).await;
    let nexus = Nexus::get();

    // 时间窗起点计算失败（checked_sub_signed 溢出）→ 空上下文，不静默回退到错误的时间窗
    let start = match chrono::Local::now()
        .checked_sub_signed(chrono::Duration::seconds(cfg.memory_time_secs as i64))
        .map(|t| ArcStr::from(t.format("%Y-%m-%d %H:%M:%S").to_string()))
    {
        Some(start) => start,
        None => return Vec::new(),
    };

    let count = cfg.memory_count;

    // ===== 解析：BTreeMap 以 (time, sn) 为键同时做去重与排序（迭代天然升序） =====
    // 两次查询共用同一 map → 并集自动去重（Query2 的 [M, ln] 区间与 Query1 尾部在 ln 处重叠）
    let mut map: BTreeMap<(ArcStr, u64), MessageContent> = BTreeMap::new();

    // ===== Query1：POST {store}/store/query/channel/recent（RecentQuery，无时间参数） =====
    let groups = match nexus.memory_store_client.query_last(memory_key.agent_id.clone(), memory_key.role_event.clone(), count as u32).await {
        Ok(groups) => groups,
        Err(_) => return Vec::new(),
    };
    parse_channel_groups(groups, &mut map);

    // ln = 已解析记录最旧一条的 time（BTreeMap 首键）；map 为空说明无记录（count=0 时 Query1 必为空）
    // 一并在此处理；Query1 最旧若为非文本，其同时间组除非文本外无其他记录（否则文本记录会占据更小首键）
    let Some((ln, _)) = map.keys().next() else {
        return Vec::new();
    };
    let ln = ln.clone();
    // 不足 N 条：直接返回（无需 Query2）。理论上去重后记录不重复，map 大小即取到的数量；
    // 注意：以解析后文本数（map.len()）判定——非文本记录在最后 N 内被跳过时也会提前返回
    if map.len() < count {
        return pack_memory_messages(&map.into_values().collect::<Vec<_>>());
    }
    // M = min(时间窗起点 start, ln)
    let m = if start.as_str() < ln.as_str() { start } else { ln.clone() };  // M = min(cutoff, ln)

    // ===== Query2：POST {store}/store/query/channel（QueryRequest 时间范围 [M, ln]，无 limit） =====
    // M == ln 时退化为单点 [ln, ln]（取 ln 同时间组）；与 Query1 并集（共用 map 去重）
    let groups = match nexus.memory_store_client.query_recent(memory_key.agent_id.clone(), memory_key.role_event.clone(), m, ln).await {
        Ok(groups) => groups,
        Err(_) => return Vec::new(),
    };
    parse_channel_groups(groups, &mut map);

    pack_memory_messages(&map.into_values().collect::<Vec<_>>())
}

pub async fn send_messages(session_key: SessionKey, session: &Session, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse> {
    let _ = session.context_append(messages).await;
    let full_messages = session.build_context().await;
    let llm_cfg = ConfigManager::get().session_config::<LLMConfig, EffectiveLLMConfig>(session_key).await;
    Nexus::get().provider_manager.call(llm_cfg.as_ref(), full_messages, tools).await
}

async fn send_messages_with_limit(session_key: SessionKey, session: &Session, mut messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>, reducer: &impl TokenReducer, tokens_usage: u64) -> Result<ModelResponse> {
    let cfg_mngr = ConfigManager::get();
    let llm_cfg = cfg_mngr.session_config::<LLMConfig, EffectiveLLMConfig>(session_key.clone()).await;
    let compress_cfg = cfg_mngr.session_config::<CompressConfig, EffectiveCompressConfig>(session_key.clone()).await;
    let provider = llm_cfg.provider.as_str();
    let model = llm_cfg.model.as_str();
    let model_cfg = cfg_mngr.provider_model_config(provider, model).await
    .ok_or_else(|| Error::ModelProviderNotFound(ArcStr::from(provider), ArcStr::from(model)))?;

    // 1. 检查上下文 token 占用超限
    //    阈值来自会话模型的 max_tokens_usage
    //    上次模型响应的 usage.total_tokens 超过阈值触发
    //    无新消息不触发
    if model_cfg.max_tokens_usage > 0 {
        let max_tokens_usage = ((model_cfg.max_tokens_usage as f64) * compress_cfg.compress_threshold) as u64;
        if tokens_usage > max_tokens_usage {
            // 压缩前回退至最后一条 assistant 消息
            let mut pending_messages = session.context_revert_until(&mut |msg| {
                match msg {
                    Message::Assistant { .. } => true,
                    _ => false,
                }
            }).await?;
            pending_messages.extend(messages);
            (_, messages) = reducer.reduce(session_key, session, pending_messages).await;
        }
    }
    // 2. 正常的发送流程
    let _ = session.context_append(messages).await;
    let full_messages = session.build_context().await;
    Nexus::get().provider_manager.call(llm_cfg.as_ref(), full_messages, tools).await
}

pub fn retrieve_assistant_message(res: &mut ModelResponse) -> Message {
    Message::Assistant {
        content: res.raw_content.take(),
        reasoning_content: res.raw_reasoning_content.take(),
        tool_calls: res.raw_tool_calls.take(),
    }
}

/// 会话可用工具：会话配置的启用 group 白名单 → Station 平铺查询（本级 + 直接子递归）并按 group 本地过滤
/// 结果转换为 LLM 侧工具定义；聚合为空则请求不携带 tools 字段（兼容无工具场景）
pub async fn tools_for_session(session_key: SessionKey) -> Vec<Arc<ToolConfig>> {
    let cfg = ConfigManager::get().session_config::<GroupSetConfig, GroupSetConfig>(session_key).await;
    if cfg.group_set.is_empty() {
        return Vec::new();
    }
    match Station::get().list_tools_local(Some(cfg.group_set.as_ref())).await {
        Ok(tools) => tools
            .iter()
            .map(|t| Arc::new(tool_config_from_mcp(t.as_ref())))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Agentic Loop 产出回复：发到 out_channel（agent_id/role_name 定位、role_event 记忆编码），
/// 成功后把下行消息写入记忆（is_self=1）；`meta` 由调用方给出（摘要记忆 pipeline 写入当前 case）。
///
/// 发送前校验 out_channel 身份在目标 channel 仍绑定；未绑定 → 清理该 (agent, role) 的 out 配置并跳过。
pub async fn send_outgoing(
    memory_key: &MemoryKey,
    out_channel: &OutChannel,
    content: Content,
    meta: Option<Arc<MemoryMeta>>,
) {
    let nexus = Nexus::get();
    let agent_id = memory_key.agent_id.clone();
    let role_name = memory_key.role_event.role_name.clone();
    // 1. 校验 out_channel 身份在目标 channel 仍绑定（未绑定 = 配置悬空，清理并跳过发送）
    let bound = ConfigManager::get().channel(out_channel.channel_id.as_str()).await
        .map(|c| c.bind_users.contains(&out_channel.user))
        .unwrap_or(false);
    if !bound {
        warn!("send_outgoing: out_channel 身份未绑定，清理 {}/{} 的回复通道", agent_id, role_name);
        let _ = ConfigManager::get().set_out_channel(agent_id, role_name, None).await;
        return;
    }
    // 2. 发送
    let msg = OutgoingMessage {
        messenger_id: out_channel.user.messenger_id.clone(),
        user_id: out_channel.user.user_id.clone(),
        group_id: out_channel.group_id.clone(),
        content,
    };
    match nexus.channel_manager.send(out_channel.channel_id.as_str(), msg).await {
        Ok(response) => {
            // 3. 下行成功后写记忆（is_self=1）
            let request = channel_request_from_outgoing(memory_key, out_channel, response.as_ref(), meta);
            nexus.memory_store_client.push_channel_record(request).await;
        }
        Err(e) => {
            warn!("send_outgoing 失败: {:?}", e);
        }
    }
}

pub async fn call_tool(mut tool_call: ToolCall) -> (ToolCall, Option<Error>) {
    let arguments = tool_call.arguments.take();
    let (arguments_recover, result) = Station::get().call_tool_local(tool_call.name.clone(), arguments).await;
    tool_call.result_arguments = Some(Arc::new(arguments_recover));
    match result {
        Ok(r) => {
            tool_call.result = Some(Arc::new(r.content));
            (tool_call, None)
        },
        Err(e) => {
            (tool_call, Some(e))
        }
    }
}

pub async fn call_tool_with_memory(tool_call: ToolCall, memory_key: &MemoryKey) -> (ArcStr, ToolCall, Option<Error>) {
    let nexus = Nexus::get();
    // 1. 工具调用 key：UUID（ToolCall/ToolResult 详情与 channel 占位同 key 关联）
    let tool_key = ArcStr::from(uuid::Uuid::new_v4().to_string());
    // 2. 记忆写入：ToolCallRequest.key 与 ToolResultRequest.key 用同一 key
    let tool_call_request = build_memory_tool_call(memory_key, tool_key.clone(), &tool_call);
    nexus.memory_store_client.push_tool_call(tool_call_request).await;
    // 3. 执行工具
    let result = call_tool(tool_call).await;
    // 4. 成功时记忆写入：ToolCallRequest.key 与 ToolResultRequest.key 用同一 key
    if result.1.is_none() {
        let tool_result_request = build_memory_tool_result(memory_key, tool_key.clone(), &result.0);
        nexus.memory_store_client.push_tool_result(tool_result_request).await;
    }
    (tool_key, result.0, result.1)
}

pub async fn call_tools(tool_calls: Vec<ToolCall>) -> (Vec<ToolCall>, Vec<Error>) {
    let futs: Vec<_> = tool_calls.into_iter().map(call_tool).collect();
    let results = future::join_all(futs).await;
    let mut errors = Vec::new();
    let mut tool_calls = Vec::with_capacity(results.len());
    for (tool_call, error) in results.into_iter() {
        if let Some(error) = error {
            errors.push(error);
        }
        tool_calls.push(tool_call);
    }
    (tool_calls, errors)
}

/// 并发执行带记忆写入的 tool calls；全部完成后按返回顺序把工具 key 写入本次 run 的 pending_contents。
///
/// 工具调用本身并发执行；pending 内容统一在 join_all 完成后更新，避免并发写入。
pub async fn call_tools_with_memory(
    tool_calls: Vec<ToolCall>,
    memory_key: &MemoryKey,
    pending_contents: &mut Vec<Content>,
) -> (Vec<ToolCall>, Vec<Error>) {
    let results: Vec<(ArcStr, ToolCall, Option<Error>)> = future::join_all(
        tool_calls.into_iter().map(|tool_call| call_tool_with_memory(tool_call, memory_key))
    ).await;
    let mut tool_calls = Vec::with_capacity(results.len());
    let mut errors = Vec::new();
    for (key, tool_call, error) in results {
        pending_contents.push(Content::ToolCall(key.clone()));
        if let Some(error) = error {
            errors.push(error);
        } else {
            pending_contents.push(Content::ToolResult(key));
        }
        tool_calls.push(tool_call);
    }
    (tool_calls, errors)
}

pub fn retrieve_tool_messages(tool_calls: Vec<ToolCall>, multi: bool) -> Vec<Message> {
    let mut tool_messages = Vec::new();
    for mut tool_call in tool_calls.into_iter() {
        if let Some(mut result) = tool_call.result.take() {
            let content = if multi {
                let cs = Arc::make_mut(&mut result);
                for mut block in cs.iter_mut() {
                    match &mut block {
                        ContentBlock::Text { annotations, meta, .. } => {
                            *annotations = None;
                            *meta = Value::Null;
                        },
                        ContentBlock::Image { annotations, meta, .. } => {
                            *annotations = None;
                            *meta = Value::Null;
                        },
                        ContentBlock::Audio { annotations, meta, .. } => {
                            *annotations = None;
                            *meta = Value::Null;
                        },
                        ContentBlock::ResourceLink { annotations, meta, .. } => {
                            *annotations = None;
                            *meta = Value::Null;
                        },
                        ContentBlock::Resource { resource, annotations, meta } => {
                            *annotations = None;
                            *meta = Value::Null;
                            let res = Arc::make_mut(resource);
                            match res {
                                ResourceContents::Text { meta, .. } => {
                                    *meta = Value::Null;
                                },
                                ResourceContents::Blob { meta, .. } => {
                                    *meta = Value::Null;
                                },
                            };
                        },
                    }
                }
                serde_json::to_value(result).unwrap_or(Value::Null)
            } else {
                let content = result.iter()
                .map(|block| match block {
                    ContentBlock::Text { text, .. } => text.clone(),
                    ContentBlock::Image { mime_type, .. } => ArcStr::from(format!("[image {}]", mime_type)),
                    ContentBlock::Audio { mime_type, .. } => ArcStr::from(format!("[image {}]", mime_type)),
                    ContentBlock::ResourceLink { uri, name, .. } => ArcStr::from(format!("[resource_link {} {}]", name, uri)),
                    ContentBlock::Resource { resource, .. } => match resource.as_ref() {
                        ResourceContents::Text { text, .. } => text.clone(),
                        ResourceContents::Blob { uri, mime_type, .. } => ArcStr::from(format!("[resource {} {}]", uri, mime_type.as_deref().unwrap_or_default())),
                    },
                })
                .collect::<Vec<_>>()
                .join("\n");
                Value::String(content)
            };
            tool_messages.push(Message::Tool {
                tool_call_id: tool_call.id,
                content
            });
        }
    }
    tool_messages
}

/// Agentic Loop 工具调用轮次上限（防死循环）
const MAX_TOOL_ROUNDS: usize = 10;

pub async fn run_once(session_key: SessionKey, session: &Session, round: usize, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>, tokens_usage: Option<u64>) -> Result<(bool,u64)> {
    let mut res = if let Some(tokens_usage) = tokens_usage {
        send_messages_with_limit(session_key.clone(), session, messages, tools, &CompressTokenReducer, tokens_usage).await?
    } else {
        send_messages(session_key.clone(), session, messages, tools).await?
    };
    // 保存回复的消息
    let _ = session.context_append(vec![retrieve_assistant_message(&mut res)]).await;
    // 遍历执行 tool call
    let mut next = false;
    let errors = if let Some(tool_calls) = res.tool_calls.take() {
        let (tool_calls, errors) = call_tools(tool_calls).await;
        if !tool_calls.is_empty() {
            next = true;
            let tool_messages = retrieve_tool_messages(tool_calls, false);
            session.context_append(tool_messages).await?;
        }
        errors
    } else {
        vec![]
    };
    if !errors.is_empty() {
        return Err(Error::MultiError(errors));
    }
    // force end
    if round >= MAX_TOOL_ROUNDS {
        next = false;
    }
    Ok((next, res.total_tokens))
}

pub async fn run(session_key: SessionKey, session: &Session, message: Message, tools: &Vec<Arc<ToolConfig>>, mut tokens_usage: Option<u64>) -> Result<u64> {
    let mut round = 1 as usize;
    let (mut next, mut total_tokens) = run_once(session_key.clone(), session, round, vec![message], tools, tokens_usage).await?;
    while next {
        round = round + 1;
        if tokens_usage.is_some() {
            tokens_usage = Some(total_tokens);
        }
        (next, total_tokens) = run_once(session_key.clone(), session, round, vec![], tools, tokens_usage).await?;
    }
    Ok(total_tokens)
}

pub struct DefaultAgenticLoopPipeline {
    session_key: SessionKey,
    session: Arc<Session>,
    last_total_tokens: AtomicU64,
    producer: BatchProducer,
}

#[async_trait]
impl AgentPipeline for DefaultAgenticLoopPipeline {
    async fn reset_system_prompt(&self) {
        self.session.set_system_message(DEFAULT_SYSTEM_PROMPT.clone()).await
    }
}

#[async_trait]
impl IncomingMessageProcessor for DefaultAgenticLoopPipeline {
    async fn incoming_message(self: Arc<Self>, event: Arc<IncomingMessageEvent>) {
        let cfg = ConfigManager::get().session_config::<ChannelBatchConfig, ChannelBatchConfig>(self.session_key.clone()).await;
        self.producer.send(event, cfg.channel_batch_interval_secs)
    }
}

#[async_trait]
impl IncomingMessageBatchProcessor for DefaultAgenticLoopPipeline {
    async fn incoming_message_batch(self: Arc<Self>, events: Vec<Arc<IncomingMessageEvent>>) {
        // 打包为一条 user 消息的 content（复用 message::pack_batch：extract_content + user_line + 空 content 跳过）
        let content = pack_batch(&events);
        // 找到工具
        let tools = tools_for_session(self.session_key.clone()).await;
        // 发送消息启动 agent 循环
        let token_usage = self.last_total_tokens.load(Ordering::Relaxed);
        if let Ok(total_tokens) = run(self.session_key.clone(), &self.session, content, &tools, Some(token_usage)).await {
            self.last_total_tokens.store(total_tokens, Ordering::Relaxed);
        }
    }
}

impl DefaultAgenticLoopPipeline {
    pub fn new(session_key: SessionKey, session: Arc<Session>) -> Arc<Self> {
        let (producer, mut consumer) = channel_batch_processor();
        let result = Arc::new(DefaultAgenticLoopPipeline {
            session_key,
            session,
            last_total_tokens: AtomicU64::new(0),
            producer
        });
        consumer.processor = Arc::downgrade(&result);
        spawn(consumer.run());
        result
    }
}

pub struct DefaultAgenticLoopPipelineWithMemory {
    session_key: SessionKey,
    session: Arc<Session>,
    load_memory: bool,
    last_total_tokens: AtomicU64,
    producer: BatchProducer,
}

#[async_trait]
impl AgentPipeline for DefaultAgenticLoopPipelineWithMemory {
    async fn reset_system_prompt(&self) {
        if let SessionKey::Memory(memory_key) = &self.session_key {
            if let Some(prompt) = get_ego_system_prompt(memory_key.agent_id.clone(), memory_key.role_event.role_name.clone()).await {
                self.session.set_system_message(prompt).await;
                return;
            }
        }
        self.session.set_system_message(DEFAULT_SYSTEM_PROMPT.clone()).await
    }
}

#[async_trait]
impl IncomingMessageProcessor for DefaultAgenticLoopPipelineWithMemory {
    async fn incoming_message(self: Arc<Self>, event: Arc<IncomingMessageEvent>) {
        // 记忆 pipeline 自己负责写入上行记录（Nexus 不再统一写入）
        if let SessionKey::Memory(memory_key) = &self.session_key {
            let request = channel_request_from_incoming(memory_key.as_ref(), event.as_ref(), None);
            Nexus::get().memory_store_client.push_channel_record(request).await;
        }
        let cfg = ConfigManager::get().session_config::<ChannelBatchConfig, ChannelBatchConfig>(self.session_key.clone()).await;
        self.producer.send(event, cfg.channel_batch_interval_secs)
    }
}

#[async_trait]
impl IncomingMessageBatchProcessor for DefaultAgenticLoopPipelineWithMemory {
    async fn incoming_message_batch(self: Arc<Self>, events: Vec<Arc<IncomingMessageEvent>>) {
        // 打包为一条 user 消息的 content（复用 message::pack_batch：extract_content + user_line + 空 content 跳过）
        let content = pack_batch(&events);
        // 找到工具
        let tools = tools_for_session(self.session_key.clone()).await;
        // 发送消息
        let _ = self.run(content, &tools).await;
    }
}

impl DefaultAgenticLoopPipelineWithMemory {
    pub fn new(session_key: SessionKey, session: Arc<Session>, load_memory: bool) -> Arc<Self> {
        let (producer, mut consumer) = channel_batch_processor();
        let result = Arc::new(DefaultAgenticLoopPipelineWithMemory {
            session_key,
            session,
            load_memory,
            last_total_tokens: AtomicU64::new(0),
            producer
        });
        consumer.processor = Arc::downgrade(&result);
        spawn(consumer.run());
        result
    }

    async fn run_once(
        &self,
        round: usize,
        messages: Vec<Message>,
        tools: &Vec<Arc<ToolConfig>>,
        pending_contents: &mut Vec<Content>,
    ) -> Result<bool> {
        // send message to llm
        let tokens_usage = self.last_total_tokens.load(Ordering::Relaxed);
        let mut res = if self.load_memory {
            send_messages_with_limit(self.session_key.clone(), self.session.as_ref(), messages, tools, &MemoryRecoverTokenReducer, tokens_usage).await?
        } else {
            send_messages_with_limit(self.session_key.clone(), self.session.as_ref(), messages, tools, &CompressTokenReducer, tokens_usage).await?
        };
        self.last_total_tokens.store(res.total_tokens, Ordering::Relaxed);
        // 推送 think 到 memory-store（reasoning_content + thinking 双字段，key 关联 ChannelRecord(Think)）
        if let SessionKey::Memory(mk) = &self.session_key {
            // 任一有值才写，都 None 跳过
            if res.reasoning_content.is_some() || res.thinking.is_some() {
                let key = ArcStr::from(uuid::Uuid::new_v4().to_string());
                let think_request = build_memory_think(mk.as_ref(), key, res.reasoning_content.clone(), res.thinking.clone());
                Nexus::get().memory_store_client.push_think(think_request).await;
            }
        }
        // 保存回复的消息
        let _ = self.session.context_append(vec![retrieve_assistant_message(&mut res)]).await;
        // 遍历执行 tool call
        let mut next = false;
        let errors = if let Some(tool_calls) = res.tool_calls.take() {
            let (tool_calls, errors) = match &self.session_key {
                SessionKey::Memory(memory_key) => {
                    call_tools_with_memory(tool_calls, memory_key.as_ref(), pending_contents).await
                }
                _ => call_tools(tool_calls).await,
            };
            if !tool_calls.is_empty() {
                next = true;
                let tool_messages = retrieve_tool_messages(tool_calls, false);
                self.session.context_append(tool_messages).await?;
            }
            errors
        } else {
            vec![]
        };
        // output channel
        if let SessionKey::Memory(mk) = &self.session_key {
            let out_channel = ConfigManager::get().out_channel_config(mk.agent_id.as_str(), mk.role_event.role_name.as_str()).await;
            if let Some(out_channel) = out_channel {
                let mut contents = std::mem::take(pending_contents);
                let content = if contents.is_empty() {
                    if let Some(content) = res.content.take() {
                        Some(Content::Text(content))
                    } else {
                        None
                    }
                } else {
                    if let Some(content) = res.content.take() {
                        contents.push(Content::Text(content));
                    }
                    Some(Content::Multi(Arc::new(contents)))
                };
                if let Some(content) = content {
                    send_outgoing(mk.as_ref(), out_channel.as_ref(), content, None).await;
                }
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

    pub async fn run(&self, message: Message, tools: &Vec<Arc<ToolConfig>>) -> Result<()> {
        let mut pending_contents = Vec::new();
        let mut round = 1 as usize;
        let mut next = self.run_once(round, vec![message], tools, &mut pending_contents).await?;
        while next {
            round = round + 1;
            next = self.run_once(round, vec![], tools, &mut pending_contents).await?;
        }
        Ok(())
    }
}

const PIPELINE_DEFAULT_STR: &str = "default";
const PIPELINE_DEFAULT_MEMORY_STR: &str = "default_memory";
const PIPELINE_ABSTRACT_MEMORY_STR: &str = "abstract_memory";
const PIPELINE_DEFAULT: ArcStr = arcstr::literal!(PIPELINE_DEFAULT_STR);
const PIPELINE_DEAULT_MEMORY: ArcStr = arcstr::literal!(PIPELINE_DEFAULT_MEMORY_STR);

pub fn create_pipeline(pipeline_name: ArcStr, session_key: SessionKey, session: Arc<Session>) -> Result<Arc<dyn AgentPipeline>> {
    match pipeline_name.as_str() {
        PIPELINE_DEFAULT_STR => Ok(DefaultAgenticLoopPipeline::new(session_key, session)),
        PIPELINE_DEFAULT_MEMORY_STR => Ok(DefaultAgenticLoopPipelineWithMemory::new(session_key, session, true)),
        PIPELINE_ABSTRACT_MEMORY_STR => match session_key {
            SessionKey::Memory(memory_key) => Ok(AbstractMemoryPipeline::new(memory_key, session)),
            SessionKey::Raw(_) => Err(Error::PipelineNotFound(pipeline_name, session_key)),
        },
        _ => Err(Error::PipelineNotFound(pipeline_name, session_key))
    }
}

/// 会话 pipeline 注册表
pub struct PipelineManager {
    pipeline_map: DashMap<SessionKey, (Arc<PipelineConfig>, Arc<dyn AgentPipeline>)>,
}

impl PipelineManager {
    pub fn new() -> Self {
        Self {
            pipeline_map: DashMap::new(),
        }
    }

    pub async fn sync_pipeline(&self, session_key: SessionKey, session: Arc<Session>) -> Result<()> {
        let cfg = ConfigManager::get().session_config::<PipelineConfig, PipelineConfig>(session_key.clone()).await;
        let pipeline_name = if let Some(name) = cfg.pipeline_name.as_ref() {
            name.clone()
        } else {
            match &session_key {
                SessionKey::Memory(_) => PIPELINE_DEAULT_MEMORY.clone(),
                SessionKey::Raw(_) => PIPELINE_DEFAULT.clone(),
            }
        };
        match self.pipeline_map.entry(session_key.clone()) {
            Entry::Occupied(mut entry) => {
                if cfg.as_ref() == entry.get().0.as_ref() {
                    return Ok(())
                }
                let pipeline = create_pipeline(pipeline_name, session_key, session)?;
                entry.insert((cfg, pipeline));
            }
            Entry::Vacant(entry) => {
                let pipeline = create_pipeline(pipeline_name, session_key, session)?;
                entry.insert((cfg, pipeline));
            }
        }
        Ok(())
    }

    /// 只保留仍在绑定集合中的会话的 trigger/pipeline（与 SessionManager::retain 成对调用，
    /// 会话被清理后其 trigger/pipeline 一并丢弃；trigger 丢弃会触发输入处理器的 Drop → 合批任务退出）
    pub fn retain(&self, keys: &HashSet<SessionKey>) {
        self.pipeline_map.retain(|k, _| keys.contains(k));
    }

    pub async fn incoming_message(&self, session_key: SessionKey, event: Arc<IncomingMessageEvent>) {
        // 输入处理器以 Arc<Self> 接收消息（便于把 Weak 交给批次消费者）：先克隆出 pipeline 再放掉 map 读锁
        let pipeline = self.pipeline_map.get(&session_key).map(|entry| entry.1.clone());
        if let Some(pipeline) = pipeline {
            pipeline.incoming_message(event).await;
        }
    }

    pub async fn reset_system_prompt(&self, session_key: SessionKey) {
        let pipeline = self.pipeline_map.get(&session_key).map(|entry| entry.1.clone());
        if let Some(pipeline) = pipeline {
            pipeline.reset_system_prompt().await;
        }
    }

    /// channel 不再绑定该会话：通知旧 pipeline 清理该 channel（messenger + 绑定用户）的批次状态
    pub async fn channel_unbound(&self, session_key: &SessionKey, messenger_id: &str, self_user_id: &str) {
        let pipeline = self.pipeline_map.get(session_key).map(|entry| entry.1.clone());
        if let Some(pipeline) = pipeline {
            pipeline.channel_unbound(ArcStr::from(messenger_id), ArcStr::from(self_user_id)).await;
        }
    }

    /// 单测用：会话是否已装配 pipeline（生产代码不依赖该查询）
    #[cfg(test)]
    pub fn contains(&self, session_key: &SessionKey) -> bool {
        self.pipeline_map.contains_key(session_key)
    }
}
