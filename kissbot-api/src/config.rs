use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    pub memory_store_url: String,
    pub memory_ego_url: String,
}

impl ApiConfig {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<ApiConfig> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("api")
        })
    }
}
