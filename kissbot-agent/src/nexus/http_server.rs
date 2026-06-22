use std::sync::Arc;

use tokio::net::TcpListener;
use tracing::{info, error};

use crate::nexus::config_manager::ConfigManager;
use crate::nexus::types::Result;

/// 管理 REST API 服务器（本期骨架，供管理界面对接）
pub struct HttpServer {
    config: Arc<ConfigManager>,
    port: u16,
}

impl HttpServer {
    pub fn new(config: Arc<ConfigManager>, port: u16) -> Self {
        Self { config, port }
    }

    /// 启动 HTTP 服务器（阻塞，在协程中运行）
    pub async fn start(&self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.port);
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| crate::nexus::types::Error::IoError(e.to_string()))?;

        info!("管理 API 服务器启动: {}", addr);

        loop {
            match listener.accept().await {
                Ok((_stream, addr)) => {
                    info!("收到管理 API 连接: {}", addr);
                    // 暂时 drop，后续使用 axum/actix-web 实现完整路由
                }
                Err(e) => {
                    error!("管理 API 接受连接失败: {:?}", e);
                }
            }
        }
    }
}
