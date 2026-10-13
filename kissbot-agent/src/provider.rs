use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::AsyncCacheFactory;
use kissbot_llm::{
    AnthropicProvider, EffectiveLLMConfig, Message, ModelResponse, OpenAiProvider, Provider, ProviderConfig, ToolConfig, call_with_retry, create_anthropic_provider, create_openai_provider,
};

use crate::config_manager::ConfigManager;
use crate::types::{Error, Result};

/// Provider 工厂与分派（agent 侧运行时组件）：
/// - 工厂去重：按 provider 名缓存实现实例（同一 provider 的多次调用复用同一实例与 HTTP 客户端）
/// - 分派：按 provider_type 选择 openai / anthropic 实现
/// - 配置：按 (provider, model) 从 ConfigManager 取有效模型配置（重试次数 / 超时 / token 上限）
///
/// 实现本身（请求构造、API 调用、重试、响应归一化）由 kissbot-llm 提供，本结构只做上述组织工作。
pub struct ProviderManager {
    openai_provider_factory: AsyncCacheFactory<OpenAiProvider>,
    anthropic_provider_factory: AsyncCacheFactory<AnthropicProvider>,
}

impl ProviderManager {
    pub fn new() -> Self {
        Self {
            openai_provider_factory: AsyncCacheFactory::new(),
            anthropic_provider_factory: AsyncCacheFactory::new(),
        }
    }

    /// 取（或首次构造）指定 provider 名的 OpenAI 兼容实现：同名复用同一实例
    async fn get_openai_provider(&self, name: &str, provider_config: &ProviderConfig) -> Arc<OpenAiProvider> {
        self.openai_provider_factory.get_or_create(ArcStr::from(name), || create_openai_provider(provider_config)).await
    }

    /// 取（或首次构造）指定 provider 名的 Anthropic 实现：同名复用同一实例
    async fn get_anthropic_provider(&self, name: &str, provider_config: &ProviderConfig) -> Arc<AnthropicProvider> {
        self.anthropic_provider_factory.get_or_create(ArcStr::from(name), || create_anthropic_provider(provider_config)).await
    }

    /// 调用 LLM：按 (provider, model) 取有效模型配置 → 按 provider_type 取实现 → kissbot-llm 带重试调用
    pub async fn call(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse> {
        let provider = llm_cfg.provider.as_str();
        let model = llm_cfg.model.as_str();
        let pm_cfg = ConfigManager::get().provider_model_config(provider, model).await
        .ok_or_else(|| Error::ModelProviderNotFound(ArcStr::from(provider), ArcStr::from(model)))?;
        // 按 provider_type 分派实现（"openai" | "anthropic"）
        match pm_cfg.provider_config.provider_type.as_str() {
            "openai" => {
                let provider = self.get_openai_provider(provider, pm_cfg.provider_config.as_ref()).await;
                let result = call_with_retry(provider.as_ref(), llm_cfg, &pm_cfg, messages, tools).await?;
                Ok(result)
            },
            "anthropic" => {
                let provider = self.get_anthropic_provider(provider, pm_cfg.provider_config.as_ref()).await;
                let result = call_with_retry(provider.as_ref(), llm_cfg, &pm_cfg, messages, tools).await?;
                Ok(result)
            },
            other => Err(Error::ModelProviderNotSupported(format!("未知 provider_type: {}", other).into())),
        }
    }

    /// 从服务商 API 获取全部模型名
    /// 返回 Err 表示 provider 未配置 / provider_type 无法映射 / API 调用失败（网络、鉴权）
    pub async fn list_models(&self, provider_name: &str) -> Result<Vec<ArcStr>> {
        let provider_config = ConfigManager::get().provider_config(provider_name).await
            .ok_or_else(|| Error::ModelProviderNotFound(ArcStr::from(provider_name), ArcStr::new()))?;
        // 按 provider_type 分派实现（"openai" | "anthropic"）
        match provider_config.provider_type.as_str() {
            "openai" => {
                let provider = self.get_openai_provider(provider_name, provider_config.as_ref()).await;
                let result = provider.list_models().await?;
                Ok(result)
            },
            "anthropic" => {
                let provider = self.get_anthropic_provider(provider_name, provider_config.as_ref()).await;
                let result = provider.list_models().await?;
                Ok(result)
            },
            other => Err(Error::ModelProviderNotSupported(format!("未知 provider_type: {}", other).into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock};
    use std::sync::atomic::{AtomicBool, Ordering};

    use kissbot_llm::{ModelConfig, ProviderModelConfig};

    use super::*;

    fn sample_provider_config() -> ProviderConfig {
        ProviderConfig {
            provider_type: ArcStr::from("openai"),
            base_url: ArcStr::from("https://api.example.com"),
            api_key: ArcStr::from("sk-test"),
        }
    }

    /// 进程级装配（幂等）：list_models / call 经 ConfigManager::get() 读 provider 配置，未初始化会 panic。
    /// data_dir 目录经 OnceLock 保活，避免 tempdir drop 后单例路径失效
    static TEST_GLOBAL_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static TEST_INIT_DONE: AtomicBool = AtomicBool::new(false);
    async fn ensure_config_manager() {
        if !TEST_INIT_DONE.load(Ordering::Relaxed) {
            let dir = TEST_GLOBAL_DIR.get_or_init(|| tempfile::tempdir().unwrap());
            let cfg_path = dir.path().join("config.json");
            let cfg_json = format!(
                r#"{{"api":{{"memory_store_url":"","memory_ego_url":""}},"security":{{"api_key":"user-key-456","admin_api_key":"admin-key-123"}},"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9093,"ws_reconnect_interval_secs":5}}}}"#,
                dir.path().join("data").to_str().unwrap()
            );
            std::fs::write(&cfg_path, cfg_json).unwrap();
            // 2024 edition：设置环境变量需要 unsafe
            unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };
            // 幂等：ConfigManager::new() 注册一次（第二实例丢弃）
            let _ = ConfigManager::new().await;
            TEST_INIT_DONE.store(true, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn factory_dedupes_provider_instances_by_name() {
        // 工厂去重：同名 provider 复用同一实例（HTTP 客户端随之复用），不同名各自独立
        let manager = ProviderManager::new();
        let cfg = sample_provider_config();
        let a = manager.get_openai_provider("p1", &cfg).await;
        let b = manager.get_openai_provider("p1", &cfg).await;
        assert!(Arc::ptr_eq(&a, &b), "同名 provider 应复用同一实例");
        let c = manager.get_openai_provider("p2", &cfg).await;
        assert!(!Arc::ptr_eq(&a, &c), "不同名 provider 应各自持有实例");
    }

    #[tokio::test]
    async fn call_rejects_unknown_provider() {
        ensure_config_manager().await;
        // provider 未配置 → 取有效模型配置失败，返回 ModelProviderNotFound（不触网）
        let mut llm_cfg = EffectiveLLMConfig::new();
        llm_cfg.provider = ArcStr::from("no-such-provider");
        llm_cfg.model = ArcStr::from("m");
        let err = ProviderManager::new().call(&llm_cfg, vec![], &vec![]).await.err().expect("provider 不存在应返回 Err");
        assert!(matches!(err, Error::ModelProviderNotFound(_, _)), "未配置的 provider 应返回 ModelProviderNotFound");
    }

    #[tokio::test]
    async fn list_models_rejects_unknown_provider_type() {
        ensure_config_manager().await;
        // provider_type 无法映射实现 → ModelProviderNotSupported（分派在 call/list_models 内联；
        // 该分支在构造 Provider 前返回，不触网）
        let _ = ConfigManager::get().add_provider("typo-provider", ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("typo"),
                base_url: ArcStr::from("https://api.example.com"),
                api_key: ArcStr::from("sk-test"),
            }),
            default_model_config: ModelConfig {
                max_tokens_usage: 128000,
                timeout_secs: None,
                retry_count: None,
            },
            model_configs: Arc::new(HashMap::new()),
        }).await;
        let manager = ProviderManager::new();
        let err = manager.list_models("typo-provider").await.err().expect("未知 provider_type 应返回 Err");
        assert!(matches!(err, Error::ModelProviderNotSupported(_)), "未知类型应返回 ModelProviderNotSupported");
        assert!(err.to_string().contains("未知 provider_type: typo"), "错误信息应指明未知类型：{}", err);
    }

    #[tokio::test]
    async fn list_models_rejects_unknown_provider_name() {
        ensure_config_manager().await;
        let err = ProviderManager::new().list_models("no-such-provider").await.err().expect("provider 不存在应返回 Err");
        assert!(matches!(err, Error::ModelProviderNotFound(_, _)), "未配置的 provider 应返回 ModelProviderNotFound");
    }
}
