use serde::Deserialize;
use std::sync::Arc;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    pub api_key: Arc<String>,
    pub admin_api_key: Arc<String>,
}

impl SecurityConfig {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<SecurityConfig> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("security")
        })
    }
}
