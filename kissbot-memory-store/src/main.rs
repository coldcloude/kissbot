mod error;
mod config;
mod record;
mod api;
mod notify;

use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};
use kissbot_security::{AuthLayer, SecurityConfig, SimpleApiKeyValidator};

use crate::config::Config;
use crate::api::create_router;
use crate::notify::NotifyServer;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::get();
    let security = SecurityConfig::get();

    let app = create_router()
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(security.api_key.clone()))));

    // 新数据通知服务：独立 WS 监听，供记忆结构实现模块连接
    let notify_addr = config.notify_addr();
    tokio::spawn(async move {
        if let Err(e) = NotifyServer::start(&notify_addr).await {
            error!("[memory-store] notify server stopped: {}", e);
        }
    });

    let addr = format!("{}:{}", config.listen_addr, config.listen_port);
    let listener = TcpListener::bind(&addr).await.unwrap();

    info!("kissbot-memory-store listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
