pub mod abstract_input_processor;
pub mod channel_batch_processor;
pub mod token_reducer;

use async_trait::async_trait;
use futures_util::future;
use kissbot_api::{Content, ContentBlock, RESERVED_AGENT_ID, ResourceContents};
pub use token_reducer::*;
use tokio::sync::Mutex;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;

use arcstr::ArcStr;
use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, ModelResponse, Session, ToolCall, ToolConfig};

use crate::configs::{CompressConfig, EffectiveCompressConfig};
use crate::memory_store_client::QueryChannelData;
use crate::message::extract_content;
use crate::pipeline::{TokenReducer, ToolCaller};
use crate::types::build_memory_think;
use crate::{config_manager::ConfigManager, configs::{EffectiveMemoryRecoverConfig, MemoryRecoverConfig}, message::{MessageContent, pack_memory_messages}, nexus::Nexus, station::Station, types::{Error, MemoryKey, Result, SessionKey, build_memory_tool_call, build_memory_tool_result}};


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

/// 默认系统提示词
pub const DEFAULT_SYSTEM_PROMPT: ArcStr = arcstr::literal!("你是一个通用智能体。使用可用工具完成用户任务");

pub async fn get_system_prompt(memory_key: &MemoryKey) -> ArcStr {
    if let Some(prompt) = get_ego_system_prompt(memory_key.agent_id.clone(), memory_key.role_event.role_name.clone()).await {
        prompt
    } else {
        DEFAULT_SYSTEM_PROMPT.clone()
    }
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

/// 记忆打包：两次查询并集——① RecentQuery 最近 N 条（无时间参数）→ ln = 最旧一条 time；
/// ② QueryRequest 时间范围 [M, ln]（M = min(时间窗起点, ln)，无 limit，M == ln 时退化为单点取 ln 同时间组）；
/// 结果 = ① ∪ ②（两次解析共用同一 BTreeMap，键 (time, sn) → 去重 + 天然时间正序），升序返回
/// （HTTP 经共享 config.send_store_query 发送）
/// 参数：role_event 角色 + 事件模式、memory_time_secs 时间窗（秒）、memory_count 条数上限（与 context 解耦，标量传入）
pub async fn read_recent_for_context(
    memory_key: &MemoryKey,
    memory_time_secs: u64,
    memory_count: usize,
) -> Result<Vec<MessageContent>> {
    let nexus = Nexus::get();

    // 时间窗起点计算失败（checked_sub_signed 溢出）→ 直接报错，不静默回退
    let start = chrono::Local::now()
        .checked_sub_signed(chrono::Duration::seconds(memory_time_secs as i64))
        .map(|t| ArcStr::from(t.format("%Y-%m-%d %H:%M:%S").to_string()))
        .ok_or_else(|| Error::MemoryTimeWindow("计算记忆时间窗起点失败".into()))?;

    let count = memory_count;

    // ===== 解析：BTreeMap 以 (time, sn) 为键同时做去重与排序（迭代天然升序） =====
    // 两次查询共用同一 map → 并集自动去重（Query2 的 [M, ln] 区间与 Query1 尾部在 ln 处重叠）
    let mut map: BTreeMap<(ArcStr, u64), MessageContent> = BTreeMap::new();

    // ===== Query1：POST {store}/store/query/channel/recent（RecentQuery，无时间参数） =====
    let groups = nexus.memory_store_client.query_last(memory_key.agent_id.clone(), memory_key.role_event.clone(), count as u32).await?;
    parse_channel_groups(groups, &mut map);

    // ln = 已解析记录最旧一条的 time（BTreeMap 首键）；map 为空说明无记录（count=0 时 Query1 必为空）
    // 一并在此处理；Query1 最旧若为非文本，其同时间组除非文本外无其他记录（否则文本记录会占据更小首键）
    let Some((ln, _)) = map.keys().next() else {
        return Ok(Vec::new());
    };
    let ln = ln.clone();
    // 不足 N 条：直接返回（无需 Query2）。理论上去重后记录不重复，map 大小即取到的数量；
    // 注意：以解析后文本数（map.len()）判定——非文本记录在最后 N 内被跳过时也会提前返回
    if map.len() < count {
        return Ok(map.into_values().collect());
    }
    // M = min(时间窗起点 start, ln)
    let m = if start.as_str() < ln.as_str() { start } else { ln.clone() };  // M = min(cutoff, ln)

    // ===== Query2：POST {store}/store/query/channel（QueryRequest 时间范围 [M, ln]，无 limit） =====
    // M == ln 时退化为单点 [ln, ln]（取 ln 同时间组）；与 Query1 并集（共用 map 去重）
    let groups = nexus.memory_store_client.query_recent(memory_key.agent_id.clone(), memory_key.role_event.clone(), m, ln).await?;
    parse_channel_groups(groups, &mut map);

    Ok(map.into_values().collect())
}

pub async fn build_context_from_memory_store(memory_key: Arc<MemoryKey>) -> Vec<Message> {
    let cfg = ConfigManager::get().memory_session_config::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(memory_key.clone()).await;
    read_recent_for_context(memory_key.as_ref(), cfg.memory_time_secs, cfg.memory_count).await
    .map_or_else(|_| vec![], |msgs| pack_memory_messages(&msgs))
}

pub async fn send_messages(session_key: SessionKey, session: &Session, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse> {
    session.context_append(messages).await;
    let full_messages = session.build_context().await;
    let llm_cfg = ConfigManager::get().session_config::<LLMConfig, EffectiveLLMConfig>(session_key).await;
    Nexus::get().provider_manager.call(llm_cfg.as_ref(), full_messages, tools).await
}

async fn send_messages_with_limit(session_key: SessionKey, session: &Session, mut messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>, reducer: &impl TokenReducer, usage: &AtomicU64) -> Result<ModelResponse> {
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
        let mut tokens_usage = usage.load(Relaxed);
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
            (tokens_usage, messages) = reducer.reduce(session_key, session, pending_messages).await;
            // 保存本次请求 token 占用
            usage.store(tokens_usage, Relaxed);
        }
    }
    // 2. 正常的发送流程
    session.context_append(messages).await;
    let full_messages = session.build_context().await;
    let response = Nexus::get().provider_manager.call(llm_cfg.as_ref(), full_messages, tools).await;
    if let Ok(model_resp) = response.as_ref() {
        // 保存本次请求 token 占用
        usage.store(model_resp.total_tokens, Relaxed);
    }
    response
}

pub fn retrieve_assistant_message(res: &mut ModelResponse) -> Message {
    Message::Assistant {
        content: res.raw_content.take(),
        reasoning_content: res.raw_reasoning_content.take(),
        tool_calls: res.raw_tool_calls.take(),
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

pub async fn call_tools_dyn(mut tool_calls: Vec<ToolCall>, op: &impl ToolCaller) -> (Vec<ToolCall>, Vec<Error>) {
    let mut futs = Vec::with_capacity(tool_calls.len());
    for tool_call in tool_calls.drain(..) {
        let f = op.call_tool(tool_call);
        futs.push(f);
    }
    let mut tool_results = future::join_all(futs).await;
    let mut errors = Vec::new();
    let mut tool_calls = Vec::with_capacity(tool_results.len());
    for (tool_call, mut error) in tool_results.drain(..) {
        if let Some(error) = error.take() {
            errors.push(error);
        }
        tool_calls.push(tool_call);
    }
    (tool_calls, errors)
}

struct RawToolCaller;

#[async_trait]
impl ToolCaller for RawToolCaller {
    async fn call_tool(&self, tool_call: ToolCall) -> (ToolCall, Option<Error>) {
        call_tool(tool_call).await
    }
}

pub async fn call_tools(tool_calls: Vec<ToolCall>) -> (Vec<ToolCall>, Vec<Error>) {
    call_tools_dyn(tool_calls, &RawToolCaller).await
}

pub fn retrieve_tool_messages(mut tool_calls: Vec<ToolCall>) -> Vec<Message> {
    let mut tool_messages = Vec::new();
    for tool_call in tool_calls.drain(..) {
        let content = render_tool_result(tool_call.result.as_deref().unwrap_or(&vec![]));
        tool_messages.push(Message::Tool {
            tool_call_id: tool_call.id,
            content: ArcStr::from(content),
        });
    }
    tool_messages
}

pub async fn run_once(session_key: SessionKey, session: &Session, round: usize, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>, tokens_usage: &AtomicU64) -> Result<bool> {
    let mut res = send_messages_with_limit(session_key.clone(), session, messages, tools, &CompressTokenReducer, tokens_usage).await?;
    // 保存回复的消息
    session.context_append(vec![retrieve_assistant_message(&mut res)]).await;
    // 遍历执行 tool call
    let mut next = false;
    let errors = if let Some(tool_calls) = res.tool_calls.take() {
        next = !tool_calls.is_empty();
        let (tool_calls, errors) = call_tools(tool_calls).await;
        let tool_messages = retrieve_tool_messages(tool_calls);
        session.context_append(tool_messages).await;
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
    Ok(next)
}

/// 单个内容块渲染为提示词文本：文本块取原文，其余块给紧凑占位（不把 base64 塞进上下文）
fn render_content_block(block: &ContentBlock) -> ArcStr {
    match block {
        ContentBlock::Text { text, .. } => text.clone(),
        ContentBlock::Image { mime_type, .. } => ArcStr::from(format!("[image {}]", mime_type)),
        ContentBlock::Audio { mime_type, .. } => ArcStr::from(format!("[image {}]", mime_type)),
        ContentBlock::ResourceLink { uri, name, .. } => ArcStr::from(format!("[resource_link {} {}]", name, uri)),
        ContentBlock::Resource { resource, .. } => match resource.as_ref() {
            ResourceContents::Text { text, .. } => text.clone(),
            ResourceContents::Blob { uri, mime_type, .. } => ArcStr::from(format!("[resource {} {}]", uri, mime_type.as_deref().unwrap_or_default())),
        },
    }
}

/// 工具结果渲染：MCP content 数组（内容块数组）逐块渲染，字符串原样输出，其余值按 JSON 输出
pub(crate) fn render_tool_result(blocks: &Vec<ContentBlock>) -> ArcStr {
    let result = blocks.iter()
    .map(render_content_block)
    .collect::<Vec<_>>()
    .join("\n");
    ArcStr::from(result)
}

/// Agentic Loop 工具调用轮次上限（防死循环）
const MAX_TOOL_ROUNDS: usize = 10;

pub struct AgenticLoopPipeline {
    session_key: SessionKey,
    session: Arc<Session>,
    load_memory: bool,
    last_total_tokens: AtomicU64,
    pending_contents: Mutex<Vec<Content>>,
}

#[async_trait]
impl ToolCaller for AgenticLoopPipeline {
    async fn call_tool(&self, tool_call: ToolCall) -> (ToolCall, Option<Error>) {
        match &self.session_key {
            SessionKey::Memory(memory_key) => {
                let result = call_tool_with_memory(tool_call, memory_key.as_ref()).await;
                {
                    let mut pending_contents = self.pending_contents.lock().await;
                    pending_contents.push(Content::ToolCall(result.0.clone()));
                    if result.2.is_none() {
                        pending_contents.push(Content::ToolResult(result.0.clone()));
                    }
                }
                (result.1, result.2)
            },
            _ => call_tool(tool_call).await
        }
    }
}

impl AgenticLoopPipeline {
    async fn run_once(&self, round: usize, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<bool> {
        let mut res = if self.load_memory {
            send_messages_with_limit(self.session_key.clone(), self.session.as_ref(), messages, tools, &MemoryRecoverTokenReducer, &self.last_total_tokens).await?
        } else {
            send_messages_with_limit(self.session_key.clone(), self.session.as_ref(), messages, tools, &CompressTokenReducer, &self.last_total_tokens).await?
        };
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
        self.session.context_append(vec![retrieve_assistant_message(&mut res)]).await;
        // 遍历执行 tool call
        let mut next = false;
        let errors = if let Some(tool_calls) = res.tool_calls.take() {
            let (mut tool_calls, errors) = call_tools_dyn(tool_calls, self).await;
            let mut tool_messages = Vec::new();
            for tool_call in tool_calls.drain(..) {
                let content = render_tool_result(tool_call.result.as_deref().unwrap_or(&vec![]));
                tool_messages.push(Message::Tool {
                    tool_call_id: tool_call.id,
                    content: ArcStr::from(content),
                });
                next = true;
            }
            self.session.context_append(tool_messages).await;
            errors
        } else {
            vec![]
        };
        // output channel
        if let Some(content) = res.content.as_ref() {
            if let SessionKey::Memory(mk) = &self.session_key {
                let out_channel = ConfigManager::get().out_channel_config(mk.agent_id.as_str(), mk.role_event.role_name.as_str()).await;
                if let Some(out_channel) = out_channel {
                    let agent_id = mk.agent_id.clone();
                    let role_name = mk.role_event.role_name.clone();
                    let role_event = mk.role_event.clone();
                    Nexus::get().send_outgoing(agent_id, role_name, role_event, out_channel.as_ref(), content.clone()).await;
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
        let mut round = 1 as usize;
        let mut next = self.run_once(round, vec![message], tools).await?;
        while next {
            round = round + 1;
            next = self.run_once(round, vec![], tools).await?;
        }
        Ok(())
    }
}
