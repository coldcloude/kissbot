use std::sync::Arc;

use serde::Deserialize;


fn default_station_host() -> Arc<String> {
    Arc::new("127.0.0.1".into())
}

fn default_station_port() -> u16 {
    9100
}

/// 静态配置：来自 KISSBOT_CONFIG 的 agent 段，启动后不变
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub data_dir: Arc<String>,
    pub mgmt_host: Arc<String>,
    pub mgmt_port: u16,
    /// station 对外 HTTP 服务监听地址（独立于管理 API；被其他 station 作为 sub 调用时使用）
    #[serde(default = "default_station_host")]
    pub station_host: Arc<String>,
    #[serde(default = "default_station_port")]
    pub station_port: u16,
    pub ws_reconnect_interval_secs: u64,
    // 注：default_system_prompt 由 NexusRepo（nexus.json）承载，config.json 不承载
}

impl AgentConfig {
    /// 从 kissbot-config 全局单例的 agent 段加载
    pub fn from_public_config() -> Self {
        kissbot_config::Config::get().get_section("agent")
    }
}
