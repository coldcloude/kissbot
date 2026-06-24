use serde::Deserialize;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::error::Result;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub listen_addr: String,
    pub listen_port: u16,
    pub api_key: String,
}

static CONFIG_INSTANCE: OnceLock<Config> = OnceLock::new();

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = std::env::var("KISSBOT_MEMORY_EGO_CONFIG")
            .map(|p| PathBuf::from(p))
            .unwrap_or_else(|_| PathBuf::from("config.json"));

        let config = config::Config::builder()
            .add_source(config::File::from(config_path))
            .build()?;

        let config = config.try_deserialize()?;
        Ok(config)
    }

    pub fn get() -> &'static Self {
        CONFIG_INSTANCE.get_or_init(|| {
            Config::load().expect("Failed to load config from file")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ego_config_with_values() {
        let config = Config {
            listen_addr: "0.0.0.0".to_string(),
            listen_port: 9999,
            api_key: "test-key".to_string(),
        };
        assert_eq!(config.listen_addr, "0.0.0.0");
        assert_eq!(config.listen_port, 9999);
        assert_eq!(config.api_key, "test-key");
    }

    #[test]
    fn test_ego_config_load() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test-ego-config.json");
        std::fs::write(&config_path,
            r#"{"listen_addr":"127.0.0.1","listen_port":3001,"api_key":"abc123"}"#).unwrap();
        // SAFETY: 单线程测试
        unsafe { std::env::set_var("KISSBOT_MEMORY_EGO_CONFIG", config_path.to_str().unwrap()); }
        let config = Config::load().unwrap();
        assert_eq!(config.listen_addr, "127.0.0.1");
        assert_eq!(config.listen_port, 3001);
        assert_eq!(config.api_key, "abc123");
        unsafe { std::env::remove_var("KISSBOT_MEMORY_EGO_CONFIG"); }
    }
}
