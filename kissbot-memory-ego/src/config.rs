use serde::Deserialize;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::error::Result;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub listen_addr: String,
    pub listen_port: u16,
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
