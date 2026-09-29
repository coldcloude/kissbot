use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use arcstr::ArcStr;
use kissbot_api::MergeEffectiveConfig;
use kissbot_llm::{
    EffectiveLLMConfig, EffectiveModelConfig, LLMConfig, ModelConfig, ProviderConfig,
    ProviderModelConfig,
};
use serde::Deserialize;

/// 组件数据目录名（记忆根目录下各 agent 目录内的子目录）
pub const STRUCT_DIR_NAME: ArcStr = arcstr::literal!("memory-struct-abstract");

pub const DEFAULT_LISTEN_ADDR: ArcStr = arcstr::literal!("127.0.0.1");
/// HTTP 服务默认端口（memory-store 8082 / memory-ego 3001 之外）
pub const DEFAULT_LISTEN_PORT: u16 = 8083;
/// 单通道单次处理的消息条数上限（懒处理时按此分块，避免一次把过长记录送入模型）
pub const DEFAULT_MAX_BATCH_RECORDS: usize = 200;
/// 通知断线重连间隔默认值（秒）
pub const DEFAULT_WS_RECONNECT_INTERVAL_SECS: u64 = 5;
/// 模型会话上下文保留的消息条数上限
pub const DEFAULT_SESSION_MAX_MESSAGES: usize = 20;
fn default_listen_addr() -> ArcStr {
    DEFAULT_LISTEN_ADDR.clone()
}

fn default_listen_port() -> u16 {
    DEFAULT_LISTEN_PORT
}

fn default_max_batch_records() -> usize {
    DEFAULT_MAX_BATCH_RECORDS
}

fn default_ws_reconnect_interval_secs() -> u64 {
    DEFAULT_WS_RECONNECT_INTERVAL_SECS
}

fn default_session_max_messages() -> usize {
    DEFAULT_SESSION_MAX_MESSAGES
}

/// 摘要记忆组件配置（`memory_struct_abstract` 段）
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: ArcStr,
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    /// memory-store 通知服务地址（ws://…）；缺省则不订阅通知（仅手动触发/重建）
    #[serde(default)]
    pub notify_url: Option<ArcStr>,
    /// 通知断线重连间隔（秒）
    #[serde(default = "default_ws_reconnect_interval_secs")]
    pub ws_reconnect_interval_secs: u64,
    /// 单通道单次处理的消息条数上限（懒处理分块）
    #[serde(default = "default_max_batch_records")]
    pub max_batch_records: usize,
    /// 模型会话上下文保留消息条数上限
    #[serde(default = "default_session_max_messages")]
    pub session_max_messages: usize,
    /// provider 配置（结构与 kissbot-llm 的 ProviderModelConfig 一致；同时接受文档中的 `provider` 键名）
    #[serde(alias = "provider")]
    pub provider_config: Arc<ProviderConfig>,
    /// provider 默认模型参数（`max_tokens_usage` 必填）
    pub default_model_config: ModelConfig,
    /// 单模型覆盖
    #[serde(default)]
    pub model_configs: Arc<HashMap<ArcStr, Arc<ModelConfig>>>,
    /// LLM 请求参数（provider / model / 生成参数）
    pub llm: LLMConfig,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("memory_struct_abstract")
        })
    }

    /// 记忆根目录：取 kissbot-memory 的 `memory.root_dir`（case 数据与原始记录同根，不单独配置）
    pub fn storage_root(&self) -> PathBuf {
        kissbot_memory::Config::get().root_dir.clone()
    }

    pub fn listen_addr(&self) -> ArcStr {
        ArcStr::from(format!("{}:{}", self.listen_addr, self.listen_port))
    }

    /// 合成 kissbot-llm 的 provider + 模型参数结构
    pub fn provider_model_config(&self) -> ProviderModelConfig {
        ProviderModelConfig {
            provider_config: self.provider_config.clone(),
            default_model_config: self.default_model_config.clone(),
            model_configs: self.model_configs.clone(),
        }
    }

    /// 有效 LLM 请求参数（生成参数按 `has_*` 标志取值，与 kissbot-llm 的合并语义一致）
    pub fn effective_llm(&self) -> EffectiveLLMConfig {
        self.llm.get_effective_config()
    }

    /// 有效模型参数（provider 默认 ← model 覆盖）
    pub fn effective_model(&self) -> EffectiveModelConfig {
        self.provider_model_config()
            .get_effective_config(self.effective_llm().model.as_str())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_config() -> serde_json::Value {
        json!({
            "provider": {
                "provider_type": "openai",
                "base_url": "https://api.deepseek.com",
                "api_key": "sk-test"
            },
            "default_model_config": { "max_tokens_usage": 131072, "timeout_secs": 60, "retry_count": 3 },
            "model_configs": { "deepseek-flash": { "max_tokens_usage": 131072 } },
            "llm": {
                "provider": "deepseek", "model": "deepseek-flash",
                "max_tokens": 4096, "has_max_tokens": true
            }
        })
    }

    #[test]
    fn documented_example_parses_with_defaults() {
        // 文档示例的 `provider` 键名作为 provider_config 的别名；未配字段取默认值
        let cfg: Config = serde_json::from_value(base_config()).unwrap();
        assert_eq!(cfg.listen_addr, DEFAULT_LISTEN_ADDR);
        assert_eq!(cfg.listen_port, DEFAULT_LISTEN_PORT);
        assert_eq!(cfg.max_batch_records, DEFAULT_MAX_BATCH_RECORDS);
        assert_eq!(cfg.ws_reconnect_interval_secs, DEFAULT_WS_RECONNECT_INTERVAL_SECS);
        assert_eq!(cfg.session_max_messages, DEFAULT_SESSION_MAX_MESSAGES);
        
        assert!(cfg.notify_url.is_none());
        assert_eq!(cfg.provider_config.provider_type.as_str(), "openai");
        assert_eq!(cfg.llm.model.as_deref(), Some("deepseek-flash"));
    }

    #[test]
    fn provider_config_key_also_accepted() {
        let mut value = base_config();
        let provider = value.as_object_mut().unwrap().remove("provider").unwrap();
        value.as_object_mut().unwrap().insert("provider_config".into(), provider);
        let cfg: Config = serde_json::from_value(value).unwrap();
        assert_eq!(cfg.provider_config.base_url.as_str(), "https://api.deepseek.com");
    }

    #[test]
    fn default_model_config_requires_max_tokens_usage() {
        // max_tokens_usage 必填：省略 → 解析失败（与 kissbot-llm 的 ModelConfig 一致）
        let mut value = base_config();
        value["default_model_config"] = json!({ "timeout_secs": 60, "retry_count": 3 });
        assert!(serde_json::from_value::<Config>(value).is_err());

        // 单模型覆盖同样要求 max_tokens_usage
        let mut value = base_config();
        value["model_configs"] = json!({ "deepseek-flash": { "timeout_secs": 30 } });
        assert!(serde_json::from_value::<Config>(value).is_err());
    }

    #[test]
    fn explicit_values_override_defaults() {
        let mut value = base_config();
        value["listen_port"] = json!(9101);
        value["max_batch_records"] = json!(50);
        value["notify_url"] = json!("ws://127.0.0.1:8182");
        let cfg: Config = serde_json::from_value(value).unwrap();
        assert_eq!(cfg.listen_addr(), "127.0.0.1:9101");
        assert_eq!(cfg.max_batch_records, 50);
        assert_eq!(cfg.notify_url.as_deref(), Some("ws://127.0.0.1:8182"));
    }

    #[test]
    fn effective_configs_merge_model_override() {
        let cfg: Config = serde_json::from_value(base_config()).unwrap();
        let llm = cfg.effective_llm();
        assert_eq!(llm.provider.as_str(), "deepseek");
        assert_eq!(llm.model.as_str(), "deepseek-flash");
        assert_eq!(llm.max_tokens, Some(4096), "has_max_tokens=true 时生效");
        let model = cfg.effective_model();
        assert_eq!(model.max_tokens_usage, 131072);
        assert_eq!(model.timeout_secs, 60);
        assert_eq!(model.retry_count, 3);
    }

    #[test]
    fn llm_generation_params_require_has_flags() {
        // max_tokens 与 has_max_tokens 必须成对出现：只有 max_tokens 时不生效
        let mut value = base_config();
        value["llm"] = json!({ "provider": "deepseek", "model": "deepseek-flash", "max_tokens": 4096 });
        let cfg: Config = serde_json::from_value(value).unwrap();
        assert_eq!(cfg.effective_llm().max_tokens, None, "缺 has_max_tokens 时忽略 max_tokens");
    }

    #[test]
    fn storage_root_comes_from_memory_root_dir() {
        // 记忆根目录取自 kissbot-memory 的 memory.root_dir（组件不单独配置）
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.json");
        std::fs::write(&config_path, format!(r#"{{"memory":{{"root_dir":"{}"}}}}"#, temp.path().display())).unwrap();
        // SAFETY: 测试内单线程设置环境变量；kissbot-config/kissbot-memory 均为进程级 OnceLock 单例
        unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
        let cfg: Config = serde_json::from_value(base_config()).unwrap();
        assert_eq!(cfg.storage_root(), temp.path());
    }
}
