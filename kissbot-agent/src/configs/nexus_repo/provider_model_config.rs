
// ---- 全局默认值（provider/model 未配字段回落；值 = 原模板必填值） ----

use std::sync::Arc;

use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};

use crate::configs::{MergeBy, MergeSelf};

/// 模型默认请求超时（秒）
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// 模型默认重试次数
pub const DEFAULT_RETRY_COUNT: u32 = 3;

// ========== Provider 配置 ==========

// (provider, model) 固定一起出现：函数调用、current 运行状态、default 配置共用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider: Arc<String>,
    pub model: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: Arc<String>,      // "openai" | "anthropic"，决定 Provider 实现
    pub base_url: Arc<String>,           // URL 前缀，如 https://api.deepseek.com（原 endpoint）
    pub api_key: Arc<String>,            // provider 级密钥
}

// ProviderConfig 定义在本文件，供 provider / model_client 与本文件的 NexusRepo.providers 共用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    pub provider_config: Arc<ProviderConfig>,
    pub default_model_config: ModelConfig,
    pub model_configs: Arc<ArcSwapHashMap<String, ModelConfig>>,  // key = model 标识
}

/// 可继承模型参数（Option 覆盖字段；未配字段回落上一级：model → provider 默认 → 全局常量）
/// 复用作 provider 默认值容器（ProviderConfig.default_model_config）与 model 覆盖（models map 值）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    pub max_tokens_usage: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
}

impl MergeBy<ModelConfig> for ModelConfig {
    fn merge(&mut self, other: &ModelConfig) {
        self.max_tokens_usage = other.max_tokens_usage;
        if let Some(timeout_secs) = other.timeout_secs {
            self.timeout_secs = Some(timeout_secs);
        }
        if let Some(retry_count) = other.retry_count {
            self.retry_count = Some(retry_count);
        }
    }
}

impl MergeSelf for ModelConfig {}

// 合并后的有效配置（provider 默认 + model 覆盖），运行时合成、不持久化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveModelConfig {
    pub provider_config: Arc<ProviderConfig>,
    pub max_tokens_usage: u32,
    pub timeout_secs: u64,
    pub retry_count: u32,
}

impl EffectiveModelConfig {
    pub fn new(provider_config : Arc<ProviderConfig>) -> Self {
        Self {
            provider_config,
            max_tokens_usage: 0,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            retry_count: DEFAULT_RETRY_COUNT,
        }
    }
}

impl MergeBy<ModelConfig> for EffectiveModelConfig {
    fn merge(&mut self, other: &ModelConfig) {
        self.max_tokens_usage = other.max_tokens_usage;
        if let Some(timeout_secs) = other.timeout_secs {
            self.timeout_secs = timeout_secs;
        }
        if let Some(retry_count) = other.retry_count {
            self.retry_count = retry_count;
        }
    }
}

/// 合成 provider 默认 + model 覆盖的有效参数（与 merge_context_config 同模式：
/// 全局默认 ← provider 默认 ← model 覆盖，model 未配字段继承 provider，二者都未配回落全局常量；
/// temperature/thinking/reasoning_effort 无全局默认，None 传播（不发送））
impl ProviderModelConfig {
    pub fn get_effective_config(&self, model_name: &str) -> EffectiveModelConfig {
        let mut config = self.default_model_config.clone();
        if let Some(model_config) = self.model_configs.get(model_name) {
            config.merge(model_config.load().as_ref());
        }
        let mut effective_config = EffectiveModelConfig::new(self.provider_config.clone());
        effective_config.merge(&config);
        effective_config
    }
}
