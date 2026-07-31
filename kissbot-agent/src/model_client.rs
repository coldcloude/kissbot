use std::time::Duration;

use serde_json::json;
use tokio::time::sleep;

use crate::types::{ModelResponse, Result, Error};
use crate::config_manager::ModelConfig;

pub struct ModelClient {
    config: ModelConfig,
    client: reqwest::Client,
}

impl ModelClient {
    pub fn new(config: ModelConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();
        Self { config, client }
    }

    #[allow(dead_code)]
    pub fn update_config(&mut self, config: ModelConfig) {
        self.config = config;
        self.client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.config.timeout_secs))
            .build()
            .unwrap_or_default();
    }

    /// 调用模型 API（非流式）
    pub async fn call(&self, messages: &[MessageItem]) -> Result<ModelResponse> {
        let max_retries = self.config.retry_count;
        let mut last_error = None;

        for attempt in 0..=max_retries {
            match self.call_inner(messages).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        sleep(Duration::from_secs(1u64 << attempt)).await; // 指数退避
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::ModelApiError("模型调用失败".to_string())))
    }

    async fn call_inner(&self, messages: &[MessageItem]) -> Result<ModelResponse> {
        match self.config.provider.as_str() {
            "openai" => self.call_openai(messages).await,
            "anthropic" => self.call_anthropic(messages).await,
            _ => Err(Error::ModelProviderNotSupported(self.config.provider.clone())),
        }
    }

    async fn call_openai(&self, messages: &[MessageItem]) -> Result<ModelResponse> {
        let url = format!("{}/chat/completions", self.config.endpoint.trim_end_matches('/'));

        let msgs: Vec<serde_json::Value> = messages.iter().map(|m| {
            json!({
                "role": m.role,
                "content": m.content,
            })
        }).collect();

        let body = json!({
            "model": self.config.model,
            "messages": msgs,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
            "stream": false,
        });

        let resp = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("OpenAI API {}: {}", status, text)));
        }

        let data: serde_json::Value = resp.json().await?;
        let choice = data["choices"][0].clone();

        let content = choice["message"]["content"].as_str()
            .unwrap_or("")
            .to_string();

        let tool_calls = Vec::new(); // 本期不支持 tool call
        let finish_reason = choice["finish_reason"].as_str()
            .unwrap_or("stop")
            .to_string();

        Ok(ModelResponse { content, tool_calls, finish_reason })
    }

    async fn call_anthropic(&self, messages: &[MessageItem]) -> Result<ModelResponse> {
        let url = format!("{}/v1/messages", self.config.endpoint.trim_end_matches('/'));

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
            "model": self.config.model,
            "messages": msgs,
            "max_tokens": self.config.max_tokens,
        });

        if !system.is_empty() {
            body["system"] = json!(system);
        }

        let resp = self.client.post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("Anthropic API {}: {}", status, text)));
        }

        let data: serde_json::Value = resp.json().await?;
        let content = data["content"][0]["text"].as_str()
            .unwrap_or("")
            .to_string();
        let finish_reason = data["stop_reason"].as_str()
            .unwrap_or("end_turn")
            .to_string();

        Ok(ModelResponse { content, tool_calls: Vec::new(), finish_reason })
    }
}

/// 模型上下文中的单条消息
#[derive(Debug, Clone)]
pub struct MessageItem {
    pub role: String,
    pub content: String,
}
