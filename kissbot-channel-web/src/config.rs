use serde::Deserialize;
use std::sync::OnceLock;

use arcstr::ArcStr;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub messenger_id: ArcStr,
    pub admin_name: ArcStr,
    pub messenger_repo: ArcStr,
    pub attachment_dir: ArcStr,
    pub message_dir: ArcStr,
    pub ws_listen_addr: ArcStr,
    pub http_listen_addr: ArcStr,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("channel-web")
        })
    }
}
