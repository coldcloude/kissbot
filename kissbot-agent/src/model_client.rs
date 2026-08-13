use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use crate::config_manager::{ConfigManager, EffectiveModelConfig, ProviderModel, ToolConfig};
use crate::provider::Provider;
use crate::types::{Error, Message, ModelResponse, Result};

pub struct ModelClient {
    client: Arc<reqwest::Client>,
}

impl ModelClient {
    pub fn new() -> Self {
        let client = Arc::new(reqwest::Client::new());
        Self { client }
    }

    /// 调用模型 API（非流式）
    /// 每次调用经 ConfigManager 现场合成最新 EffectiveModelConfig（配置永远最新，无需热更新），
    /// 并按 provider_type 构建对应 Provider 实现（未知类型报错）。
    /// tools 为本次请求携带的工具定义（空则 OpenAI 请求不发送 tools 字段）。
    pub async fn call(&self, pm: &ProviderModel, messages: &[Message], tools: &[ToolConfig]) -> Result<ModelResponse> {
        let effective = ConfigManager::get().resolve_effective_config(pm).await
            .ok_or_else(|| Error::ModelProviderNotSupported(format!(
                "provider/model 不存在: {}/{}", pm.provider, pm.model)))?;
        let provider: Box<dyn Provider> = self.build_provider(&effective)?;
        self.call_with_retry(&effective, provider, messages, tools).await
    }

    /// 按 provider_type 构建 Provider 实现（protocol 差异封装在 provider.rs，未知类型报错）
    fn build_provider(&self, effective: &EffectiveModelConfig) -> Result<Box<dyn Provider>> {
        crate::provider::provider_for(self.client.clone(), &effective.provider_type, &effective.base_url, &effective.api_key)
    }

    /// 从服务商 API 获取全部模型名（按 pm.provider 的 ProviderConfig 构造 Provider）
    /// 返回 Err 表示 API 调用失败（网络/鉴权）
    pub async fn list_models(&self, pm: &ProviderModel) -> Result<Vec<String>> {
        let pc = ConfigManager::get().provider_config_by_name(&pm.provider).await
            .ok_or_else(|| Error::ModelProviderNotSupported(format!("provider 不存在: {}", pm.provider)))?;
        let provider = crate::provider::provider_for(
            self.client.clone(), &pc.provider_type, &pc.base_url, &pc.api_key)?;
        provider.list_models().await
    }

    /// 指数退避重试（retry_count 来自有效配置）
    async fn call_with_retry(
        &self,
        effective: &EffectiveModelConfig,
        provider: Box<dyn Provider>,
        messages: &[Message],
        tools: &[ToolConfig],
    ) -> Result<ModelResponse> {
        let max_retries = effective.retry_count;
        let mut last_error = None;

        for attempt in 0..=max_retries {
            match provider.send(effective, messages, tools).await {
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
