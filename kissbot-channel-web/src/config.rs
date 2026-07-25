use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub messenger_repo: String,
    pub attachment_dir: String,
    pub message_dir: String,
    pub ws_listen_addr: String,
    pub http_listen_addr: String,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("channel-web")
        })
    }
}
