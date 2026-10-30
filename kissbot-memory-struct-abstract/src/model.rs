//! 模型接入：各判定环节的结构化输出调用。
//!
//! 调用方（归档器/定位器）只依赖 [`DecisionModel`]；真实实现 [`LlmModel`] 复用 kissbot-llm 的
//! provider 与按「角色 + event_id」隔离的会话上下文，测试用桩实现替换（见 spec 验证）。
//!
//! 判定的两个参数是：`agent_role`（角色，决定 case 树与会话所属 agent）与
//! `event_id`（`None` 为角色模式，`Some` 为事件模式）；不存在单独的模型归属结构。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use arcstr::ArcStr;
use async_trait::async_trait;
use dashmap::DashMap;
use kissbot_api::CaseBrief;
use kissbot_llm::{
    AnthropicProvider, EffectiveLLMConfig, EffectiveModelConfig, Message, ModelResponse,
    OpenAiProvider, Session, call_with_retry, create_anthropic_provider, create_openai_provider,
};
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::warn;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::agent_role::{AgentRole, session_id, role_event_of};

/// 分组输入的一条原文消息：一条通道消息一条
///
/// 思考/工具不是独立消息：它们是所属消息 `Multi` 内容的一部分，已按 `<think>` / `<reasoning_content>` /
/// `<tool_call>` / `<tool_result>` 标签内嵌到 `content` 里，整条消息只占一个条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupItem {
    /// 分组标识（消息的 `msg_id`；历史记录没有 msg_id 时由归档器给兜底标识），批次内唯一
    pub msg_id: ArcStr,
    pub time: ArcStr,
    /// 发送者身份
    pub user_id: ArcStr,
    /// 原文（思考/工具已内嵌）
    pub content: ArcStr,
}

/// 第 1 轮（分组轮）的一个分组：属于同一 case 的消息标识
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageGroup {
    /// 组内消息的 `msg_id`（集合语义：组内顺序不构成约定）
    pub msg_ids: Vec<ArcStr>,
    /// 是否属于当前 case（该通道的当前话题延续）
    pub is_current: bool,
}

/// 分层归并判定结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LevelDecision {
    /// 在本层新增叶子 case：模型同一次调用就给出 case 名称与摘要（不等到后续再调用模型）
    New {
        /// case 名称：只能由英文字母、数字与 `-` 组成
        name: ArcStr,
        /// case 摘要
        summary: ArcStr,
    },
    /// 并入当层某 case（非叶子时继续下潜）
    Merge(ArcStr),
}

impl LevelDecision {
    /// 便捷构造：新建名为 `name`、摘要为 `summary` 的 case（测试与桩模型用）
    pub fn new_case(name: &str, summary: &str) -> Self {
        Self::New { name: ArcStr::from(name), summary: ArcStr::from(summary) }
    }
}

/// case 名称是否合法：非空、长度不超过 [`MAX_CASE_NAME_LEN`]，且只含英文字母、数字与 `-`
pub fn valid_case_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CASE_NAME_LEN
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// case 名称长度上限（避免超长目录名）
pub const MAX_CASE_NAME_LEN: usize = 64;

/// 各判定环节的模型接口
#[async_trait]
pub trait DecisionModel: Send + Sync {
    /// 第 1 轮（分组轮）：把待处理消息按 case 分组，并标记属于当前 case 的组
    async fn group(
        &self,
        agent_role: &AgentRole,
        event_id: Option<ArcStr>,
        items: &[GroupItem],
        current: Option<&CaseBrief>,
    ) -> Result<Vec<MessageGroup>>;

    /// 第 2 轮起逐层判定：并入当层某 case（非叶子继续下潜）/ 在本层新增叶子 case。
    /// 判定为新增时同一次调用就给出 case 名称与摘要（只用于定位的组也一样，新 case 一建出来就有内容）。
    async fn decide_level(
        &self,
        agent_role: &AgentRole,
        event_id: Option<ArcStr>,
        batch: &str,
        level: &[CaseBrief],
    ) -> Result<LevelDecision>;

    /// 摘要生成：case 直接记录 + 子 case 摘要 → case 摘要
    async fn summarize(&self, agent_role: &AgentRole, event_id: Option<ArcStr>, content: &str) -> Result<ArcStr>;
}

// ========== 提示词 ==========

const SYSTEM_PROMPT: ArcStr = arcstr::literal!("你是记忆 case 归档与定位的判定器。输入是记忆记录（固定格式 JSON 或文本）与候选 case 摘要。\
你只输出一个 JSON 对象，不输出解释、markdown 代码块或其他任何内容。");

fn render_level(level: &[CaseBrief]) -> String {
    if level.is_empty() {
        return "（当层没有 case）".to_string();
    }
    level
        .iter()
        .map(|b| format!("- {}: {}", b.case_name, b.summary))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 分组输入：固定格式的 JSON 数组，每项四个字段 `msg_id` / `time` / `user_id` / `content`
pub fn grouping_batch(items: &[GroupItem]) -> String {
    let messages: Vec<Value> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "msg_id": item.msg_id,
                "time": item.time,
                "user_id": item.user_id,
                "content": item.content,
            })
        })
        .collect();
    serde_json::to_string(&messages).unwrap_or_else(|_| String::from("[]"))
}

/// 第 1 轮（分组轮）提示词：写明输入格式、分组操作与输出格式，要求按 `msg_id` 给出每组的原文列表
pub fn grouping_prompt(items: &[GroupItem], current: Option<&CaseBrief>) -> String {
    let current = match current {
        Some(brief) => format!("- {}: {}", brief.case_name, brief.summary),
        None => "（该通道还没有当前 case）".to_string(),
    };
    let current_rule = if current.starts_with("（") {
        "该通道还没有当前 case：所有组都不得标记 current"
    } else {
        "与当前 case 属于同一话题（当前 case 的延续）的组标记 \"current\": true，其余组标记 false"
    };
    format!(
        "当前 case：\n{current}\n\n\
输入格式：`messages` 是 JSON 数组，每项固定四个字段：\
`msg_id`（消息标识）、`time`（时间）、`user_id`（发送者）、`content`（原文，字符串）。\
思考与工具不是独立条目：它们已按顺序内嵌在所属消息的 `content` 里，\
分别用 <think>…</think>、<reasoning_content>…</reasoning_content>、<tool_call>…</tool_call>、\
<tool_result>…</tool_result> 标签包裹，每条消息一行、一个 `msg_id`。\n\
messages：\n{messages}\n\n\
分组操作：\n\
- 按 case 分组：一个 case 是一个独立话题，同一话题的消息即使中间夹着别的话题也要放进同一组；\n\
- 每条消息必须且只能属于一个组（不重不漏），组内顺序没有意义；\n\
- 每组输出该组全部消息的 `msg_id` 列表，`msg_id` 原样照抄，不要改写、不要用下标代替；\n\
- {current_rule}。\n\n\
输出格式：只输出一个 JSON 对象，不要解释、不要 markdown 代码块：\n\
{{\"groups\":[{{\"msg_ids\":[\"<msg_id>\",\"<msg_id>\"],\"current\":true}},{{\"msg_ids\":[\"<msg_id>\"],\"current\":false}}]}}",
        current = current,
        messages = grouping_batch(items),
        current_rule = current_rule
    )
}

/// 第 2 轮起逐层判定提示词：当层全部 case 作为候选（定位的组同样用它，`new` 即当层未命中）
pub fn level_prompt(batch: &str, level: &[CaseBrief]) -> String {
    format!(
        "当层 case（id + 摘要）：\n{level}\n\n\
待归并内容（每行一条消息，格式 `[时间] 说话人: 原文`；思考/工具内容已内嵌在原文里）：\n{batch}\n\n\
请判断：\n\
- 若应并入某个已有 case，输出 {{\"target\":\"<case_name>\"}}\n\
- 若应在本层新增 case，输出 {{\"target\":\"new\",\"name\":\"<名称>\",\"summary\":\"<摘要>\"}}：\
名称只能由英文字母、数字与 \"-\" 组成（如 topic-weather-chat），不能用中文、空格或其他字符，且要能概括内容；\
摘要用简洁中文概括这个新 case（保留关键人物、话题、结论与时间线索）\n\
只输出 JSON。",
        level = render_level(level),
        batch = batch
    )
}

pub fn summary_prompt(content: &str) -> String {
    format!(
        "case 内容（直接记录 + 子 case 摘要）：\n{content}\n\n\
请用简洁中文总结这个 case，保留关键人物、话题、结论与时间线索，输出 {{\"summary\":\"...\"}}",
        content = content
    )
}

// ========== 结构化输出解析 ==========

/// 从模型输出中提取 JSON 对象（容忍 markdown 代码块与前后解释文字）
pub fn extract_json(text: &str) -> Result<Value> {
    let trimmed = text.trim();
    let body = if let Some(rest) = trimmed.strip_prefix("```") {
        // 去掉首行语言标记与结尾围栏
        let rest = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
        rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    };
    let start = body.find('{').ok_or_else(|| Error::ModelApiError(format!("模型输出不含 JSON 对象: {}", text).into()))?;
    let end = body.rfind('}').ok_or_else(|| Error::ModelApiError(format!("模型输出 JSON 不完整: {}", text).into()))?;
    if end < start {
        return Err(Error::ModelApiError(format!("模型输出 JSON 不完整: {}", text).into()));
    }
    serde_json::from_str(&body[start..=end])
        .map_err(|e| Error::ModelApiError(format!("模型输出 JSON 解析失败: {} ({})", e, text).into()))
}

/// 第 1 轮输出：按 case 分组的结果，每组是该组原文消息的 `msg_id` 列表。
///
/// 严格校验分组可拼成完整批次：`msg_id` 必须都在批次里、每组非空、每个 `msg_id` 恰好出现一次、
/// 且合起来正好覆盖 `expected` 全部标识——漏掉任何一条都不允许（否则该记录会被水位跳过而丢失）。
/// 组之间、组内都不要求顺序（话题消息可以交错），只要求集合覆盖。
pub fn parse_groups(value: &Value, expected: &[ArcStr]) -> Result<Vec<MessageGroup>> {
    let groups = value
        .get("groups")
        .and_then(|g| g.as_array())
        .ok_or_else(|| Error::ModelApiError(format!("分组输出缺少 groups: {}", value).into()))?;
    if groups.is_empty() {
        return Err(Error::ModelApiError("分组输出 groups 为空".into()));
    }

    let expected_set: HashSet<&str> = expected.iter().map(|id| id.as_str()).collect();
    let mut seen: HashSet<&str> = HashSet::with_capacity(expected.len());
    let mut parsed: Vec<MessageGroup> = Vec::with_capacity(groups.len());
    for (index, group) in groups.iter().enumerate() {
        let msg_ids = group
            .get("msg_ids")
            .and_then(|m| m.as_array())
            .ok_or_else(|| Error::ModelApiError(format!("第 {} 组缺少 msg_ids: {}", index, group).into()))?;
        if msg_ids.is_empty() {
            return Err(Error::ModelApiError(format!("第 {} 组没有消息", index).into()));
        }
        let mut ids = Vec::with_capacity(msg_ids.len());
        for msg_id in msg_ids {
            let msg_id = msg_id
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| Error::ModelApiError(format!("第 {} 组的 msg_ids 含非法标识: {}", index, msg_id).into()))?;
            if !expected_set.contains(msg_id) {
                return Err(Error::ModelApiError(format!(
                    "第 {} 组的 msg_id {} 不在本批消息里（批次共 {} 条）",
                    index, msg_id, expected.len()
                ).into()));
            }
            if !seen.insert(msg_id) {
                return Err(Error::ModelApiError(format!(
                    "第 {} 组的 msg_id {} 重复出现（每条消息只能属于一个组）",
                    index, msg_id
                ).into()));
            }
            ids.push(ArcStr::from(msg_id));
        }
        let is_current = group.get("current").and_then(|c| c.as_bool()).unwrap_or(false);
        parsed.push(MessageGroup { msg_ids: ids, is_current });
    }
    let missing: Vec<&str> = expected_set.iter().filter(|id| !seen.contains(**id)).copied().collect();
    if !missing.is_empty() {
        return Err(Error::ModelApiError(format!(
            "分组未覆盖全部消息：漏掉 {} 条（{:?}）",
            missing.len(),
            missing
        ).into()));
    }
    Ok(parsed)
}

/// 第 2 轮输出：当层某 case id，或带名称与摘要的新 case
pub fn parse_level_decision(value: &Value) -> Result<LevelDecision> {
    match value.get("target").and_then(|t| t.as_str()) {
        Some("new") => {
            let name = value
                .get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.trim())
                .filter(|n| valid_case_name(n))
                .ok_or_else(|| {
                    Error::ModelApiError(format!(
                        "新增 case 的 name 非法（只能含英文字母、数字与 '-'，且不超过 {} 字符）: {}",
                        MAX_CASE_NAME_LEN, value
                    ).into())
                })?;
            let summary = parse_summary(value)?;
            Ok(LevelDecision::New { name: ArcStr::from(name), summary })
        }
        Some(id) if !id.is_empty() => Ok(LevelDecision::Merge(ArcStr::from(id))),
        other => Err(Error::ModelApiError(format!("分层归并 target 非法: {:?}", other).into())),
    }
}

pub fn parse_summary(value: &Value) -> Result<ArcStr> {
    value
        .get("summary")
        .and_then(|s| s.as_str())
        .map(|s| ArcStr::from(s.trim()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::ModelApiError("摘要输出缺少 summary".into()))
}

// ========== LLM 实现 ==========

enum ProviderKind {
    OpenAi(OpenAiProvider),
    Anthropic(AnthropicProvider),
}

/// 真实模型实现：会话上下文按「角色 + event_id」（即记忆后缀）隔离
pub struct LlmModel {
    provider: ProviderKind,
    llm_cfg: EffectiveLLMConfig,
    model_cfg: EffectiveModelConfig,
    /// 记忆根目录（会话上下文按 `{root}/{agent}/memory-struct-abstract/llm` 落盘）
    root: PathBuf,
    session_max_messages: usize,
    /// 会话表：键为（角色 + event_id），即一个记忆后缀一个会话
    sessions: DashMap<(Arc<AgentRole>, Option<ArcStr>), Arc<Session>>,
    session_lock: Mutex<()>,
}

impl LlmModel {
    pub fn new(config: &Config) -> Result<Self> {
        let root = config.storage_root();
        let llm_cfg = config.effective_llm();
        let model_cfg = config.effective_model();
        let provider_config = config.provider_config.as_ref();
        let provider = match provider_config.provider_type.as_str() {
            "openai" => ProviderKind::OpenAi(create_openai_provider(provider_config)),
            "anthropic" => ProviderKind::Anthropic(create_anthropic_provider(provider_config)),
            other => {
                return Err(Error::ModelApiError(format!("未知 provider_type: {}", other).into()));
            }
        };
        Ok(Self {
            provider,
            llm_cfg,
            model_cfg,
            root,
            session_max_messages: config.session_max_messages,
            sessions: DashMap::new(),
            session_lock: Mutex::new(()),
        })
    }

    async fn session(&self, agent_role: &AgentRole, event_id: Option<ArcStr>) -> Result<Arc<Session>> {
        let key: (Arc<AgentRole>, Option<ArcStr>) = (Arc::new(agent_role.clone()), event_id.clone());
        if let Some(session) = self.sessions.get(&key) {
            return Ok(session.clone());
        }
        let _guard = self.session_lock.lock().await;
        if let Some(session) = self.sessions.get(&key) {
            return Ok(session.clone());
        }
        let data_dir = self
            .root
            .join(agent_role.agent_id.as_str())
            .join(crate::config::STRUCT_DIR_NAME.as_str())
            .join("llm")
            .to_string_lossy()
            .to_string();
        let session = Arc::new(Session::new(&data_dir, ArcStr::from(session_id(&role_event_of(agent_role, event_id)))));
        session.recover_from_cache().await?;
        session.set_system_message(SYSTEM_PROMPT.clone()).await;
        self.sessions.insert(key, session.clone());
        Ok(session)
    }

    /// 一次判定调用：会话上下文 + 本次提示 → 结构化 JSON
    async fn call(&self, agent_role: &AgentRole, event_id: Option<ArcStr>, prompt: &str) -> Result<Value> {
        let session = self.session(agent_role, event_id.clone()).await?;
        let mut messages = session.build_context_full().await;
        messages.push(Message::User { content: ArcStr::from(prompt) });

        let tools = Vec::new();
        let response = match &self.provider {
            ProviderKind::OpenAi(provider) => {
                call_with_retry(provider, &self.llm_cfg, &self.model_cfg, messages, &tools).await?
            }
            ProviderKind::Anthropic(provider) => {
                call_with_retry(provider, &self.llm_cfg, &self.model_cfg, messages, &tools).await?
            }
        };

        let text = response_text(&response)?;
        // 共享上下文：把本次提示与回答追加回会话，超限时只保留最近的消息
        let _ = session.context_append(vec![
            Message::User { content: ArcStr::from(prompt) },
            Message::Assistant {
                content: response.raw_content.clone(),
                reasoning_content: response.raw_reasoning_content.clone(),
                tool_calls: Value::Null,
            },
        ]).await;
        self.trim_session(agent_role, event_id, &session).await;

        extract_json(&text)
    }

    async fn trim_session(&self, agent_role: &AgentRole, event_id: Option<ArcStr>, session: &Arc<Session>) {
        let context = session.build_context_full().await;
        let non_system: Vec<Message> = context
            .into_iter()
            .filter(|m| !matches!(m, Message::System { .. }))
            .collect();
        if non_system.len() <= self.session_max_messages {
            return;
        }
        warn!(
            "[abstract] session context of {}/{:?} exceeds {} messages, archive and keep recent",
            agent_role, event_id, self.session_max_messages
        );
        let keep = non_system[non_system.len() - self.session_max_messages..].to_vec();
        let _ = session.context_archive_and_clear_cache_and_reset_messages(keep).await;
    }
}

fn response_text(response: &ModelResponse) -> Result<String> {
    if let Some(content) = &response.content {
        if !content.trim().is_empty() {
            return Ok(content.as_str().to_string());
        }
    }
    if let Some(text) = response.raw_content.as_str() {
        if !text.trim().is_empty() {
            return Ok(text.to_string());
        }
    }
    Err(Error::ModelApiError("模型返回空内容".into()))
}

#[async_trait]
impl DecisionModel for LlmModel {
    async fn group(
        &self,
        agent_role: &AgentRole,
        event_id: Option<ArcStr>,
        items: &[GroupItem],
        current: Option<&CaseBrief>,
    ) -> Result<Vec<MessageGroup>> {
        let value = self.call(agent_role, event_id, &grouping_prompt(items, current)).await?;
        // 按批次标识严格校验：必须完整覆盖批次，漏掉任何一条都按失败处理（不能让它被水位跳过）
        let expected: Vec<ArcStr> = items.iter().map(|item| item.msg_id.clone()).collect();
        parse_groups(&value, &expected)
    }

    async fn decide_level(
        &self,
        agent_role: &AgentRole,
        event_id: Option<ArcStr>,
        batch: &str,
        level: &[CaseBrief],
    ) -> Result<LevelDecision> {
        let value = self.call(agent_role, event_id, &level_prompt(batch, level)).await?;
        parse_level_decision(&value)
    }

    async fn summarize(&self, agent_role: &AgentRole, event_id: Option<ArcStr>, content: &str) -> Result<ArcStr> {
        let value = self.call(agent_role, event_id, &summary_prompt(content)).await?;
        parse_summary(&value)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn brief(id: &str, summary: &str) -> CaseBrief {
        CaseBrief { case_name: ArcStr::from(id), summary: ArcStr::from(summary) }
    }

    #[test]
    fn extract_json_handles_fences_and_prose() {
        assert_eq!(extract_json(r#"{"a":1}"#).unwrap()["a"], 1);
        assert_eq!(extract_json("```json\n{\"a\":2}\n```").unwrap()["a"], 2);
        assert_eq!(extract_json("好的，结果是：{\"a\":3} 完毕").unwrap()["a"], 3);
        assert!(extract_json("没有 JSON").is_err());
        assert!(extract_json("{不完整").is_err());
    }

    #[test]
    fn parse_groups_requires_full_coverage() {
        let expected: Vec<ArcStr> = ["m1", "m2", "m3"].into_iter().map(ArcStr::from).collect();
        // 合法：同一话题的消息可以交错（m1/m3 一组、m2 另一组），组内顺序不构成约定
        let groups = parse_groups(&json!({"groups":[
            {"msg_ids":["m3","m1"],"current":true},
            {"msg_ids":["m2"],"current":false}
        ]}), &expected).unwrap();
        assert_eq!(groups, vec![
            MessageGroup { msg_ids: vec![ArcStr::from("m3"), ArcStr::from("m1")], is_current: true },
            MessageGroup { msg_ids: vec![ArcStr::from("m2")], is_current: false },
        ]);
        // current 缺省为 false
        let groups = parse_groups(&json!({"groups":[{"msg_ids":["m1"]}]}), &expected[..1]).unwrap();
        assert!(!groups[0].is_current);

        // 漏掉标识 → 失败（不能让它被水位跳过）
        assert!(parse_groups(&json!({"groups":[{"msg_ids":["m1"]}]}), &expected).is_err());
        // 不在批次里的标识
        assert!(parse_groups(&json!({"groups":[{"msg_ids":["m1","m9"]}]}), &expected[..1]).is_err());
        // 同一标识出现在两个组 / 同一组里重复
        assert!(parse_groups(&json!({"groups":[{"msg_ids":["m1","m3"]},{"msg_ids":["m1"]}]}), &expected).is_err());
        assert!(parse_groups(&json!({"groups":[{"msg_ids":["m1","m1"]}]}), &expected[..1]).is_err());
        // 空组、空标识、用下标代替 msg_id
        assert!(parse_groups(&json!({"groups":[{"msg_ids":[]}]}), &expected[..1]).is_err());
        assert!(parse_groups(&json!({"groups":[{"msg_ids":[""]}]}), &expected[..1]).is_err());
        assert!(parse_groups(&json!({"groups":[{"msg_ids":[0]}]}), &expected[..1]).is_err());
        // 结构错误
        assert!(parse_groups(&json!({}), &expected[..1]).is_err());
        assert!(parse_groups(&json!({"groups":[]}), &[]).is_err());
        assert!(parse_groups(&json!({"groups":[{"messages":["m1"]}]}), &expected[..1]).is_err());
    }

    #[test]
    fn parse_level_decision_variants() {
        // target=new 必须同时给出合法的 name 与 summary
        assert_eq!(
            parse_level_decision(&json!({"target":"new","name":"weather-chat","summary":"聊天气"})).unwrap(),
            LevelDecision::new_case("weather-chat", "聊天气")
        );
        // name 只要求字符集与长度合法：`role-x` 这类字符串也允许
        assert_eq!(
            parse_level_decision(&json!({"target":"new","name":"role-x","summary":"摘要"})).unwrap(),
            LevelDecision::new_case("role-x", "摘要")
        );
        assert_eq!(parse_level_decision(&json!({"target":"e12"})).unwrap(), LevelDecision::Merge("e12".into()));
        assert!(parse_level_decision(&json!({"target":""})).is_err());
        assert!(parse_level_decision(&json!({})).is_err());

        // name 非法：空 / 中文 / 空格 / 其他符号 / 超长
        let too_long = "a".repeat(MAX_CASE_NAME_LEN + 1);
        for name in ["", "天气", "weather chat", "weather_chat", "weather.dot", too_long.as_str()] {
            let value = json!({"target":"new","name":name,"summary":"摘要"});
            assert!(parse_level_decision(&value).is_err(), "name 非法应报错: {:?}", name);
        }
        // 缺 name / 缺 summary / summary 为空白
        assert!(parse_level_decision(&json!({"target":"new","summary":"摘要"})).is_err());
        assert!(parse_level_decision(&json!({"target":"new","name":"ok"})).is_err());
        assert!(parse_level_decision(&json!({"target":"new","name":"ok","summary":"  "})).is_err());

        // 边界：恰好达到长度上限（全为合法字符）可通过
        let max = "a".repeat(MAX_CASE_NAME_LEN);
        assert_eq!(
            parse_level_decision(&json!({"target":"new","name":max,"summary":"摘要"})).unwrap(),
            LevelDecision::new_case(&max, "摘要")
        );
    }

    #[test]
    fn parse_summary_output() {
        assert_eq!(parse_summary(&json!({"summary":" 摘要 "})).unwrap(), "摘要");
        assert!(parse_summary(&json!({"summary":""})).is_err());
    }

    #[test]
    fn prompts_include_context_and_require_json() {
        let candidates = vec![brief("e1", "聊了天气"), brief("e2", "聊了工作")];
        let items = vec![
            item("m1", "2026-09-22 10:00:00", "u1", "你好"),
            item("m2", "2026-09-22 10:01:00", "self", "<reasoning_content>想法</reasoning_content>\n你好呀"),
        ];

        // 分组轮：写明输入格式、分组操作、输出格式，输入是固定字段的 JSON
        let prompt = grouping_prompt(&items, Some(&candidates[0]));
        assert!(prompt.contains("- e1: 聊了天气"));
        assert!(prompt.contains("输入格式") && prompt.contains("分组操作") && prompt.contains("输出格式"));
        assert!(prompt.contains("\"groups\"") && prompt.contains("\"msg_ids\""));
        assert!(prompt.contains("\"msg_id\":\"m1\""));
        assert!(prompt.contains("\"time\":\"2026-09-22 10:00:00\""));
        assert!(prompt.contains("\"user_id\":\"u1\""));
        assert!(prompt.contains("\"content\":\"你好\""));
        assert!(prompt.contains("<reasoning_content>想法</reasoning_content>"), "思考已内嵌在原文里");
        assert!(prompt.contains("标记 \"current\": true"));
        // 没有当前 case：要求所有组都不得标记 current
        let prompt = grouping_prompt(&items[..1], None);
        assert!(prompt.contains("还没有当前 case"));
        assert!(prompt.contains("都不得标记 current"));

        let prompt = level_prompt("内容", &candidates);
        assert!(prompt.contains("当层 case"));
        assert!(prompt.contains("\"target\":\"new\""));
        assert_eq!(render_level(&[]), "（当层没有 case）");
    }

    fn item(msg_id: &str, time: &str, user_id: &str, content: &str) -> GroupItem {
        GroupItem {
            msg_id: ArcStr::from(msg_id),
            time: ArcStr::from(time),
            user_id: ArcStr::from(user_id),
            content: ArcStr::from(content),
        }
    }

    /// 分组输入是固定字段的 JSON 数组：每项恰好 msg_id / time / user_id / content 四个字段
    #[test]
    fn grouping_batch_is_fixed_shape_json() {
        let items = vec![item("m1", "2026-09-22 10:00:00", "u1", "你好"), item("m2", "2026-09-22 10:01:00", "u2", "在吗")];
        let batch: Vec<Value> = serde_json::from_str(&grouping_batch(&items)).unwrap();
        assert_eq!(batch.len(), 2);
        for (index, value) in batch.iter().enumerate() {
            let object = value.as_object().expect("每项是 JSON 对象");
            let mut keys: Vec<&str> = object.keys().map(|key| key.as_str()).collect();
            keys.sort_unstable();
            assert_eq!(keys, vec!["content", "msg_id", "time", "user_id"], "第 {} 项字段固定", index);
        }
        assert_eq!(batch[0]["msg_id"], "m1");
        assert_eq!(batch[0]["user_id"], "u1");
        assert_eq!(batch[0]["content"], "你好");
    }
}
