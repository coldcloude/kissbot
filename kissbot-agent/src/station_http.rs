use std::sync::Arc;

use axum::Router;
use kissbot_mcp::{Implementation, McpHttpConfig, McpServer};
use kissbot_security::{AuthLayer, SecurityConfig, SimpleApiKeyValidator};
use tokio::net::TcpListener;
use tracing::info;

use crate::config_manager::ConfigManager;
use crate::station::Station;
use crate::types::Result;

/// station 对外 MCP 服务：单端点 `POST /mcp`，供其他 station 作为 sub 调用
pub struct StationHttpServer {
    api_key: Arc<String>,
}

impl StationHttpServer {
    pub fn new() -> Self {
        let config = SecurityConfig::get();
        Self { api_key: config.api_key.clone() }
    }

    /// 构建 Router：以全局 Station 单例为处理器，认证用已加载的 api key
    fn build_router(&self) -> Router {
        let server = McpServer::new(Station::shared(), server_info());
        kissbot_mcp::router(server, McpHttpConfig::default())
            .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(self.api_key.clone()))))
    }

    /// 启动 station MCP 服务（阻塞，在协程中运行）
    pub async fn start(&self) -> Result<()> {
        let host = ConfigManager::get().station_host().to_string();
        let port = ConfigManager::get().station_port();
        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr).await?;
        info!("station MCP 服务器启动: {} {}（端点 /mcp）", addr, kissbot_mcp::PROTOCOL_VERSION);
        axum::serve(listener, self.build_router()).await?;
        Ok(())
    }
}

fn server_info() -> Implementation {
    Implementation::new("kissbot-station", env!("CARGO_PKG_VERSION"))
}
