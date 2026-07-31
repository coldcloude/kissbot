use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::config_manager::EffectiveModelConfig;
use crate::types::{Error, MessageItem, ModelResponse, Result};

/// Provider 抽象：负责向模型服务商发一次请求并解析响应
// 尚未被 ModelClient 消费（Task 4 接线），暂标 allow(dead_code)
#[allow(dead_code)]
#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse>;
}

// ========== OpenAI 兼容协议（/chat/completions） ==========

#[allow(dead_code)]
pub struct OpenAiProvider {
    client: Arc<reqwest::Client>,
    base_url: String,
    api_key: String,
}

#[allow(dead_code)]
impl OpenAiProvider {
    pub fn new(client: Arc<reqwest::Client>, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
        }
    }
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
}

// ========== Anthropic 协议（/v1/messages） ==========

#[allow(dead_code)]
pub struct AnthropicProvider {
    client: Arc<reqwest::Client>,
    base_url: String,
    api_key: String,
}

#[allow(dead_code)]
impl AnthropicProvider {
    pub fn new(client: Arc<reqwest::Client>, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
        }
    }
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
            temperature: 0.3,
            timeout_secs: 30,
            retry_count: 2,
            context_length: 65536,
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
}
