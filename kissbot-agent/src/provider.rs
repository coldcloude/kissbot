use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::config_manager::EffectiveModelConfig;
use crate::types::{Error, MessageItem, ModelResponse, Result};

/// Provider 抽象：负责向模型服务商发一次请求并解析响应
#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse>;
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

fn openai_body(effective: &EffectiveModelConfig, messages: &[MessageItem]) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages.iter().map(|m| {
        json!({ "role": m.role, "content": m.content })
    }).collect();
    json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
        "temperature": effective.temperature,
        "stream": false,
    })
}

fn parse_openai_response(data: &serde_json::Value) -> ModelResponse {
    let choice = &data["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("").to_string();
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop").to_string();
    ModelResponse { content, tool_calls: Vec::new(), finish_reason }
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
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url)
            .timeout(Duration::from_secs(effective.timeout_secs))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&openai_body(effective, messages))
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

fn anthropic_body(effective: &EffectiveModelConfig, messages: &[MessageItem]) -> serde_json::Value {
    // 分离 system 消息
    let system_parts: Vec<String> = messages.iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.clone())
        .collect();
    let system = system_parts.join("\n");

    let msgs: Vec<serde_json::Value> = messages.iter()
        .filter(|m| m.role != "system")
        .map(|m| json!({
            "role": if m.role == "assistant" { "assistant" } else { "user" },
            "content": m.content,
        }))
        .collect();

    let mut body = json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    body
}

fn parse_anthropic_response(data: &serde_json::Value) -> ModelResponse {
    let content = data["content"][0]["text"].as_str().unwrap_or("").to_string();
    let finish_reason = data["stop_reason"].as_str().unwrap_or("end_turn").to_string();
    ModelResponse { content, tool_calls: Vec::new(), finish_reason }
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
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url)
            .timeout(Duration::from_secs(effective.timeout_secs))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&anthropic_body(effective, messages))
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
            thinking: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn openai_body_includes_params_and_messages() {
        let eff = sample_effective();
        let msgs = vec![
            MessageItem { role: "system".into(), content: "你是助手".into() },
            MessageItem { role: "user".into(), content: "你好".into() },
        ];
        let body = openai_body(&eff, &msgs);
        assert_eq!(body["model"], "deepseek-4-flash");
        assert_eq!(body["max_tokens"], 2048);
        // temperature 为 f32，序列化为 f64 表示，用 f32 精确值比较
        assert_eq!(body["temperature"], 0.3_f32 as f64);
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "你好");
    }

    #[test]
    fn parse_openai_response_extracts_content_and_finish_reason() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content, "答案");
        assert_eq!(resp.finish_reason, "stop");
    }

    #[test]
    fn anthropic_body_separates_system_messages() {
        let eff = sample_effective();
        let msgs = vec![
            MessageItem { role: "system".into(), content: "设定".into() },
            MessageItem { role: "user".into(), content: "hi".into() },
        ];
        let body = anthropic_body(&eff, &msgs);
        assert_eq!(body["system"], "设定");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1, "system 不应出现在 messages");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn parse_anthropic_response_extracts_text_and_stop_reason() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "答复" }],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content, "答复");
        assert_eq!(resp.finish_reason, "end_turn");
    }

    #[test]
    fn parse_openai_models_extracts_ids() {
        let data = serde_json::json!({ "data": [ { "id": "deepseek-chat" }, { "id": "deepseek-reasoner" } ] });
        assert_eq!(parse_openai_models(&data), vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()]);
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
