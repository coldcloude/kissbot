use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub listen_addr: String,
    pub listen_port: u16,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("memory.ego")
        })
    }
}
