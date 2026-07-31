use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use crate::config_manager::{ConfigManager, EffectiveModelConfig, ProviderModel};
use crate::provider::{AnthropicProvider, OpenAiProvider, Provider};
use crate::types::{Error, MessageItem, ModelResponse, Result};

pub struct ModelClient {
    config_manager: Arc<ConfigManager>,
    client: Arc<reqwest::Client>,
}

impl ModelClient {
    pub fn new(config_manager: Arc<ConfigManager>) -> Self {
        let client = Arc::new(reqwest::Client::new());
        Self { config_manager, client }
    }

    /// 调用模型 API（非流式）
    /// 每次调用经 ConfigManager 现场合成最新 EffectiveModelConfig（配置永远最新，无需热更新），
    /// 并按 provider_type 构建对应 Provider 实现（未知类型报错）。
    pub async fn call(&self, pm: &ProviderModel, messages: &[MessageItem]) -> Result<ModelResponse> {
        let effective = self.config_manager.resolve_effective_config(pm).await
            .ok_or_else(|| Error::ModelProviderNotSupported(format!(
                "provider/model 不存在: {}/{}", pm.provider, pm.model)))?;
        let provider: Box<dyn Provider> = self.build_provider(&effective);
        self.call_with_retry(&effective, provider, messages).await
    }

    /// 按 provider_type 构建 Provider 实现（protocol 差异封装在 provider.rs）
    fn build_provider(&self, effective: &EffectiveModelConfig) -> Box<dyn Provider> {
        match effective.provider_type.as_str() {
            "openai" => Box::new(OpenAiProvider::new(self.client.clone(), &effective.base_url, &effective.api_key)),
            "anthropic" => Box::new(AnthropicProvider::new(self.client.clone(), &effective.base_url, &effective.api_key)),
            other => Box::new(UnsupportedProvider { provider_type: other.to_string() }),
        }
    }

    /// 指数退避重试（retry_count 来自有效配置）
    async fn call_with_retry(
        &self,
        effective: &EffectiveModelConfig,
        provider: Box<dyn Provider>,
        messages: &[MessageItem],
    ) -> Result<ModelResponse> {
        let max_retries = effective.retry_count;
        let mut last_error = None;

        for attempt in 0..=max_retries {
            match provider.send(effective, messages).await {
                Ok(response) => return Ok(response),
                // 配置类错误（如未知 provider_type）是永久性错误，重试无意义，直接返回
                Err(e @ Error::ModelProviderNotSupported(_)) => return Err(e),
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
}

/// 未知 provider_type 时的占位实现（调用即报错）
struct UnsupportedProvider {
    provider_type: String,
}

#[async_trait::async_trait]
impl Provider for UnsupportedProvider {
    async fn send(&self, _effective: &EffectiveModelConfig, _messages: &[MessageItem]) -> Result<ModelResponse> {
        Err(Error::ModelProviderNotSupported(self.provider_type.clone()))
    }
}
