use serde::Deserialize;
use std::sync::OnceLock;

use arcstr::ArcStr;

/// 通知服务默认监听地址（与 HTTP 服务分开监听，连接方式见 communication.md）
pub const DEFAULT_NOTIFY_LISTEN_ADDR: ArcStr = arcstr::literal!("127.0.0.1");
/// 通知服务默认监听端口（HTTP 8082 之外的独立端口）
pub const DEFAULT_NOTIFY_LISTEN_PORT: u16 = 8182;

fn default_notify_listen_addr() -> ArcStr {
    DEFAULT_NOTIFY_LISTEN_ADDR.clone()
}

fn default_notify_listen_port() -> u16 {
    DEFAULT_NOTIFY_LISTEN_PORT
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub listen_addr: ArcStr,
    pub listen_port: u16,
    /// 新数据通知 WS 服务监听地址
    #[serde(default = "default_notify_listen_addr")]
    pub notify_listen_addr: ArcStr,
    /// 新数据通知 WS 服务监听端口
    #[serde(default = "default_notify_listen_port")]
    pub notify_listen_port: u16,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("memory.store")
        })
    }

    pub fn notify_addr(&self) -> ArcStr {
        ArcStr::from(format!("{}:{}", self.notify_listen_addr, self.notify_listen_port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_fields_default_when_absent() {
        // 旧配置（只有 listen_addr/listen_port）仍可解析，通知服务用默认端口
        let cfg: Config = serde_json::from_str(r#"{"listen_addr":"127.0.0.1","listen_port":8082}"#).unwrap();
        assert_eq!(cfg.notify_listen_addr, DEFAULT_NOTIFY_LISTEN_ADDR);
        assert_eq!(cfg.notify_listen_port, DEFAULT_NOTIFY_LISTEN_PORT);
        assert_eq!(cfg.notify_addr(), "127.0.0.1:8182");
    }

    #[test]
    fn notify_fields_override() {
        let cfg: Config = serde_json::from_str(
            r#"{"listen_addr":"0.0.0.0","listen_port":8082,"notify_listen_addr":"0.0.0.0","notify_listen_port":9000}"#,
        ).unwrap();
        assert_eq!(cfg.notify_addr(), "0.0.0.0:9000");
    }
}
