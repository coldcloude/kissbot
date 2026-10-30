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
use kissbot_api::{ChannelUser, Content, ContentBlock, IncomingMessageEvent, MemoryMeta, MergeEffectiveConfig, MergeSelf, OptionArcField, OutgoingMessage, RESERVED_AGENT_ID, ResourceContents};
use tokio::spawn;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arcstr::ArcStr;
use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, ModelResponse, Session, ToolCall, ToolConfig};

use crate::configs::{ChannelBatchConfig, CompressConfig, EffectiveCompressConfig, GroupSetConfig, OutChannel, OutChannelConfig, PipelineConfig, SessionConfigMap, SessionConfigMergeMap};
use crate::memory_store_client::QueryChannelData;
use crate::message::{extract_content, pack_batch};
use crate::pipeline::{AgentPipeline, IncomingMessageBatchProcessor, IncomingMessageProcessor, TokenReducer};
use crate::station::tool_config_from_mcp;
use crate::{config_manager::ConfigManager, configs::{EffectiveMemoryRecoverConfig, MemoryRecoverConfig}, message::{MessageContent, pack_memory_messages}, nexus::Nexus, station::Station, types::{Error, MemoryKey, Result, build_think_record, build_tool_call_record, build_tool_result_record, channel_request_from_incoming, channel_request_from_outgoing}};

/// 默认系统提示词
pub const DEFAULT_SYSTEM_PROMPT: ArcStr = arcstr::literal!("你是一个通用智能体。使用工具完成用户任务");

/// 根据 agent_id 获取系统提示词（新建 pipeline 的 session 时由系统提示词重置调用）：
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

/// 现场组装系统提示词（不缓存，新建/重置 session 时各自计算）
pub async fn get_ego_system_prompt_or_default(memory_key: &MemoryKey) -> ArcStr {
    get_ego_system_prompt(memory_key.agent_id.clone(), memory_key.role_event.role_name.clone()).await
    .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.clone())
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
            map.insert(key, extract_content(&rec.content, rec.user_name, rec.is_self));
        }
    }
}

/// 记忆打包（两次查询并集）→ 交替的 User/Assistant 消息：
/// ① RecentQuery 最近 N 条（无时间参数）→ ln = 最旧一条 time；
/// ② QueryRequest 时间范围 [M, ln]（M = min(时间窗起点, ln)，无 limit，M == ln 时退化为单点取 ln 同时间组）；
/// 结果 = ① ∪ ②（两次解析共用同一 BTreeMap，键 (time, sn) → 去重 + 天然时间正序），升序打包。
/// 任一步失败（时间窗计算 / 查询）均返回空上下文（不阻塞消息发送）。
pub async fn build_context_from_memory_store(memory_key: Arc<MemoryKey>, session_id: ArcStr) -> Vec<Message> {
    let cfg = ConfigManager::get().session_config::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(session_id).await;
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

pub async fn send_messages(session_id: ArcStr, session: &Session, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse> {
    let _ = session.context_append(messages).await;
    let full_messages = session.build_context_full().await;
    let llm_cfg = ConfigManager::get().session_config::<LLMConfig, EffectiveLLMConfig>(session_id).await;
    Nexus::get().provider_manager.call(llm_cfg.as_ref(), full_messages, tools).await
}

async fn send_messages_with_limit(session_id: ArcStr, session: &Session, mut messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>, reducer: &impl TokenReducer, tokens_usage: u64) -> Result<ModelResponse> {
    let cfg_mngr = ConfigManager::get();
    let llm_cfg = cfg_mngr.session_config::<LLMConfig, EffectiveLLMConfig>(session_id.clone()).await;
    let compress_cfg = cfg_mngr.session_config::<CompressConfig, EffectiveCompressConfig>(session_id.clone()).await;
    let provider = llm_cfg.provider.as_str();
    let model = llm_cfg.model.as_str();
    let model_cfg = cfg_mngr.provider_model_config(provider, model).await
    .ok_or_else(|| Error::ModelProviderNotFound(ArcStr::from(provider), ArcStr::from(model)))?;

    // 1. 检查上下文 token 占用超限
    //    阈值来自 session 模型的 max_tokens_usage
    //    上次模型响应的 usage.total_tokens 超过阈值触发
    if model_cfg.max_tokens_usage > 0 {
        let max_tokens_usage = ((model_cfg.max_tokens_usage as f64) * compress_cfg.compress_threshold) as u64;
        if tokens_usage > max_tokens_usage {
            (_, messages) = reducer.reduce(session_id, session, messages).await;
        }
    }
    // 2. 正常的发送流程
    let _ = session.context_append(messages).await;
    let full_messages = session.build_context_full().await;
    Nexus::get().provider_manager.call(llm_cfg.as_ref(), full_messages, tools).await
}

pub fn retrieve_assistant_message(res: &mut ModelResponse) -> Message {
    Message::Assistant {
        content: res.raw_content.take(),
        reasoning_content: res.raw_reasoning_content.take(),
        tool_calls: res.raw_tool_calls.take(),
    }
}

/// session 可用工具：session 配置的启用 group 白名单 → Station 平铺查询（本级 + 直接子递归）并按 group 本地过滤
/// 结果转换为 LLM 侧工具定义；聚合为空则请求不携带 tools 字段（兼容无工具场景）
pub async fn tools_for_session(session_id: ArcStr) -> Vec<Arc<ToolConfig>> {
    let cfg = ConfigManager::get().session_config::<GroupSetConfig, GroupSetConfig>(session_id).await;
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

/// Agentic Loop 产出回复：发到 pipeline 的 out_channel（role_event 记忆编码），
/// 成功后把下行消息写入记忆（is_self=1）；`meta` 由调用方给出（摘要记忆 pipeline 写入当前 case）。
///
/// 发送前校验 out_channel 身份在目标 channel 仍绑定；未绑定 → 跳过发送。
pub async fn send_outgoing(
    memory_key: &MemoryKey,
    out_channel: &OutChannel,
    content: Content,
    meta: Option<Arc<MemoryMeta>>,
) {
    let nexus = Nexus::get();
    // 1. 校验 out_channel 身份在目标 channel 仍绑定
    let bound = ConfigManager::get().channel(out_channel.channel_id.as_str()).await
        .map(|c| c.bind_users.contains(&out_channel.user))
        .unwrap_or(false);
    if !bound {
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
        Err(_) => {}
    }
}

/// 本次 run 的下行内容：本轮没有文本时返回 None（不发送，累积的思考/工具内容留到下一轮）；
/// 有文本时把累积的思考/工具内容与文本合并为 `Multi`（没有累积内容则直接 `Text`），并清空累积。
pub fn outgoing_content(pending_contents: &mut Vec<Content>, text: Option<ArcStr>) -> Option<Content> {
    let text = text?;
    if pending_contents.is_empty() {
        return Some(Content::Text(text));
    }
    let mut contents = std::mem::take(pending_contents);
    contents.push(Content::Text(text));
    Some(Content::Multi(Arc::new(contents)))
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

/// 带记忆内容的工具调用结果：ToolCall 内容在执行前构造，成功时在其后追加 ToolResult 内容
struct MemoryToolCall {
    tool_call: ToolCall,
    contents: Vec<Content>,
    error: Option<Error>,
}

async fn call_tool_with_memory(tool_call: ToolCall) -> MemoryToolCall {
    // 1. 工具调用内容：执行前构造，随回复的通道记录一起落盘
    let contents = vec![Content::ToolCall(Arc::new(build_tool_call_record(&tool_call)))];
    // 2. 执行工具
    let (tool_call, error) = call_tool(tool_call).await;
    let mut contents = contents;
    // 3. 成功时在 ToolCall 内容之后追加 ToolResult 内容（按 Multi 中的先后顺序配对）
    if error.is_none() {
        contents.push(Content::ToolResult(Arc::new(build_tool_result_record(&tool_call))));
    }
    MemoryToolCall { tool_call, contents, error }
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

/// 并发执行带记忆内容的 tool calls；按返回顺序收集 ToolCall/ToolResult 内容。
///
/// 工具调用本身并发执行；内容统一在 join_all 完成后收集，避免并发写入。
pub async fn call_tools_with_memory(
    tool_calls: Vec<ToolCall>,
) -> (Vec<ToolCall>, Vec<Content>, Vec<Error>) {
    let results: Vec<MemoryToolCall> = future::join_all(
        tool_calls.into_iter().map(call_tool_with_memory)
    ).await;
    let mut tool_calls = Vec::with_capacity(results.len());
    let mut contents = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        contents.extend(result.contents);
        if let Some(error) = result.error {
            errors.push(error);
        }
        tool_calls.push(result.tool_call);
    }
    (tool_calls, contents, errors)
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

pub async fn run_once(session_id: ArcStr, session: &Session, round: usize, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>, tokens_usage: Option<u64>) -> Result<(bool,u64)> {
    let result = if let Some(tokens_usage) = tokens_usage {
        let reducer = CompressTokenReducer::new();
        send_messages_with_limit(session_id.clone(), session, messages, tools, &reducer, tokens_usage).await
    } else {
        send_messages(session_id, session, messages, tools).await
    };
    let mut res = match result {
        Ok(res) => res,
        Err(e) => return Err(e),
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

pub async fn run(session_id: ArcStr, session: &Session, message: Message, tools: &Vec<Arc<ToolConfig>>, mut tokens_usage: Option<u64>) -> Result<u64> {
    let mut round = 1 as usize;
    let (mut next, mut total_tokens) = run_once(session_id.clone(), session, round, vec![message], tools, tokens_usage).await?;
    while next {
        round = round + 1;
        if tokens_usage.is_some() {
            tokens_usage = Some(total_tokens);
        }
        (next, total_tokens) = run_once(session_id.clone(), session, round, vec![], tools, tokens_usage).await?;
    }
    Ok(total_tokens)
}

pub struct DefaultAgenticLoopPipeline {
    session_id: ArcStr,
    session: Arc<Session>,
    last_total_tokens: AtomicU64,
    producer: BatchProducer,
}

#[async_trait]
impl AgentPipeline for DefaultAgenticLoopPipeline {
    async fn reset_system_prompt(&self) {
        self.session.set_system_message(DEFAULT_SYSTEM_PROMPT.clone()).await
    }

    async fn execute_command(&self, _channel_id: ArcStr, content: ArcStr) -> Option<Result<ArcStr>> {
        let parts = command_parts(&content);
        match parts.first().map(|p| p.as_str()) {
            Some("/model") => Some(handle_model_command(&parts, self.session_id.clone()).await),
            _ => None,
        }
    }
}

#[async_trait]
impl IncomingMessageProcessor for DefaultAgenticLoopPipeline {
    async fn incoming_message(self: Arc<Self>, event: Arc<IncomingMessageEvent>) {
        let cfg = ConfigManager::get().session_config::<ChannelBatchConfig, ChannelBatchConfig>(self.session_id.clone()).await;
        self.producer.send(event, cfg.channel_batch_interval_secs)
    }
}

#[async_trait]
impl IncomingMessageBatchProcessor for DefaultAgenticLoopPipeline {
    async fn incoming_message_batch(self: Arc<Self>, events: Vec<Arc<IncomingMessageEvent>>) {
        // 打包为一条 user 消息的 content（复用 message::pack_batch：extract_content + user_line + 空 content 跳过）
        let content = pack_batch(&events);
        // 找到工具
        let tools = tools_for_session(self.session_id.clone()).await;
        // 发送消息启动 agent 循环
        let token_usage = self.last_total_tokens.load(Ordering::Relaxed);
        if let Ok(total_tokens) = run(self.session_id.clone(), &self.session, content, &tools, Some(token_usage)).await {
            self.last_total_tokens.store(total_tokens, Ordering::Relaxed);
        }
    }
}

impl DefaultAgenticLoopPipeline {
    pub async fn new(session_id: ArcStr) -> Arc<Self> {
        let session = create_session(session_id.clone()).await;
        let (producer, mut consumer) = channel_batch_processor();
        let result = Arc::new(DefaultAgenticLoopPipeline {
            session_id,
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
    memory_key: Arc<MemoryKey>,
    session_id: ArcStr,
    session: Arc<Session>,
    /// 是否读取记忆（决定超限时用记忆回溯还是压缩 reducer）；写记忆始终进行
    load_memory: bool,
    reducer: MemoryRecoverTokenReducer,
    last_total_tokens: AtomicU64,
    producer: BatchProducer,
}

#[async_trait]
impl AgentPipeline for DefaultAgenticLoopPipelineWithMemory {
    async fn reset_system_prompt(&self) {
        let prompt = get_ego_system_prompt_or_default(self.memory_key.as_ref()).await;
        self.session.set_system_message(prompt).await
    }

    async fn execute_command(&self, channel_id: ArcStr, content: ArcStr) -> Option<Result<ArcStr>> {
        let parts = command_parts(&content);
        match parts.first().map(|p| p.as_str()) {
            Some("/model") => Some(handle_model_command(&parts, self.session_id.clone()).await),
            Some("/bind-outgoing") => Some(bind_outgoing(self.session_id.clone(), &channel_id, &parts).await),
            Some("/unbind-outgoing") => Some(unbind_outgoing(self.session_id.clone(), &parts).await),
            _ => None,
        }
    }
}

#[async_trait]
impl IncomingMessageProcessor for DefaultAgenticLoopPipelineWithMemory {
    async fn incoming_message(self: Arc<Self>, event: Arc<IncomingMessageEvent>) {
        // 记忆 pipeline 自己负责写入上行记录（Nexus 不再统一写入）
        let request = channel_request_from_incoming(self.memory_key.as_ref(), event.as_ref(), None);
        Nexus::get().memory_store_client.push_channel_record(request).await;
        let cfg = ConfigManager::get().session_config::<ChannelBatchConfig, ChannelBatchConfig>(self.session_id.clone()).await;
        self.producer.send(event, cfg.channel_batch_interval_secs)
    }
}

#[async_trait]
impl IncomingMessageBatchProcessor for DefaultAgenticLoopPipelineWithMemory {
    async fn incoming_message_batch(self: Arc<Self>, events: Vec<Arc<IncomingMessageEvent>>) {
        // 打包为一条 user 消息的 content（复用 message::pack_batch：extract_content + user_line + 空 content 跳过）
        let content = pack_batch(&events);
        // 找到工具
        let tools = tools_for_session(self.session_id.clone()).await;
        // 发送消息
        let _ = self.run(content, &tools).await;
    }
}

impl DefaultAgenticLoopPipelineWithMemory {
    pub async fn new(memory_key: Arc<MemoryKey>, load_memory: bool) -> Arc<Self> {
        let session_id = memory_key.encode();
        // 从 agent role 配置导入
        import_session_config_from_agent_role::<LLMConfig, EffectiveLLMConfig>(&memory_key, &session_id).await;
        if load_memory {
            import_session_config_from_agent_role::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(&memory_key, &session_id).await;
        } else {
            import_session_config_from_agent_role::<CompressConfig, EffectiveCompressConfig>(&memory_key, &session_id).await;
        }
        import_session_config_from_agent_role::<ChannelBatchConfig, ChannelBatchConfig>(&memory_key, &session_id).await;
        import_session_config_from_agent_role::<OutChannelConfig, OutChannelConfig>(&memory_key, &session_id).await;
        import_session_config_from_agent_role::<GroupSetConfig, GroupSetConfig>(&memory_key, &session_id).await;
        // 建立 session
        let session = create_session(session_id.clone()).await;
        let (producer, mut consumer) = channel_batch_processor();
        let reducer = MemoryRecoverTokenReducer::new(memory_key.clone());
        let result = Arc::new(DefaultAgenticLoopPipelineWithMemory {
            memory_key,
            session_id,
            session,
            load_memory,
            reducer,
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
            send_messages_with_limit(
                self.session_id.clone(), self.session.as_ref(),
                messages, tools, &self.reducer, tokens_usage).await?
        } else {
            let reducer = CompressTokenReducer::new();
            send_messages_with_limit(
                self.session_id.clone(), self.session.as_ref(),
                messages, tools, &reducer, tokens_usage).await?
        };
        self.last_total_tokens.store(res.total_tokens, Ordering::Relaxed);
        // 思考内容累积到本次 run（reasoning_content + thinking 双字段）；任一有值才累积，都 None 跳过。
        // 它随最终回复的通道记录一起落盘，不单独写入记忆。
        if res.reasoning_content.is_some() || res.thinking.is_some() {
            let record = build_think_record(res.reasoning_content.clone(), res.thinking.clone());
            pending_contents.push(Content::Think(Arc::new(record)));
        }
        // 保存回复的消息
        let _ = self.session.context_append(vec![retrieve_assistant_message(&mut res)]).await;
        // 遍历执行 tool call
        let mut next = false;
        let errors = if let Some(tool_calls) = res.tool_calls.take() {
            let (tool_calls, tool_contents, errors) = call_tools_with_memory(tool_calls).await;
            pending_contents.extend(tool_contents);
            if !tool_calls.is_empty() {
                next = true;
                let tool_messages = retrieve_tool_messages(tool_calls, false);
                self.session.context_append(tool_messages).await?;
            }
            errors
        } else {
            vec![]
        };
        // output channel：只有本轮有文本才发送，本轮及之前累积的思考/工具内容与文本一起作为下行记录写入
        if let Some(out_channel) = outgoing_channel(&self.session_id).await {
            if let Some(content) = outgoing_content(pending_contents, res.content.take()) {
                send_outgoing(self.memory_key.as_ref(), out_channel.as_ref(), content, None).await;
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
const PIPELINE_DEFAULT_MEMORY: ArcStr = arcstr::literal!(PIPELINE_DEFAULT_MEMORY_STR);

/// pipeline 名称是否受支持（与 create_pipeline 的注册表一致）
pub fn is_supported_pipeline_name(name: &str) -> bool {
    matches!(name, PIPELINE_DEFAULT_STR | PIPELINE_DEFAULT_MEMORY_STR | PIPELINE_ABSTRACT_MEMORY_STR)
}

/// 把 (agent, role) 的某项配置导入 session 配置；已有 session 级值时不覆盖。
pub(crate) async fn import_session_config_from_agent_role<C, E>(memory_key: &MemoryKey, session_id: &ArcStr)
where
    C: MergeSelf + Clone + MergeEffectiveConfig<E>,
    E: Clone,
    SessionConfigMergeMap: OptionArcField<C>,
    SessionConfigMap: OptionArcField<E>,
{
    let cfg_mngr = ConfigManager::get();
    let cfg = cfg_mngr.agent_role_config::<C>(
        memory_key.agent_id.as_str(),
        memory_key.role_event.role_name.as_str(),
    ).await.unwrap_or_default();
    let _ = cfg_mngr.set_session_config_if_not_exists(
        session_id.clone(), Arc::new(cfg.get_effective_config())).await;
}

/// 建立 session 时从本地缓存恢复；default_memory / abstract_memory 在建立前从 agent/role 导入配置，
/// default 只用 session 配置
pub async fn create_session(session_id: ArcStr) -> Arc<Session> {
    let cfg = ConfigManager::get();
    let session = Arc::new(Session::new(cfg.data_dir(), session_id));
    let _ = session.recover_from_cache().await;
    session
}

/// 给 session 写入模型覆盖（provider/model；其余字段保留当前有效值）
pub async fn set_session_model_config(session_id: ArcStr, provider: ArcStr, model: ArcStr) -> Result<()> {
    let mut over = LLMConfig::default();
    over.provider = Some(provider);
    over.model = Some(model);
    ConfigManager::get().set_session_config::<LLMConfig, EffectiveLLMConfig>(session_id, &over).await
}

// ==================== pipeline 命令 ====================

/// 命令按空白切分（与默认命令检测一致）
pub fn command_parts(content: &str) -> Vec<ArcStr> {
    content.split_whitespace().map(ArcStr::from).collect()
}

/// `/model` 参数解析（调用方已按命令名匹配）：参数个数不符返回 InvalidCommand
pub fn parse_model_command(parts: &[ArcStr]) -> Result<(ArcStr, ArcStr)> {
    if parts.len() != 3 {
        return Err(Error::InvalidCommand("格式: /model <provider> <model>".into()));
    }
    Ok((parts[1].clone(), parts[2].clone()))
}

/// 校验模型有效性：从 API 拉模型列表，确认 model 在列表中（失败保持原模型）
async fn verify_model(provider: &str, model: &str) -> Result<()> {
    let models = Nexus::get().provider_manager.list_models(provider).await
        .map_err(|e| Error::ModelApiError(format!("获取模型列表失败: {}", e).into()))?;
    if !models.iter().any(|m| m.as_str() == model) {
        return Err(Error::ModelProviderNotSupported(format!(
            "模型 {} 不在 {} 的 API 模型列表", model, provider).into()));
    }
    Ok(())
}

/// 校验模型并写入多个 session（pipeline 决定写入哪些 session）；返回回复文案
pub async fn switch_model(session_ids: Vec<ArcStr>, provider: ArcStr, model: ArcStr) -> Result<ArcStr> {
    verify_model(&provider, &model).await?;
    for session_id in session_ids {
        set_session_model_config(session_id, provider.clone(), model.clone()).await?;
    }
    Ok(ArcStr::from(format!("✅ 已切换模型为: {}/{}", provider, model)))
}

/// 单 session pipeline 的 `/model` 执行（调用方已按命令名匹配）
pub async fn handle_model_command(parts: &[ArcStr], session_id: ArcStr) -> Result<ArcStr> {
    let (provider, model) = parse_model_command(parts)?;
    switch_model(vec![session_id], provider, model).await
}

/// `/bind-outgoing <messenger_id> <user_id> <group_id>`：设 pipeline 的回复通道
/// （先校验来源 channel 存在、该身份已绑定来源 channel）
pub async fn bind_outgoing(session_id: ArcStr, channel_id: &ArcStr, parts: &[ArcStr]) -> Result<ArcStr> {
    if parts.len() < 4 {
        return Err(Error::InvalidCommand(
            "格式: /bind-outgoing <messenger_id> <user_id> <group_id>".into()
        ));
    }
    let cfg_mngr = ConfigManager::get();
    let Some(ch) = cfg_mngr.channel(channel_id.as_str()).await else {
        return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id).into()));
    };
    let user = ChannelUser { messenger_id: parts[1].clone(), user_id: parts[2].clone() };
    if !ch.bind_users.contains(&user) {
        return Err(Error::InvalidCommand(format!(
            "ChannelUser 未绑定: {} / {}", parts[1], parts[2]).into()));
    }
    // 存到 pipeline 的 session（channel_id = 来源 channel）
    cfg_mngr.set_session_config::<OutChannelConfig, OutChannelConfig>(session_id, &OutChannelConfig {
        out_channel: Some(Arc::new(OutChannel {
            channel_id: channel_id.clone(),
            user: Arc::new(user),
            group_id: parts[3].clone(),
        })),
    }).await?;
    Ok(ArcStr::from(format!("✅ 已设发送通道: {} / {} -> {}", parts[1], parts[2], parts[3])))
}

/// `/unbind-outgoing`：清空 pipeline 的回复通道（回到只存不回复模式）
pub async fn unbind_outgoing(session_id: ArcStr, parts: &[ArcStr]) -> Result<ArcStr> {
    if parts.len() > 1 {
        return Err(Error::InvalidCommand("格式: /unbind-outgoing（无参数）".into()));
    }
    ConfigManager::get().set_session_config::<OutChannelConfig, OutChannelConfig>(
        session_id, &OutChannelConfig { out_channel: None }).await?;
    Ok(ArcStr::from("✅ 已取消发送通道（只存不回复）"))
}

/// 读 pipeline（session）的回复通道配置；未配置 = 只存不回复
pub async fn outgoing_channel(session_id: &ArcStr) -> Option<Arc<OutChannel>> {
    ConfigManager::get()
        .session_config::<OutChannelConfig, OutChannelConfig>(session_id.clone()).await
        .out_channel.clone()
}

pub async fn create_pipeline(pipeline_name: ArcStr, memory_key: Arc<MemoryKey>) -> Result<Arc<dyn AgentPipeline>> {
    match pipeline_name.as_str() {
        PIPELINE_DEFAULT_STR => Ok(DefaultAgenticLoopPipeline::new(memory_key.encode()).await),
        PIPELINE_DEFAULT_MEMORY_STR => Ok(DefaultAgenticLoopPipelineWithMemory::new(memory_key, true).await),
        PIPELINE_ABSTRACT_MEMORY_STR => Ok(AbstractMemoryPipeline::new(memory_key)),
        _ => Err(Error::PipelineNotFound(pipeline_name, memory_key))
    }
}

/// pipeline 注册表：key 为 pipeline 身份（agent_id + role_event）
pub struct PipelineManager {
    pipeline_map: DashMap<Arc<MemoryKey>, Arc<dyn AgentPipeline>>,
}

impl PipelineManager {
    pub fn new() -> Self {
        Self {
            pipeline_map: DashMap::new(),
        }
    }

    /// 定位 pipeline，不存在则按 agent/role 配置的 pipeline_name 装配（默认 default_memory）
    /// 并初始化系统提示词；pipeline 需要哪些 session 由它自己的构造过程决定
    pub async fn ensure_pipeline(&self, memory_key: Arc<MemoryKey>) -> Result<Arc<dyn AgentPipeline>> {
        if let Some(pipeline) = self.pipeline_map.get(&memory_key) {
            return Ok(pipeline.clone());
        }
        let cfg = ConfigManager::get()
            .agent_role_config::<PipelineConfig>(memory_key.agent_id.as_str(), memory_key.role_event.role_name.as_str())
            .await;
        let pipeline_name = cfg.and_then(|cfg| cfg.pipeline_name)
            .unwrap_or_else(|| PIPELINE_DEFAULT_MEMORY.clone());
        let pipeline = create_pipeline(pipeline_name, memory_key.clone()).await?;
        pipeline.reset_system_prompt().await;
        match self.pipeline_map.entry(memory_key) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                entry.insert(pipeline.clone());
                Ok(pipeline)
            }
        }
    }

    /// 修改 agent（role 空）或 agent+role 的 pipeline 配置：
    /// 先保存受影响 pipeline 当前 merge 后的配置，写新配置后重新 merge；
    /// pipeline 名称变化的已装配 pipeline 从注册表移除（下次按新配置重建，session 随 Arc 释放回收）
    pub async fn set_pipeline_config(&self, agent_id: ArcStr, role_name: ArcStr, pipeline_name: ArcStr) -> Result<()> {
        let cfg = ConfigManager::get();
        // 受影响的已装配 pipeline：agent 级（role 空）= 该 agent 全部 role；role 级 = 该 (agent, role)
        let affected: Vec<Arc<MemoryKey>> = self.pipeline_map.iter()
            .filter(|entry| {
                let key = entry.key();
                key.agent_id == agent_id
                    && (role_name.is_empty() || key.role_event.role_name == role_name)
            })
            .map(|entry| entry.key().clone())
            .collect();
        // 提前保存受影响 role 的当前 merge 配置（同 role 的多个 event pipeline 共用一份配置）
        let mut roles: Vec<ArcStr> = affected.iter().map(|key| key.role_event.role_name.clone()).collect();
        roles.sort();
        roles.dedup();
        let mut old_names = HashMap::new();
        for role in &roles {
            let old_name = cfg.agent_role_config::<PipelineConfig>(agent_id.as_str(), role.as_str()).await
                .and_then(|config| config.pipeline_name);
            old_names.insert(role.clone(), old_name);
        }
        // 写新配置（role 空 = agent 级）
        cfg.set_agent_role_config::<PipelineConfig>(
            agent_id.clone(), role_name, Arc::new(PipelineConfig { pipeline_name: Some(pipeline_name) }),
        ).await?;
        // 重新生成 merge 后的配置；名称变化的 role 下已装配 pipeline 失效
        for role in roles {
            let new_name = cfg.agent_role_config::<PipelineConfig>(agent_id.as_str(), role.as_str()).await
                .and_then(|config| config.pipeline_name);
            if old_names.get(&role) != Some(&new_name) {
                for key in &affected {
                    if key.role_event.role_name == role {
                        self.pipeline_map.remove(key);
                    }
                }
            }
        }
        Ok(())
    }

    /// 只保留仍在绑定集合中的 pipeline（pipeline 丢弃会触发输入处理器 Drop → 合批任务退出；
    /// 其持有的 session 随 pipeline 的 Arc 释放自然回收）
    pub fn retain(&self, keys: &HashSet<Arc<MemoryKey>>) {
        self.pipeline_map.retain(|k, _| keys.contains(k));
    }

    pub async fn incoming_message(&self, memory_key: Arc<MemoryKey>, event: Arc<IncomingMessageEvent>) -> Result<()> {
        let pipeline = self.ensure_pipeline(memory_key).await?;
        pipeline.incoming_message(event).await;
        Ok(())
    }

    /// channel 不再绑定该 pipeline：通知旧 pipeline 清理该 channel（messenger + 绑定用户）的批次状态
    pub async fn channel_unbound(&self, memory_key: &Arc<MemoryKey>, messenger_id: &str, self_user_id: &str) {
        let pipeline = self.pipeline_map.get(memory_key).map(|entry| entry.clone());
        if let Some(pipeline) = pipeline {
            pipeline.channel_unbound(ArcStr::from(messenger_id), ArcStr::from(self_user_id)).await;
        }
    }

    /// 单测用：pipeline 是否已装配（生产代码不依赖该查询）
    #[cfg(test)]
    pub fn contains(&self, memory_key: &MemoryKey) -> bool {
        self.pipeline_map.contains_key(memory_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn think_content() -> Content {
        Content::Think(Arc::new(kissbot_api::ThinkRecord {
            reasoning_content: Some(ArcStr::from("推理")),
            thinking: None,
        }))
    }

    fn tool_call_content() -> Content {
        Content::ToolCall(Arc::new(kissbot_api::ToolCallRecord {
            tool_name: ArcStr::from("get_date"),
            tool_params: Arc::new(serde_json::json!({})),
        }))
    }

    #[test]
    fn outgoing_content_waits_for_text_and_drains_pending() {
        let mut pending = vec![think_content(), tool_call_content()];
        assert!(outgoing_content(&mut pending, None).is_none(), "没有文本不发送");
        assert_eq!(pending.len(), 2, "累积内容保留到下一轮");

        let Some(content) = outgoing_content(&mut pending, Some(ArcStr::from("回复"))) else {
            panic!("有文本应发送");
        };
        assert!(pending.is_empty(), "发送后清空累积");
        match content {
            Content::Multi(items) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(items[0], Content::Think(_)));
                assert!(matches!(items[1], Content::ToolCall(_)));
                assert!(matches!(&items[2], Content::Text(text) if text.as_str() == "回复"));
            }
            other => panic!("有累积内容时应为 Multi: {:?}", other),
        }
    }

    #[test]
    fn outgoing_content_without_pending_is_plain_text() {
        let mut pending = Vec::new();
        match outgoing_content(&mut pending, Some(ArcStr::from("回复"))) {
            Some(Content::Text(text)) => assert_eq!(text.as_str(), "回复"),
            other => panic!("无累积内容时应为 Text: {:?}", other),
        }
    }
}
