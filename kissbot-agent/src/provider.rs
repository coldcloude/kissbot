use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::config_manager::{EffectiveModelConfig, ToolConfig};
use crate::types::{Error, Message, ModelResponse, Result, ToolCall};

/// Provider 抽象：负责向模型服务商发一次请求并解析响应
#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[Message], tools: &[ToolConfig]) -> Result<ModelResponse>;
    /// 从服务商 API 获取全部可用模型名（GET /models）
    /// 本期只落位：后续任务由管理 API（GET /models）消费
    #[allow(dead_code)]
    async fn list_models(&self) -> Result<Vec<String>>;
    /// provider_type 标识（"openai" | "anthropic"），默认空串，供分发测试与运行时识别
    #[allow(dead_code)]
    fn provider_type(&self) -> &str { "" }
}

/// 按 provider_type 构造 Provider 实现（"openai" | "anthropic"）
/// provider_type 来自 nexus.json / 管理 API（自由字符串，不做前置校验），
/// 未知类型返回 Err（不 panic），由调用方优雅降级（如 no-model 态静默忽略）
pub fn provider_for(client: Arc<reqwest::Client>, provider_type: &str, base_url: &str, api_key: &str) -> Result<Box<dyn Provider>> {
    match provider_type {
        "openai" => Ok(Box::new(OpenAiProvider::new(client, base_url, api_key))),
        "anthropic" => Ok(Box::new(AnthropicProvider::new(client, base_url, api_key))),
        _ => Err(Error::ModelProviderNotSupported(format!("未知 provider_type: {}", provider_type))),
    }
}

// ========== OpenAI 兼容协议（/chat/completions） ==========

pub struct OpenAiProvider {
    client: Arc<reqwest::Client>,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(client: Arc<reqwest::Client>, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
        }
    }
}

fn openai_body(effective: &EffectiveModelConfig, messages: &[Message], tools: &[ToolConfig]) -> serde_json::Value {
    // Message 序列化即 OpenAI 格式（role 平级内部标签 + ToolCall wire 形状 + reasoning_content 字段自动生成/解析）；
    // reasoning_content 工具调用场景必须回传（DeepSeek 带 tools 请求须完整回传否则 400 / Kimi 保留式思考），
    // 上下文已保留 model_resp.reasoning_content，此处直接序列化无需处理
    let msgs: Vec<serde_json::Value> = messages.iter().map(|m| {
        serde_json::to_value(m).expect("Message 序列化不可失败（role 平级标签 + Arc 字段恒可序列化）")
    }).collect();
    let mut body = json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
        "stream": false,
    });
    // tools：非空才发送（工具定义数组，供 LLM 调用）
    if !tools.is_empty() {
        body["tools"] = json!(tools.iter().map(|t| json!({
            "type": "function",
            "function": { "name": t.name, "description": t.description, "parameters": t.parameters },
        })).collect::<Vec<_>>());
    }
    // 可选参数：有值才传（temperature / thinking / reasoning_effort）
    if let Some(t) = effective.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = &effective.thinking {
        body["thinking"] = json!({ "type": t });
    }
    if let Some(e) = &effective.reasoning_effort {
        body["reasoning_effort"] = json!(e);
    }
    body
}

fn parse_openai_response(data: &serde_json::Value) -> ModelResponse {
    let choice = &data["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("").to_string();
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("").to_string();
    let reasoning_content = choice["message"]["reasoning_content"].as_str().unwrap_or("").to_string();
    // <think> 标签总剥离；thinking 独立取标签内容，空串视为 None
    let (content, thinking) = strip_think_tag(content);
    // tool_calls：OpenAI function call 数组（含 thinking 模式下多轮工具调用）；无则空
    // 直接用 ToolCall 自定义反序列化解析 wire 形状（容错：缺字段/格式异常回退空）
    let tool_calls = match choice["message"]["tool_calls"].clone() {
        serde_json::Value::Null => Vec::new(),
        v => serde_json::from_value::<Vec<Arc<ToolCall>>>(v).unwrap_or_default(),
    };
    ModelResponse {
        content: Arc::new(content),
        reasoning_content: Arc::new(reasoning_content),
        thinking: Arc::new(thinking),
        tool_calls: tool_calls,
        finish_reason: Arc::new(finish_reason),
    }
}

/// 匹配 content 开头的 <think>...</think>（允许前导空白），剥离并返回 (剥离后内容, Option<思考内容>)
/// 标签不在开头或未闭合时原样返回
fn strip_think_tag(content: String) -> (String, String) {
    let trimmed = content.as_str().trim_start();
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            let thinking = rest[..end].to_string();
            let stripped = rest[end + "</think>".len()..].to_string();
            return (stripped, thinking);
        }
    }
    (content, String::new())
}

/// 从 OpenAI /models 响应中提取模型 id 列表（测试用解析函数，与网络解耦）
#[allow(dead_code)]
fn parse_openai_models(data: &serde_json::Value) -> Vec<String> {
    data["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default()
}

#[async_trait]
impl Provider for OpenAiProvider {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[Message], tools: &[ToolConfig]) -> Result<ModelResponse> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url)
            .timeout(Duration::from_secs(effective.timeout_secs))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&openai_body(effective, messages, tools))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("OpenAI API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(parse_openai_response(&data))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let resp = self.client.get(&url)
            .timeout(Duration::from_secs(30))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("OpenAI models API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(parse_openai_models(&data))
    }

    fn provider_type(&self) -> &str { "openai" }
}

// ========== Anthropic 协议（/v1/messages） ==========

pub struct AnthropicProvider {
    client: Arc<reqwest::Client>,
    base_url: String,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(client: Arc<reqwest::Client>, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
        }
    }
}

fn anthropic_body(effective: &EffectiveModelConfig, messages: &[Message], _tools: &[ToolConfig]) -> serde_json::Value {
    // 分离 system 消息
    let system_parts: Vec<String> = messages.iter()
        .filter_map(|m| match m {
            Message::System { content } => Some(content.to_string()),
            _ => None,
        })
        .collect();
    let system = system_parts.join("\n");

    let msgs: Vec<serde_json::Value> = messages.iter().filter_map(|m| match m {
        Message::System { .. } => None,
        Message::User { content } => Some(json!({ "role": "user", "content": content })),
        Message::Assistant { content, .. } => Some(json!({ "role": "assistant", "content": content })),
        Message::Tool { .. } => None,  // 本轮不支持工具消息
    }).collect();

    let mut body = json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    // 可选参数：有值才传（temperature / thinking / output_config.effort）
    if let Some(t) = effective.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = &effective.thinking {
        body["thinking"] = json!({ "type": t });
    }
    if let Some(e) = &effective.reasoning_effort {
        body["output_config"] = json!({ "effort": e });
    }
    body
}

fn parse_anthropic_response(data: &serde_json::Value) -> ModelResponse {
    // reasoning_content：thinking block 内容（空串视为 None）
    let mut reasoning_content = String::new();
    let mut content = String::new();
    if let Some(blocks) = data["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("thinking") if reasoning_content.is_empty() => {
                    reasoning_content = block["thinking"].as_str().unwrap_or("").to_string();
                }
                Some("text") if content.is_empty() => {
                    content = block["text"].as_str().unwrap_or("").to_string();
                }
                _ => {}
            }
        }
    }
    let finish_reason = data["stop_reason"].as_str().unwrap_or("").to_string();
    // <think> 标签总剥离；thinking 独立取标签内容
    let (content, thinking) = strip_think_tag(content);
    ModelResponse {
        content: Arc::new(content),
        reasoning_content: Arc::new(reasoning_content),
        thinking: Arc::new(thinking),
        tool_calls: Vec::new(),
        finish_reason: Arc::new(finish_reason),
    }
}

/// 从 Anthropic /v1/models 响应中提取模型 id 列表（测试用解析函数，与网络解耦）
#[allow(dead_code)]
fn parse_anthropic_models(data: &serde_json::Value) -> Vec<String> {
    data["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default()
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[Message], tools: &[ToolConfig]) -> Result<ModelResponse> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url)
            .timeout(Duration::from_secs(effective.timeout_secs))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&anthropic_body(effective, messages, tools))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("Anthropic API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(parse_anthropic_response(&data))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let resp = self.client.get(&url)
            .timeout(Duration::from_secs(30))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("Anthropic models API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(parse_anthropic_models(&data))
    }

    fn provider_type(&self) -> &str { "anthropic" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_effective() -> EffectiveModelConfig {
        EffectiveModelConfig {
            provider_type: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "sk-test".into(),
            model: "deepseek-4-flash".into(),
            max_tokens: 2048,
            temperature: Some(0.3),
            timeout_secs: 30,
            retry_count: 2,
            context_length: 65536,
            max_context_messages: 100,
            thinking: None,
            reasoning_effort: None,
        }
    }

    // Message 构造测试助手（字段为 Arc<String>，集中构造减少噪音）
    fn sys(content: &str) -> Message {
        Message::System { content: Arc::new(content.into()) }
    }

    fn usr(content: &str) -> Message {
        Message::User { content: Arc::new(content.into()) }
    }

    #[test]
    fn openai_body_includes_params_and_messages() {
        let eff = sample_effective();
        let msgs = vec![sys("你是助手"), usr("你好")];
        let body = openai_body(&eff, &msgs, &[]);
        assert_eq!(body["model"], "deepseek-4-flash");
        assert_eq!(body["max_tokens"], 2048);
        // temperature 为 f32，序列化为 f64 表示，用 f32 精确值比较
        assert_eq!(body["temperature"], 0.3_f32 as f64);
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "你好");
    }

    #[test]
    fn openai_body_omits_optional_params_when_none() {
        let mut eff = sample_effective();
        eff.temperature = None;
        eff.thinking = None;
        eff.reasoning_effort = None;
        let msgs = vec![usr("你好")];
        let body = openai_body(&eff, &msgs, &[]);
        assert!(body.get("temperature").is_none(), "temperature 未配置不应传");
        assert!(body.get("thinking").is_none(), "thinking 未配置不应传");
        assert!(body.get("reasoning_effort").is_none(), "reasoning_effort 未配置不应传");
        assert_eq!(body["model"], "deepseek-4-flash");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn openai_body_passes_thinking_and_reasoning_effort() {
        let mut eff = sample_effective();
        eff.thinking = Some("enabled".into());
        eff.reasoning_effort = Some("high".into());
        let msgs = vec![usr("你好")];
        let body = openai_body(&eff, &msgs, &[]);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["temperature"], 0.3_f32 as f64);
    }

    #[test]
    fn openai_body_includes_tools_when_present() {
        let eff = sample_effective();
        let msgs = vec![usr("查一下")];
        let tools = vec![ToolConfig {
            name: Arc::new("read".into()),
            description: Arc::new("读取文本文件".into()),
            parameters: Arc::new(serde_json::json!({ "type": "object" })),
        }];
        let body = openai_body(&eff, &msgs, &tools);
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn openai_body_omits_tools_when_empty() {
        let eff = sample_effective();
        let msgs = vec![usr("你好")];
        let body = openai_body(&eff, &msgs, &[]);
        assert!(body.get("tools").is_none(), "无工具不应发送 tools 字段");
    }

    #[test]
    fn openai_body_maps_tool_and_assistant_tool_calls() {
        let eff = sample_effective();
        let msgs = vec![
            Message::Assistant {
                content: Arc::new(String::new()),
                reasoning_content: None,
                tool_calls: Some(vec![Arc::new(ToolCall { id: Arc::new("c1".into()), name: Arc::new("read".into()), arguments: Arc::new(serde_json::json!({"path": "/a"})) })]),
            },
            Message::Tool { tool_call_id: Arc::new("c1".into()), name: Arc::new("read".into()), content: Arc::new("内容".into()) },
        ];
        let body = openai_body(&eff, &msgs, &[]);
        assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "c1");
        assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["name"], "read");
        assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["arguments"], r#"{"path":"/a"}"#, "arguments 序列化为 JSON 字符串");
        assert_eq!(body["messages"][1]["role"], "tool");
        assert_eq!(body["messages"][1]["tool_call_id"], "c1");
        // 输入 assistant 的 reasoning_content 为 None（上下文保留与否由 coordinator 策略决定），
        // 此处由 skip_serializing_if 省略；若 Some（工具调用场景须回传）则自动序列化携带
        assert!(body["messages"][0].get("reasoning_content").is_none());
    }

    #[test]
    fn openai_body_serializes_reasoning_content_when_present() {
        // 格式能力：Message 序列化即 OpenAI 格式，reasoning_content 由格式自动序列化携带；
        // 上下文已保留 model_resp.reasoning_content（工具调用场景须回传，见 coordinator 步骤 4/6），wire 直接携带
        let eff = sample_effective();
        let msgs = vec![
            Message::System { content: Arc::new("设定".into()) },
            Message::Assistant { content: Arc::new("回答".into()), reasoning_content: Some(Arc::new("思考".into())), tool_calls: None },
        ];
        let body = openai_body(&eff, &msgs, &[]);
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][1]["reasoning_content"], "思考", "格式自动序列化 reasoning_content");
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["messages"][1]["content"], "回答");
    }

    #[test]
    fn parse_openai_response_extracts_content_and_finish_reason() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content.as_str(), "答案");
        assert_eq!(resp.finish_reason.as_str(), "stop");
    }

    #[test]
    fn parse_openai_response_extracts_tool_calls() {
        let data = serde_json::json!({
            "choices": [{
                "message": { "content": null, "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{\"path\":\"/a\"}" } }] },
                "finish_reason": "tool_calls"
            }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id.as_str(), "c1");
        assert_eq!(resp.tool_calls[0].name.as_str(), "read");
        assert_eq!(resp.tool_calls[0].arguments["path"], "/a", "arguments 解析为 JSON 对象");
        assert_eq!(resp.finish_reason.as_str(), "tool_calls");
    }

    #[test]
    fn parse_openai_response_no_tool_calls_by_default() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert!(resp.tool_calls.is_empty(), "无 tool_calls 字段时为空");
    }

    #[test]
    fn anthropic_body_separates_system_messages() {
        let eff = sample_effective();
        let msgs = vec![sys("设定"), usr("hi")];
        let body = anthropic_body(&eff, &msgs, &[]);
        assert_eq!(body["system"], "设定");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1, "system 不应出现在 messages");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn anthropic_body_omits_optional_params_when_none() {
        let mut eff = sample_effective();
        eff.temperature = None;
        eff.thinking = None;
        eff.reasoning_effort = None;
        let msgs = vec![usr("hi")];
        let body = anthropic_body(&eff, &msgs, &[]);
        assert!(body.get("temperature").is_none(), "temperature 未配置不应传");
        assert!(body.get("thinking").is_none(), "thinking 未配置不应传");
        assert!(body.get("output_config").is_none(), "reasoning_effort 未配置不应传 output_config");
    }

    #[test]
    fn anthropic_body_passes_thinking_and_output_config() {
        let mut eff = sample_effective();
        eff.thinking = Some("enabled".into());
        eff.reasoning_effort = Some("high".into());
        let msgs = vec![usr("hi")];
        let body = anthropic_body(&eff, &msgs, &[]);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["temperature"], 0.3_f32 as f64);
    }

    #[test]
    fn parse_anthropic_response_extracts_text_and_stop_reason() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "答复" }],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content.as_str(), "答复");
        assert_eq!(resp.finish_reason.as_str(), "end_turn");
    }

    #[test]
    fn parse_openai_models_extracts_ids() {
        let data = serde_json::json!({ "data": [ { "id": "deepseek-chat" }, { "id": "deepseek-reasoner" } ] });
        assert_eq!(parse_openai_models(&data), vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()]);
    }

    #[test]
    fn strip_think_tag_extracts_and_removes_leading_tag() {
        assert_eq!(strip_think_tag("<think>让我想想</think>答案".to_string()), ("答案".to_string(), "让我想想".to_string()));
    }

    #[test]
    fn strip_think_tag_keeps_non_leading_tag() {
        let content = "答案<think>思考</think>".to_string();
        assert_eq!(strip_think_tag(content.clone()), (content, String::new()));
    }

    #[test]
    fn strip_think_tag_allows_leading_whitespace() {
        assert_eq!(strip_think_tag("\n<think>思考</think>答案".to_string()), ("答案".to_string(), "思考".to_string()));
    }

    #[test]
    fn strip_think_tag_returns_unchanged_when_no_tag() {
        assert_eq!(strip_think_tag("普通文本".to_string()), ("普通文本".to_string(), "".to_string()));
        assert_eq!(strip_think_tag("".to_string()), ("".to_string(), "".to_string()));
    }

    #[test]
    fn strip_think_tag_keeps_unclosed_tag() {
        let content = "<think>未闭合".to_string();
        assert_eq!(strip_think_tag(content.to_string()), (content.to_string(), String::new()));
    }

    #[test]
    fn parse_openai_response_reasoning_and_thinking_independent() {
        // API 有 reasoning_content + content 有 <think> 标签 -> 两字段都 Some（独立共存）
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>标签思考</think>答案", "reasoning_content": "API推理" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content.as_str(), "答案", "<think> 标签应剥离");
        assert_eq!(resp.reasoning_content.as_str(), "API推理", "reasoning_content 独立取 API 字段");
        assert_eq!(resp.thinking.as_str(), "标签思考", "thinking 独立取标签内容");
    }

    #[test]
    fn parse_openai_response_only_thinking_when_no_api_field() {
        // 无 API reasoning_content + <think> 标签 -> reasoning_content=None, thinking=Some
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>思考</think>答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.reasoning_content.as_str(), "");
        assert_eq!(resp.thinking.as_str(), "思考");
    }

    #[test]
    fn parse_openai_response_extracts_reasoning_content() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案", "reasoning_content": "思考" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content.as_str(), "答案");
        assert_eq!(resp.reasoning_content.as_str(), "思考");
        assert_eq!(resp.thinking.as_str(), "", "仅 API 字段无标签时 thinking 应为 None");
    }

    #[test]
    fn parse_openai_response_falls_back_to_think_tag() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>思考</think>答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content.as_str(), "答案", "<think> 标签应剥离");
        assert_eq!(resp.reasoning_content.as_str(), "", "标签内容不再合并到 reasoning_content");
        assert_eq!(resp.thinking.as_str(), "思考", "标签内容独立取 thinking");
    }

    #[test]
    fn parse_openai_response_empty_api_reasoning_falls_back_to_think_tag() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>思考</think>答案", "reasoning_content": "" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content.as_str(), "答案", "<think> 标签应剥离");
        assert_eq!(resp.reasoning_content.as_str(), "", "空字符串 reasoning_content 应视为 None");
        assert_eq!(resp.thinking.as_str(), "思考", "标签内容独立取 thinking");
    }

    #[test]
    fn parse_response_no_thinking_when_both_empty() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.reasoning_content.as_str(), "");
        assert_eq!(resp.thinking.as_str(), "");
    }

    #[test]
    fn parse_anthropic_response_reasoning_and_thinking_independent() {
        let data = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "API推理" },
                { "type": "text", "text": "<think>标签思考</think>答复" }
            ],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content.as_str(), "答复");
        assert_eq!(resp.reasoning_content.as_str(), "API推理");
        assert_eq!(resp.thinking.as_str(), "标签思考");
    }

    #[test]
    fn parse_anthropic_response_extracts_thinking_block() {
        let data = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "思考过程" },
                { "type": "text", "text": "答复" }
            ],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content.as_str(), "答复");
        assert_eq!(resp.reasoning_content.as_str(), "思考过程");
        assert_eq!(resp.thinking.as_str(), "", "无标签时 thinking 应为 None");
    }

    #[test]
    fn parse_anthropic_response_falls_back_to_think_tag() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "<think>思考</think>答复" }],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content.as_str(), "答复");
        assert_eq!(resp.reasoning_content.as_str(), "", "标签内容不再合并到 reasoning_content");
        assert_eq!(resp.thinking.as_str(), "思考", "标签内容独立取 thinking");
    }

    #[test]
    fn parse_anthropic_response_empty_thinking_block_falls_back_to_think_tag() {
        let data = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": "<think>思考</think>答复" }
            ],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content.as_str(), "答复", "<think> 标签应剥离");
        assert_eq!(resp.reasoning_content.as_str(), "", "空字符串 thinking 块应视为 None");
        assert_eq!(resp.thinking.as_str(), "思考", "标签内容独立取 thinking");
    }

    #[test]
    fn parse_anthropic_models_extracts_ids() {
        let data = serde_json::json!({ "data": [ { "id": "claude-3-5" } ] });
        assert_eq!(parse_anthropic_models(&data), vec!["claude-3-5".to_string()]);
    }

    #[test]
    fn provider_for_dispatches_by_type() {
        let client = Arc::new(reqwest::Client::new());
        assert_eq!(provider_for(client.clone(), "openai", "u", "k").unwrap().provider_type(), "openai");
        assert_eq!(provider_for(client, "anthropic", "u", "k").unwrap().provider_type(), "anthropic");
    }

    #[test]
    fn provider_for_unknown_type_returns_err() {
        let client = Arc::new(reqwest::Client::new());
        let err = provider_for(client, "typo", "u", "k").err().expect("未知 provider_type 应返回 Err");
        assert!(matches!(err, Error::ModelProviderNotSupported(_)), "未知类型应返回 ModelProviderNotSupported");
        assert!(err.to_string().contains("未知 provider_type: typo"), "错误信息应指明未知类型");
    }
}
