use std::sync::OnceLock;

use arcstr::ArcStr;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    pub api_key: ArcStr,
    pub admin_api_key: ArcStr,
}

impl SecurityConfig {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<SecurityConfig> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("security")
        })
    }
}
