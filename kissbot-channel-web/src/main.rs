mod error;
mod config;
mod attachment;
mod messenger;
mod http;

use std::sync::Arc;

use kissbot_channel::ChannelManager;
use tokio::net::TcpListener;
use kissbot_security::{AuthLayer, SecurityConfig, SimpleApiKeyValidator};

use crate::config::Config;
use crate::messenger::WebMessengerCreator;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // 1. 读取元配置
    let config = Config::get();
    let security = SecurityConfig::get();

    // 2. 读取 messenger 配置，构造 Creator
    let creator = WebMessengerCreator::new(&config.messenger_repo, &config.attachment_dir).await
    .expect("Failed to load messenger config");

    // 3. 创建 ChannelManager
    let channel_manager = Arc::new(ChannelManager::new());

    // 4. 注册 WebMessenger
    let mid = creator.messenger_id().await;
    let messenger = channel_manager.register_messenger(
        &mid,
        creator
    ).await.expect("Failed to register messenger");

    // 5. 启动 ChannelManager WS 服务器（后台）
    let ws_addr = config.ws_listen_addr.clone();
    tokio::spawn(async move {
        channel_manager.start(&ws_addr).await
        .expect("Failed to start ChannelManager");
    });

    // 6. 创建 HTTP 服务器
    let app = http::create_router(messenger.clone())
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(security.admin_api_key.clone()))));

    let addr = config.http_listen_addr.clone();
    let listener = TcpListener::bind(&addr).await.unwrap();
    println!("kissbot-channel-web HTTP server listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
