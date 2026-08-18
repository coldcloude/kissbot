mod error;
mod config;
mod attachment;
mod messenger;
mod http;
mod message_store;

use std::sync::Arc;

use kissbot_channel::ChannelServer;
use tokio::net::TcpListener;
use kissbot_security::{AuthLayer, SecurityConfig, SimpleApiKeyValidator};
use tokio::select;
use tower_http::cors::CorsLayer;

use crate::config::Config;
use crate::messenger::WebMessengerCreator;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // 1. 读取元配置
    let config = Config::get();
    let security = SecurityConfig::get();

    // 2. 读取 messenger 配置，构造 Creator
    let creator = WebMessengerCreator::new(
        &config.messenger_repo, &config.attachment_dir, &config.message_dir,
        &config.messenger_id, &config.admin_name,
    ).await
    .expect("Failed to load messenger config");

    // 3. 创建 ChannelServer
    let channel_manager = Arc::new(ChannelServer::new());

    // 4. 注册 WebMessenger
    let mid = creator.messenger_id().await;
    let messenger = channel_manager.clone().register_messenger(
        &mid,
        creator
    ).await.expect("Failed to register messenger");

    // 5. 准备 ChannelServer WS 服务器
    let ws_addr = config.ws_listen_addr.clone();

    // 6. 创建 HTTP 服务器
    let app = http::create_router(messenger.clone())
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(security.admin_api_key.clone()))))
        .layer(CorsLayer::permissive());

    let addr = config.http_listen_addr.clone();
    let listener = TcpListener::bind(&addr).await.unwrap();
    println!("kissbot-channel-web HTTP server listening on {}", addr);

    // 7. 启动所有服务器
    select! {
        r = channel_manager.start(&ws_addr) => {
            r.expect("Failed to start ChannelServer");
        }
        r = axum::serve(listener, app) => {
            r.expect("HTTP server failed")
        }
    }
}
