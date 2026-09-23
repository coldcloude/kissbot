//! 模型接入：各判定环节的结构化输出调用。
//!
//! 调用方（归档器/定位器）只依赖 [`DecisionModel`]；真实实现 [`LlmModel`] 复用 kissbot-llm 的
//! provider 与按作用域隔离的会话上下文，测试用桩实现替换（见 spec 验证）。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use kissbot_api::EventBrief;
use kissbot_llm::{
    AnthropicProvider, EffectiveLLMConfig, EffectiveModelConfig, Message, ModelResponse,
    OpenAiProvider, Session, call_with_retry, create_anthropic_provider, create_openai_provider,
};
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::warn;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::scope::ScopeId;

/// 第 1 轮（分组轮）的一个分组：属于同一事件的消息（批次内下标，升序）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageGroup {
    pub messages: Vec<usize>,
    /// 是否属于当前事件（该通道的当前话题延续）
    pub is_current: bool,
}

/// 分层归并判定结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LevelDecision {
    /// 在本层新增叶子事件
    New,
    /// 并入当层某事件（非叶子时继续下潜）
    Merge(String),
}

/// 各判定环节的模型接口
#[async_trait]
pub trait DecisionModel: Send + Sync {
    /// 第 1 轮（分组轮）：把待处理消息按事件分组，并标记属于当前事件的组
    async fn group(&self, scope: &ScopeId, batch: &str, total: usize, current: Option<&EventBrief>) -> Result<Vec<MessageGroup>>;

    /// 第 2 轮起分层归并：并入当层某事件（非叶子继续下潜）/ 在本层新增叶子事件
    async fn decide_level(&self, scope: &ScopeId, batch: &str, level: &[EventBrief]) -> Result<LevelDecision>;

    /// 摘要生成：事件直接记录 + 子事件摘要 → 事件摘要
    async fn summarize(&self, scope: &ScopeId, content: &str) -> Result<String>;

    /// 事件定位：新消息 + 候选事件摘要 → 命中事件 / 未命中
    async fn locate(&self, scope: &ScopeId, message: &str, candidates: &[EventBrief]) -> Result<Option<String>>;
}

// ========== 提示词 ==========

const SYSTEM_PROMPT: &str = "你是记忆事件归档与定位的判定器。输入是记忆记录文本与候选事件摘要。\
你只输出一个 JSON 对象，不输出解释、markdown 代码块或其他任何内容。";

fn render_level(level: &[EventBrief]) -> String {
    if level.is_empty() {
        return "（当层没有事件）".to_string();
    }
    level
        .iter()
        .map(|b| format!("- {}: {}", b.event_id, b.summary))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 第 1 轮（分组轮）提示词：给出当前事件与全部待处理消息，让模型按事件分组并标记当前事件
pub fn grouping_prompt(batch: &str, current: Option<&EventBrief>) -> String {
    let current = match current {
        Some(brief) => format!("- {}: {}", brief.event_id, brief.summary),
        None => "（该通道还没有当前事件）".to_string(),
    };
    let current_rule = if current.starts_with("（") {
        "该通道还没有当前事件：所有组都不得标记 current"
    } else {
        "与当前事件属于同一话题（当前事件的延续）的组标记 \"current\": true，其余组标记 false"
    };
    format!(
        "当前事件：\n{current}\n\n\
待处理消息（每行格式 `[下标] 时间 内容`，下标从 0 开始）：\n{batch}\n\n\
请把这些消息按事件分组（一组 = 一个独立话题/事件）：\n\
- `messages` 是该组消息的下标数组，升序、不重复、不遗漏\n\
- 各组按首个下标升序排列，且合起来正好覆盖全部下标\n\
- {current_rule}\n\
只输出 JSON，例如 {{\"groups\":[{{\"messages\":[0,1],\"current\":true}},{{\"messages\":[2,3],\"current\":false}}]}}",
        current = current,
        batch = batch,
        current_rule = current_rule
    )
}

/// 第 2 轮起分层归并提示词：当层全部事件作为候选
pub fn level_prompt(batch: &str, level: &[EventBrief]) -> String {
    format!(
        "当层事件（id + 摘要）：\n{level}\n\n\
待归并内容（每行格式 `[序号] 时间 内容`）：\n{batch}\n\n\
请判断：\n\
- 若应并入某个已有事件，输出 {{\"target\":\"<event_id>\"}}\n\
- 若应在本层新增事件，输出 {{\"target\":\"new\"}}\n\
只输出 JSON。",
        level = render_level(level),
        batch = batch
    )
}

pub fn summary_prompt(content: &str) -> String {
    format!(
        "事件内容（直接记录 + 子事件摘要）：\n{content}\n\n\
请用简洁中文总结这个事件，保留关键人物、话题、结论与时间线索，输出 {{\"summary\":\"...\"}}",
        content = content
    )
}

pub fn locate_prompt(message: &str, candidates: &[EventBrief]) -> String {
    format!(
        "新消息：\n{message}\n\n候选事件（id + 摘要）：\n{candidates}\n\n\
请判断新消息属于哪个事件：\n\
- 命中某个事件，输出 {{\"matched\":true,\"event_id\":\"<event_id>\"}}\n\
- 都不属于，输出 {{\"matched\":false}}\n\
只输出 JSON。",
        message = message,
        candidates = render_level(candidates)
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
    let start = body.find('{').ok_or_else(|| Error::ModelApiError(format!("模型输出不含 JSON 对象: {}", text)))?;
    let end = body.rfind('}').ok_or_else(|| Error::ModelApiError(format!("模型输出 JSON 不完整: {}", text)))?;
    if end < start {
        return Err(Error::ModelApiError(format!("模型输出 JSON 不完整: {}", text)));
    }
    serde_json::from_str(&body[start..=end])
        .map_err(|e| Error::ModelApiError(format!("模型输出 JSON 解析失败: {} ({})", e, text)))
}

/// 第 1 轮输出：按事件分组的结果。
///
/// 严格校验分组可拼成完整批次：下标必须唯一、升序、组内不重复、各组按首下标升序，
/// 且合起来正好覆盖 `0..total`——漏掉任何一条都不允许（否则该记录会被水位跳过而丢失）。
pub fn parse_groups(value: &Value, total: usize) -> Result<Vec<MessageGroup>> {
    let groups = value
        .get("groups")
        .and_then(|g| g.as_array())
        .ok_or_else(|| Error::ModelApiError(format!("分组输出缺少 groups: {}", value)))?;
    if groups.is_empty() {
        return Err(Error::ModelApiError("分组输出 groups 为空".into()));
    }

    let mut parsed: Vec<MessageGroup> = Vec::with_capacity(groups.len());
    let mut next_expected = 0usize;
    for (index, group) in groups.iter().enumerate() {
        let messages = group
            .get("messages")
            .and_then(|m| m.as_array())
            .ok_or_else(|| Error::ModelApiError(format!("第 {} 组缺少 messages: {}", index, group)))?;
        let mut indices = Vec::with_capacity(messages.len());
        for message in messages {
            let value = message
                .as_u64()
                .ok_or_else(|| Error::ModelApiError(format!("第 {} 组的 messages 含非整数下标: {}", index, message)))?;
            let value = value as usize;
            if value >= total {
                return Err(Error::ModelApiError(format!(
                    "第 {} 组下标 {} 超出批次范围（共 {} 条）",
                    index, value, total
                )));
            }
            if indices.last().map(|last| value <= *last).unwrap_or(false) {
                return Err(Error::ModelApiError(format!("第 {} 组下标未严格升序: {:?}", index, messages)));
            }
            indices.push(value);
        }
        if indices.is_empty() {
            return Err(Error::ModelApiError(format!("第 {} 组没有消息", index)));
        }
        // 各组按顺序首尾相接：不漏、不重、不交叉
        if indices[0] != next_expected {
            return Err(Error::ModelApiError(format!(
                "分组未覆盖全部消息：期望第 {} 组从下标 {} 开始，实际为 {:?}",
                index, next_expected, indices
            )));
        }
        next_expected = indices[indices.len() - 1] + 1;
        let is_current = group.get("current").and_then(|c| c.as_bool()).unwrap_or(false);
        parsed.push(MessageGroup { messages: indices, is_current });
    }
    if next_expected != total {
        return Err(Error::ModelApiError(format!(
            "分组未覆盖全部消息：覆盖到下标 {}，批次共 {} 条",
            next_expected, total
        )));
    }
    Ok(parsed)
}

/// 第 2 轮输出：`new` 或当层事件 id
pub fn parse_level_decision(value: &Value) -> Result<LevelDecision> {
    match value.get("target").and_then(|t| t.as_str()) {
        Some("new") => Ok(LevelDecision::New),
        Some(id) if !id.is_empty() => Ok(LevelDecision::Merge(id.to_string())),
        other => Err(Error::ModelApiError(format!("分层归并 target 非法: {:?}", other))),
    }
}

pub fn parse_summary(value: &Value) -> Result<String> {
    value
        .get("summary")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::ModelApiError("摘要输出缺少 summary".into()))
}

pub fn parse_locate(value: &Value) -> Result<Option<String>> {
    let matched = value.get("matched").and_then(|m| m.as_bool()).unwrap_or(false);
    if !matched {
        return Ok(None);
    }
    match value.get("event_id").and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => Ok(Some(id.to_string())),
        _ => Err(Error::ModelApiError("定位命中但缺少 event_id".into())),
    }
}

// ========== LLM 实现 ==========

enum ProviderKind {
    OpenAi(OpenAiProvider),
    Anthropic(AnthropicProvider),
}

/// 真实模型实现：按作用域隔离会话上下文
pub struct LlmModel {
    provider: ProviderKind,
    llm_cfg: EffectiveLLMConfig,
    model_cfg: EffectiveModelConfig,
    /// 记忆根目录（会话上下文按 `{root}/{agent}/memory-struct-abstract/llm` 落盘）
    root: PathBuf,
    session_max_messages: usize,
    sessions: DashMap<ScopeId, Arc<Session>>,
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
                return Err(Error::ModelApiError(format!("未知 provider_type: {}", other)));
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

    async fn session(&self, scope: &ScopeId) -> Result<Arc<Session>> {
        if let Some(session) = self.sessions.get(scope) {
            return Ok(session.clone());
        }
        let _guard = self.session_lock.lock().await;
        if let Some(session) = self.sessions.get(scope) {
            return Ok(session.clone());
        }
        let data_dir = self
            .root
            .join(&scope.agent_id)
            .join(crate::config::STRUCT_DIR_NAME)
            .join("llm")
            .to_string_lossy()
            .to_string();
        let session = Arc::new(Session::new(&data_dir, &scope.session_id()));
        session.recover_from_cache().await?;
        session.set_system_message(SYSTEM_PROMPT.to_string()).await;
        self.sessions.insert(scope.clone(), session.clone());
        Ok(session)
    }

    /// 一次判定调用：会话上下文 + 本次提示 → 结构化 JSON
    async fn call(&self, scope: &ScopeId, prompt: &str) -> Result<Value> {
        let session = self.session(scope).await?;
        let mut messages = session.build_context().await;
        messages.push(Message::User { content: Arc::new(prompt.to_string()) });

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
        session
            .context_append(vec![
                Message::User { content: Arc::new(prompt.to_string()) },
                Message::Assistant {
                    content: response.raw_content.clone(),
                    reasoning_content: response.raw_reasoning_content.clone(),
                    tool_calls: Value::Null,
                },
            ])
            .await;
        self.trim_session(scope, &session).await;

        extract_json(&text)
    }

    async fn trim_session(&self, scope: &ScopeId, session: &Arc<Session>) {
        let context = session.build_context().await;
        let non_system: Vec<Message> = context
            .into_iter()
            .filter(|m| !matches!(m, Message::System { .. }))
            .collect();
        if non_system.len() <= self.session_max_messages {
            return;
        }
        warn!(
            "[abstract] session context of {} exceeds {} messages, archive and keep recent",
            scope, self.session_max_messages
        );
        let keep = non_system[non_system.len() - self.session_max_messages..].to_vec();
        session.context_archive_and_clear_cache_and_reset_messages(keep).await;
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
    async fn group(&self, scope: &ScopeId, batch: &str, total: usize, current: Option<&EventBrief>) -> Result<Vec<MessageGroup>> {
        let value = self.call(scope, &grouping_prompt(batch, current)).await?;
        // 按下标总数严格校验：必须完整覆盖批次，漏掉任何一条都按失败处理（不能让它被水位跳过）
        parse_groups(&value, total)
    }

    async fn decide_level(&self, scope: &ScopeId, batch: &str, level: &[EventBrief]) -> Result<LevelDecision> {
        let value = self.call(scope, &level_prompt(batch, level)).await?;
        parse_level_decision(&value)
    }

    async fn summarize(&self, scope: &ScopeId, content: &str) -> Result<String> {
        let value = self.call(scope, &summary_prompt(content)).await?;
        parse_summary(&value)
    }

    async fn locate(&self, scope: &ScopeId, message: &str, candidates: &[EventBrief]) -> Result<Option<String>> {
        let value = self.call(scope, &locate_prompt(message, candidates)).await?;
        parse_locate(&value)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn brief(id: &str, summary: &str) -> EventBrief {
        EventBrief { event_id: Arc::new(id.into()), summary: Arc::new(summary.into()) }
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
        // 合法：两组、首尾相接、覆盖全部下标
        let groups = parse_groups(&json!({"groups":[
            {"messages":[0,1],"current":true},
            {"messages":[2],"current":false}
        ]}), 3).unwrap();
        assert_eq!(groups, vec![
            MessageGroup { messages: vec![0, 1], is_current: true },
            MessageGroup { messages: vec![2], is_current: false },
        ]);
        // current 缺省为 false
        let groups = parse_groups(&json!({"groups":[{"messages":[0]}]}), 1).unwrap();
        assert!(!groups[0].is_current);

        // 漏掉尾部下标 → 失败（不能让它被水位跳过）
        assert!(parse_groups(&json!({"groups":[{"messages":[0]}]}), 2).is_err());
        // 下标越界
        assert!(parse_groups(&json!({"groups":[{"messages":[0,5]}]}), 2).is_err());
        // 组内未严格升序 / 重复
        assert!(parse_groups(&json!({"groups":[{"messages":[1,0]}]}), 2).is_err());
        assert!(parse_groups(&json!({"groups":[{"messages":[0,0]}]}), 2).is_err());
        // 组间不连续（留洞）与空组
        assert!(parse_groups(&json!({"groups":[{"messages":[0]},{"messages":[2]}]}), 3).is_err());
        assert!(parse_groups(&json!({"groups":[{"messages":[]}]}), 1).is_err());
        // 结构错误
        assert!(parse_groups(&json!({}), 1).is_err());
        assert!(parse_groups(&json!({"groups":[]}), 0).is_err());
        assert!(parse_groups(&json!({"groups":[{"messages":["a"]}]}), 1).is_err());
    }

    #[test]
    fn parse_level_decision_variants() {
        assert_eq!(parse_level_decision(&json!({"target":"new"})).unwrap(), LevelDecision::New);
        assert_eq!(parse_level_decision(&json!({"target":"e12"})).unwrap(), LevelDecision::Merge("e12".into()));
        assert!(parse_level_decision(&json!({"target":""})).is_err());
        assert!(parse_level_decision(&json!({})).is_err());
    }

    #[test]
    fn parse_summary_and_locate() {
        assert_eq!(parse_summary(&json!({"summary":" 摘要 "})).unwrap(), "摘要");
        assert!(parse_summary(&json!({"summary":""})).is_err());
        assert_eq!(parse_locate(&json!({"matched":true,"event_id":"e1"})).unwrap(), Some("e1".into()));
        assert_eq!(parse_locate(&json!({"matched":false})).unwrap(), None);
        assert!(parse_locate(&json!({"matched":true})).is_err());
    }

    #[test]
    fn prompts_include_context_and_require_json() {
        let candidates = vec![brief("e1", "聊了天气"), brief("e2", "聊了工作")];

        // 分组轮：当前事件 + 全部待处理消息
        let prompt = grouping_prompt("[0] 2026-09-22 10:00:00 alice: 你好\n[1] 2026-09-22 10:01:00 bot: 你好呀", Some(&candidates[0]));
        assert!(prompt.contains("- e1: 聊了天气"));
        assert!(prompt.contains("\"groups\""));
        assert!(prompt.contains("[0] 2026-09-22 10:00:00 alice: 你好"));
        assert!(prompt.contains("标记 \"current\": true"));
        // 没有当前事件：要求所有组都不得标记 current
        let prompt = grouping_prompt("[0] 内容", None);
        assert!(prompt.contains("还没有当前事件"));
        assert!(prompt.contains("都不得标记 current"));

        let prompt = level_prompt("内容", &candidates);
        assert!(prompt.contains("当层事件"));
        assert!(prompt.contains("\"target\":\"new\""));
        assert_eq!(render_level(&[]), "（当层没有事件）");

        let prompt = locate_prompt("新消息正文", &candidates);
        assert!(prompt.contains("新消息正文"));
        assert!(prompt.contains("matched"));
    }
}
