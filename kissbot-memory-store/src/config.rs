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
        let config_path = std::env::var("KISSBOT_MEMORY_STORE_CONFIG")
            .map(|p| PathBuf::from(p))
            .unwrap_or_else(|_| PathBuf::from("memory-store-config.json"));

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
    fn test_config_load_ok() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test-config.json");
        let json_content = format!(
            r#"{{"listen_addr": "127.0.0.1", "listen_port": 8082, "api_key": "test-key-123"}}"#
        );
        std::fs::write(&config_path, json_content).unwrap();

        // SAFETY: test environment, single-threaded, no concurrent env access
        unsafe { std::env::set_var("KISSBOT_MEMORY_STORE_CONFIG", config_path.to_str().unwrap()); }
        let config = Config::load().unwrap();
        assert_eq!(config.listen_addr, "127.0.0.1");
        assert_eq!(config.listen_port, 8082);
        assert_eq!(config.api_key, "test-key-123");
        unsafe { std::env::remove_var("KISSBOT_MEMORY_STORE_CONFIG"); }
    }

    #[test]
    fn test_config_load_error() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("nonexistent-config.json");

        unsafe { std::env::set_var("KISSBOT_MEMORY_STORE_CONFIG", config_path.to_str().unwrap()); }
        let result = Config::load();
        assert!(result.is_err());
        unsafe { std::env::remove_var("KISSBOT_MEMORY_STORE_CONFIG"); }
    }
}
