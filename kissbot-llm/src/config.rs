use std::sync::Arc;

use kissbot_api::{ArcSwapHashMap, MergeBy, MergeEffectiveConfig, MergeSelf};
use serde::{Deserialize, Serialize};

/// 模型默认请求超时（秒）
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// 模型默认重试次数
pub const DEFAULT_RETRY_COUNT: u32 = 3;

// ========== Provider 配置 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: Arc<String>,      // "openai" | "anthropic"，决定 Provider 实现
    pub base_url: Arc<String>,           // URL 前缀，如 https://api.deepseek.com
    pub api_key: Arc<String>,            // provider 级密钥
}

/// provider 级配置：provider 连接信息 + 默认模型参数 + 单模型覆盖
/// （provider 名由调用方作为存储键提供，本结构不含 name）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    pub provider_config: Arc<ProviderConfig>,
    pub default_model_config: ModelConfig,
    pub model_configs: Arc<ArcSwapHashMap<String, ModelConfig>>,  // key = model 标识
}

/// 可继承模型参数（Option 覆盖字段；未配字段回落上一级：model → provider 默认 → 全局常量）
/// 复用作 provider 默认值容器（ProviderModelConfig.default_model_config）与 model 覆盖（model_configs map 值）
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

/// 合成 provider 默认 + model 覆盖的有效参数：
/// 全局默认 ← provider 默认 ← model 覆盖，model 未配字段继承 provider，二者都未配回落全局常量
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

// ========== LLM 请求配置（可继承 + 有效形态） ==========

/// 可继承 LLM 配置：provider + model + 生成参数。
/// `has_*` 标志区分「未配置」与「显式置空」：`set_*` 置 `has_* = true`（取值可为 None），
/// `unset_*` 置回 false；合并时只覆盖 `has_*` 为 true 的字段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LLMConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_max_tokens: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_temperature: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_thinking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_reasoning_effort: bool,
}

impl LLMConfig {
    pub fn set_max_tokens(&mut self, max_tokens: Option<u32>) {
        self.max_tokens = max_tokens;
        self.has_max_tokens = true;
    }
    pub fn unset_max_tokens(&mut self) {
        self.max_tokens = None;
        self.has_max_tokens = false;
    }
    pub fn set_temperature(&mut self, temperature: Option<f32>) {
        self.temperature = temperature;
        self.has_temperature = true;
    }
    pub fn unset_temperature(&mut self) {
        self.temperature = None;
        self.has_temperature = false;
    }
    pub fn set_thinking(&mut self, thinking: Option<Arc<String>>) {
        self.thinking = thinking;
        self.has_thinking = true;
    }
    pub fn unset_thinking(&mut self) {
        self.thinking = None;
        self.has_thinking = false;
    }
    pub fn set_reasoning_effort(&mut self, reasoning_effort: Option<Arc<String>>) {
        self.reasoning_effort = reasoning_effort;
        self.has_reasoning_effort = true;
    }
    pub fn unset_reasoning_effort(&mut self) {
        self.reasoning_effort = None;
        self.has_reasoning_effort = false;
    }
}

impl MergeBy<LLMConfig> for LLMConfig {
    fn merge(&mut self, other: &LLMConfig) {
        if let Some(provider) = other.provider.as_ref() {
            self.provider = Some(provider.clone());
        }
        if let Some(model) = other.model.as_ref() {
            self.model = Some(model.clone());
        }
        if other.has_max_tokens {
            self.set_max_tokens(other.max_tokens);
        }
        if other.has_temperature {
            self.set_temperature(other.temperature);
        }
        if other.has_thinking {
            self.set_thinking(other.thinking.clone());
        }
        if other.has_reasoning_effort {
            self.set_reasoning_effort(other.reasoning_effort.clone());
        }
    }
}

impl MergeSelf for LLMConfig {}

/// 合成后的 LLM 请求参数（provider / model 必填，生成参数可选），运行时合成、不持久化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveLLMConfig {
    pub provider: Arc<String>,
    pub model: Arc<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub thinking: Option<Arc<String>>,
    pub reasoning_effort: Option<Arc<String>>,
}

impl EffectiveLLMConfig {
    pub fn new() -> Self {
        Self {
            provider: Arc::new(String::new()),
            model: Arc::new(String::new()),
            max_tokens: None,
            temperature: None,
            thinking: None,
            reasoning_effort: None,
        }
    }
}

impl MergeBy<LLMConfig> for EffectiveLLMConfig {
    fn merge(&mut self, other: &LLMConfig) {
        if let Some(provider) = other.provider.as_ref() {
            self.provider = provider.clone();
        }
        if let Some(model) = other.model.as_ref() {
            self.model = model.clone();
        }
        if other.has_max_tokens {
            self.max_tokens = other.max_tokens;
        }
        if other.has_temperature {
            self.temperature = other.temperature;
        }
        if other.has_thinking {
            self.thinking = other.thinking.clone();
        }
        if other.has_reasoning_effort {
            self.reasoning_effort = other.reasoning_effort.clone();
        }
    }
}

impl MergeEffectiveConfig<EffectiveLLMConfig> for LLMConfig {
    fn get_effective_config(&self) -> EffectiveLLMConfig {
        let mut result = EffectiveLLMConfig::new();
        result.merge(self);
        result
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// provider 配置（provider_config + default_model_config；model_configs 默认空）
    fn sample_provider() -> ProviderModelConfig {
        ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: Arc::new("openai".into()),
                base_url: Arc::new("https://api.deepseek.com".into()),
                api_key: Arc::new("sk-test".into()),
            }),
            default_model_config: ModelConfig {
                max_tokens_usage: 128000,
                timeout_secs: Some(60),
                retry_count: Some(3),
            },
            model_configs: Arc::new(ArcSwapHashMap::new()),
        }
    }

    /// 往 provider 的 model_configs 写入一条 model 覆盖（ArcSwapHashMap 写时复制）
    fn with_model(mut provider: ProviderModelConfig, model: &str, config: ModelConfig) -> ProviderModelConfig {
        let map = Arc::make_mut(&mut provider.model_configs);
        map.insert(model.to_string(), arc_swap::ArcSwap::new(Arc::new(config)));
        provider
    }

    /// LLM 配置助手：provider/model 已配，其余字段按需覆盖
    fn llm_config(provider: &str, model: &str) -> LLMConfig {
        LLMConfig {
            provider: Some(Arc::new(provider.into())),
            model: Some(Arc::new(model.into())),
            ..LLMConfig::default()
        }
    }

    #[test]
    fn provider_config_old_shape_no_longer_parses() {
        // 旧扁平格式（name/provider_type/base_url/api_key/default_*）无 provider_config 段 →
        // 解析失败（破坏性变更，配置文件需迁移）
        let old = r#"{
            "name": "deepseek",
            "provider_type": "openai",
            "base_url": "https://api.deepseek.com",
            "api_key": "sk-test",
            "default_context_length": 65536,
            "default_max_tokens": 4096,
            "default_temperature": 0.7,
            "default_timeout_secs": 60,
            "default_retry_count": 3,
            "default_max_context_messages": 100,
            "models": {}
        }"#;
        assert!(serde_json::from_str::<ProviderModelConfig>(old).is_err(), "旧格式缺 provider_config 应解析失败");
    }

    #[test]
    fn provider_config_missing_max_tokens_usage_fails() {
        // 必填语义：default_model_config 段内缺 max_tokens_usage → 解析失败
        let json = r#"{
            "provider_config": { "provider_type": "openai", "base_url": "https://api.deepseek.com", "api_key": "sk-test" },
            "default_model_config": { "timeout_secs": 60 },
            "model_configs": {}
        }"#;
        assert!(serde_json::from_str::<ProviderModelConfig>(json).is_err(), "缺 max_tokens_usage 应解析失败");
    }

    #[test]
    fn provider_config_nested_roundtrip() {
        // 嵌套格式序列化/反序列化往返（锁定 provider_config + default_model_config + model_configs 契约）
        let pc = with_model(sample_provider(), "deepseek-4-flash", ModelConfig {
            max_tokens_usage: 131072,
            timeout_secs: Some(30),
            retry_count: Some(2),
        });
        let json = serde_json::to_string(&pc).unwrap();
        let back: ProviderModelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider_config.provider_type.as_str(), "openai");
        assert_eq!(back.provider_config.base_url.as_str(), "https://api.deepseek.com");
        assert_eq!(back.provider_config.api_key.as_str(), "sk-test");
        assert_eq!(back.default_model_config.max_tokens_usage, 128000);
        assert_eq!(back.default_model_config.timeout_secs, Some(60));
        assert_eq!(back.default_model_config.retry_count, Some(3));
        let model = back.model_configs.get("deepseek-4-flash").expect("model 覆盖应回读").load_full();
        assert_eq!(model.max_tokens_usage, 131072);
        assert_eq!(model.timeout_secs, Some(30));
    }

    #[test]
    fn get_effective_config_missing_model_uses_provider_default() {
        // model 未在 model_configs 配置（含 model_configs={} 极端情况）→ 全取 provider 默认值
        let eff = sample_provider().get_effective_config("unconfigured-model");
        assert_eq!(eff.max_tokens_usage, 128000, "未配置参数取 provider 默认值");
        assert_eq!(eff.timeout_secs, 60);
        assert_eq!(eff.retry_count, 3);
        assert_eq!(eff.provider_config.provider_type.as_str(), "openai");
    }

    #[test]
    fn merge_model_provider_partial_defaults_fall_back_to_globals() {
        // provider 默认仅配部分字段：未配字段回落全局常量；model 覆盖仍生效
        let mut provider = sample_provider();
        provider.default_model_config = ModelConfig {
            max_tokens_usage: 128000,
            timeout_secs: None,
            retry_count: None,
        };
        let provider = with_model(provider, "deepseek-4-flash", ModelConfig {
            max_tokens_usage: 128000,
            timeout_secs: Some(30),
            retry_count: None,
        });
        let eff = provider.get_effective_config("deepseek-4-flash");
        assert_eq!(eff.max_tokens_usage, 128000, "provider 默认值生效");
        assert_eq!(eff.timeout_secs, 30, "model 覆盖 provider");
        assert_eq!(eff.retry_count, DEFAULT_RETRY_COUNT, "两级都未配回落全局常量");
    }

    #[test]
    fn merge_model_provider_all_default_and_model_overrides() {
        // provider 全缺省（ModelConfig::default()）+ model 覆盖
        let mut provider = sample_provider();
        provider.default_model_config = ModelConfig::default();
        let provider = with_model(provider, "deepseek-4-flash", ModelConfig {
            max_tokens_usage: 262144,
            timeout_secs: None,
            retry_count: None,
        });
        let eff = provider.get_effective_config("deepseek-4-flash");
        assert_eq!(eff.max_tokens_usage, 262144, "model 覆盖生效");
        assert_eq!(eff.timeout_secs, DEFAULT_TIMEOUT_SECS, "provider 全缺省回落全局常量");
        assert_eq!(eff.retry_count, DEFAULT_RETRY_COUNT);
    }

    #[test]
    fn llm_config_merge_overrides_only_configured_fields() {
        // 合并语义：provider/model 有值才覆盖；其余字段按 has_* 判断（未配置不覆盖）
        let mut base = llm_config("deepseek", "deepseek-chat");
        base.set_max_tokens(Some(1024));

        let mut other = LLMConfig::default();
        other.model = Some(Arc::new("deepseek-reasoner".into()));
        base.merge(&other);

        assert_eq!(base.provider.as_deref().map(String::as_str), Some("deepseek"), "未配字段保留");
        assert_eq!(base.model.as_deref().map(String::as_str), Some("deepseek-reasoner"), "已配字段覆盖");
        assert_eq!(base.max_tokens, Some(1024), "has_* 为 false 不覆盖");
    }

    #[test]
    fn llm_config_merge_respects_explicit_none() {
        // set_*(None) = 显式置空（has_* 为 true）→ 覆盖为 None；unset_* → 回落不覆盖
        let mut base = llm_config("deepseek", "deepseek-chat");
        base.set_temperature(Some(0.7));

        let mut clear = LLMConfig::default();
        clear.set_temperature(None);
        base.merge(&clear);
        assert!(base.has_temperature);
        assert_eq!(base.temperature, None, "显式置空覆盖");

        let mut unset = LLMConfig::default();
        unset.unset_temperature();
        base.merge(&unset);
        assert_eq!(base.temperature, None, "unset 不改变取值");
    }

    #[test]
    fn llm_config_effective_config_merges_into_default() {
        let mut cfg = llm_config("deepseek", "deepseek-chat");
        cfg.set_thinking(Some(Arc::new("enabled".into())));
        cfg.set_reasoning_effort(Some(Arc::new("high".into())));
        let eff = cfg.get_effective_config();
        assert_eq!(eff.provider.as_str(), "deepseek");
        assert_eq!(eff.model.as_str(), "deepseek-chat");
        assert_eq!(eff.thinking.as_deref().map(String::as_str), Some("enabled"));
        assert_eq!(eff.reasoning_effort.as_deref().map(String::as_str), Some("high"));
        assert!(eff.max_tokens.is_none() && eff.temperature.is_none(), "未配置生成参数为空");

        // 空配置 → 全默认（provider/model 为空串）
        let empty = LLMConfig::default().get_effective_config();
        assert!(empty.provider.is_empty() && empty.model.is_empty());
    }

    #[test]
    fn llm_config_serde_skips_unset_flags() {
        // has_* 为 false 时不序列化；为 true 时随取值一起持久化（区分「未配」与「显式置空」）
        let mut cfg = llm_config("deepseek", "deepseek-chat");
        let json = serde_json::to_value(&cfg).unwrap();
        assert!(json.get("has_max_tokens").is_none(), "未配置标志不序列化");
        assert!(json.get("max_tokens").is_none());

        cfg.set_max_tokens(None);
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["has_max_tokens"], json!(true), "显式置空仍序列化标志");
        assert!(json.get("max_tokens").is_none(), "置空时取值省略");
        let back: LLMConfig = serde_json::from_value(json).unwrap();
        assert!(back.has_max_tokens && back.max_tokens.is_none(), "回读保留显式置空语义");
    }
}
