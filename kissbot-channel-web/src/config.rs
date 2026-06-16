use std::path::PathBuf;

use serde::Deserialize;

/// 元配置：不参与热更新，启动时一次性读取
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// WebMessenger 配置文件路径
    pub messenger_config: String,
    /// 附件存储根目录
    pub attachment_dir: String,
    /// memory-store 地址
    pub memory_store_url: String,
    /// WS 监听地址
    pub ws_listen_addr: String,
    /// HTTP 监听地址
    pub http_listen_addr: String,
}

impl Config {
    pub fn load() -> Result<Self, config::ConfigError> {
        let config_path = std::env::var("KISSBOT_CHANNEL_WEB_CONFIG")
            .map(|p| PathBuf::from(p))
            .unwrap_or_else(|_| PathBuf::from("config.json"));

        let config = config::Config::builder()
            .add_source(config::File::from(config_path))
            .build()?;

        config.try_deserialize()
    }
}
