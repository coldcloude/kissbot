mod error;
mod config;
mod attachment;
mod messenger;
mod http;

use std::sync::Arc;

use tokio::net::TcpListener;
use kissbot_security::{AuthLayer, SimpleApiKeyValidator};

use crate::config::Config;
use crate::messenger::{WebMessenger, WebMessengerCreator};
use crate::http::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // 1. 读取元配置（messenger_config 路径、attachment_dir、memory_store_url 等）
    let app_config = Config::load().expect("Failed to load app config");

    // 2. 读取 messenger 配置，构造 Creator
    let creator = Arc::new(
        WebMessengerCreator::new(&app_config.messenger_config, &app_config.attachment_dir)
            .await
            .expect("Failed to load messenger config")
    );

    // 3. 创建 ChannelManager
    let channel_manager = Arc::new(kissbot_channel::ChannelManager::new(
        &app_config.memory_store_url,
        creator.user_key().await,
    ));

    // 4. 注册 WebMessenger
    let mid = creator.messenger_id().await;
    let messenger = kissbot_channel::ChannelManager::register_messenger(
        channel_manager.clone(),
        &mid,
        creator.clone() as Arc<dyn kissbot_channel::MessengerCreator<WebMessenger>>,
    ).await.expect("Failed to register messenger");

    // 5. 启动 ChannelManager WS 服务器（后台）
    let cm_clone = channel_manager.clone();
    let ws_addr = app_config.ws_listen_addr.clone();
    tokio::spawn(async move {
        kissbot_channel::ChannelManager::start(cm_clone, &ws_addr).await
            .expect("Failed to start ChannelManager");
    });

    // 6. 创建 HTTP 服务器
    let app_state = AppState {
        messenger: messenger.clone(),
    };

    let app = http::create_router(app_state)
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(messenger.admin_key().await))));

    let addr = app_config.http_listen_addr;
    let listener = TcpListener::bind(&addr).await.unwrap();
    println!("kissbot-channel-web HTTP server listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
