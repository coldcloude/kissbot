//! 模型接入：各判定环节的结构化输出调用。
//!
//! 调用方（归档器/定位器）只依赖 [`DecisionModel`]；真实实现 [`LlmModel`] 复用 kissbot-llm 的
//! provider 与按作用域隔离的会话上下文，测试用桩实现替换（见 spec 验证）。

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
use crate::scope::ModelScope;

/// 第 1 轮（分组轮）的一个分组：属于同一 case 的消息（批次内下标，升序）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageGroup {
    pub messages: Vec<usize>,
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
    async fn group(&self, scope: &ModelScope, batch: &str, total: usize, current: Option<&CaseBrief>) -> Result<Vec<MessageGroup>>;

    /// 第 2 轮起逐层判定：并入当层某 case（非叶子继续下潜）/ 在本层新增叶子 case。
    /// 判定为新增时同一次调用就给出 case 名称与摘要（只用于定位的组也一样，新 case 一建出来就有内容）。
    async fn decide_level(&self, scope: &ModelScope, batch: &str, level: &[CaseBrief]) -> Result<LevelDecision>;

    /// 摘要生成：case 直接记录 + 子 case 摘要 → case 摘要
    async fn summarize(&self, scope: &ModelScope, content: &str) -> Result<ArcStr>;
}

// ========== 提示词 ==========

const SYSTEM_PROMPT: &str = "你是记忆 case 归档与定位的判定器。输入是记忆记录文本与候选 case 摘要。\
你只输出一个 JSON 对象，不输出解释、markdown 代码块或其他任何内容。";

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

/// 第 1 轮（分组轮）提示词：给出当前 case 与全部待处理消息，让模型按 case 分组并标记当前 case
pub fn grouping_prompt(batch: &str, current: Option<&CaseBrief>) -> String {
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
待处理消息（每行格式 `[下标] 时间 内容`，下标从 0 开始）：\n{batch}\n\n\
请把这些消息按 case 分组（一组 = 一个独立话题/case）：\n\
- `messages` 是该组消息的下标数组，升序、不重复、不遗漏\n\
- 各组按首个下标升序排列，且合起来正好覆盖全部下标\n\
- {current_rule}\n\
只输出 JSON，例如 {{\"groups\":[{{\"messages\":[0,1],\"current\":true}},{{\"messages\":[2,3],\"current\":false}}]}}",
        current = current,
        batch = batch,
        current_rule = current_rule
    )
}

/// 第 2 轮起逐层判定提示词：当层全部 case 作为候选（定位的组同样用它，`new` 即当层未命中）
pub fn level_prompt(batch: &str, level: &[CaseBrief]) -> String {
    format!(
        "当层 case（id + 摘要）：\n{level}\n\n\
待归并内容（每行格式 `[序号] 时间 内容`）：\n{batch}\n\n\
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

/// 第 1 轮输出：按 case 分组的结果。
///
/// 严格校验分组可拼成完整批次：下标必须唯一、升序、组内不重复、各组按首下标升序，
/// 且合起来正好覆盖 `0..total`——漏掉任何一条都不允许（否则该记录会被水位跳过而丢失）。
pub fn parse_groups(value: &Value, total: usize) -> Result<Vec<MessageGroup>> {
    let groups = value
        .get("groups")
        .and_then(|g| g.as_array())
        .ok_or_else(|| Error::ModelApiError(format!("分组输出缺少 groups: {}", value).into()))?;
    if groups.is_empty() {
        return Err(Error::ModelApiError("分组输出 groups 为空".into()));
    }

    let mut parsed: Vec<MessageGroup> = Vec::with_capacity(groups.len());
    let mut next_expected = 0usize;
    for (index, group) in groups.iter().enumerate() {
        let messages = group
            .get("messages")
            .and_then(|m| m.as_array())
            .ok_or_else(|| Error::ModelApiError(format!("第 {} 组缺少 messages: {}", index, group).into()))?;
        let mut indices = Vec::with_capacity(messages.len());
        for message in messages {
            let value = message
                .as_u64()
                .ok_or_else(|| Error::ModelApiError(format!("第 {} 组的 messages 含非整数下标: {}", index, message).into()))?;
            let value = value as usize;
            if value >= total {
                return Err(Error::ModelApiError(format!(
                    "第 {} 组下标 {} 超出批次范围（共 {} 条）",
                    index, value, total
                ).into()));
            }
            if indices.last().map(|last| value <= *last).unwrap_or(false) {
                return Err(Error::ModelApiError(format!("第 {} 组下标未严格升序: {:?}", index, messages).into()));
            }
            indices.push(value);
        }
        if indices.is_empty() {
            return Err(Error::ModelApiError(format!("第 {} 组没有消息", index).into()));
        }
        // 各组按顺序首尾相接：不漏、不重、不交叉
        if indices[0] != next_expected {
            return Err(Error::ModelApiError(format!(
                "分组未覆盖全部消息：期望第 {} 组从下标 {} 开始，实际为 {:?}",
                index, next_expected, indices
            ).into()));
        }
        next_expected = indices[indices.len() - 1] + 1;
        let is_current = group.get("current").and_then(|c| c.as_bool()).unwrap_or(false);
        parsed.push(MessageGroup { messages: indices, is_current });
    }
    if next_expected != total {
        return Err(Error::ModelApiError(format!(
            "分组未覆盖全部消息：覆盖到下标 {}，批次共 {} 条",
            next_expected, total
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

/// 真实模型实现：按判定作用域（agent + 记忆后缀）隔离会话上下文
pub struct LlmModel {
    provider: ProviderKind,
    llm_cfg: EffectiveLLMConfig,
    model_cfg: EffectiveModelConfig,
    /// 记忆根目录（会话上下文按 `{root}/{agent}/memory-struct-abstract/llm` 落盘）
    root: PathBuf,
    session_max_messages: usize,
    sessions: DashMap<ModelScope, Arc<Session>>,
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

    async fn session(&self, scope: &ModelScope) -> Result<Arc<Session>> {
        if let Some(session) = self.sessions.get(scope) {
            return Ok(session.clone());
        }
        let _guard = self.session_lock.lock().await;
        if let Some(session) = self.sessions.get(scope) {
            return Ok(session.clone());
        }
        let data_dir = self
            .root
            .join(scope.agent_id.as_str())
            .join(crate::config::STRUCT_DIR_NAME)
            .join("llm")
            .to_string_lossy()
            .to_string();
        let session = Arc::new(Session::new(&data_dir, &crate::scope::session_id(&scope.role_event)));
        session.recover_from_cache().await?;
        session.set_system_message(SYSTEM_PROMPT).await;
        self.sessions.insert(scope.clone(), session.clone());
        Ok(session)
    }

    /// 一次判定调用：会话上下文 + 本次提示 → 结构化 JSON
    async fn call(&self, scope: &ModelScope, prompt: &str) -> Result<Value> {
        let session = self.session(scope).await?;
        let mut messages = session.build_context().await;
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
        session
            .context_append(vec![
                Message::User { content: ArcStr::from(prompt) },
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

    async fn trim_session(&self, scope: &ModelScope, session: &Arc<Session>) {
        let context = session.build_context().await;
        let non_system: Vec<Message> = context
            .into_iter()
            .filter(|m| !matches!(m, Message::System { .. }))
            .collect();
        if non_system.len() <= self.session_max_messages {
            return;
        }
        warn!(
            "[abstract] session context of {:?} exceeds {} messages, archive and keep recent",
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
    async fn group(&self, scope: &ModelScope, batch: &str, total: usize, current: Option<&CaseBrief>) -> Result<Vec<MessageGroup>> {
        let value = self.call(scope, &grouping_prompt(batch, current)).await?;
        // 按下标总数严格校验：必须完整覆盖批次，漏掉任何一条都按失败处理（不能让它被水位跳过）
        parse_groups(&value, total)
    }

    async fn decide_level(&self, scope: &ModelScope, batch: &str, level: &[CaseBrief]) -> Result<LevelDecision> {
        let value = self.call(scope, &level_prompt(batch, level)).await?;
        parse_level_decision(&value)
    }

    async fn summarize(&self, scope: &ModelScope, content: &str) -> Result<ArcStr> {
        let value = self.call(scope, &summary_prompt(content)).await?;
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

        // 分组轮：当前 case + 全部待处理消息
        let prompt = grouping_prompt("[0] 2026-09-22 10:00:00 alice: 你好\n[1] 2026-09-22 10:01:00 bot: 你好呀", Some(&candidates[0]));
        assert!(prompt.contains("- e1: 聊了天气"));
        assert!(prompt.contains("\"groups\""));
        assert!(prompt.contains("[0] 2026-09-22 10:00:00 alice: 你好"));
        assert!(prompt.contains("标记 \"current\": true"));
        // 没有当前 case：要求所有组都不得标记 current
        let prompt = grouping_prompt("[0] 内容", None);
        assert!(prompt.contains("还没有当前 case"));
        assert!(prompt.contains("都不得标记 current"));

        let prompt = level_prompt("内容", &candidates);
        assert!(prompt.contains("当层 case"));
        assert!(prompt.contains("\"target\":\"new\""));
        assert_eq!(render_level(&[]), "（当层没有 case）");
    }
}
