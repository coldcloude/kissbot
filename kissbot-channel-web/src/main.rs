mod error;
mod attachment;
mod messenger;
mod http;

use std::sync::Arc;

use tokio::net::TcpListener;
use kissbot_security::{AuthLayer, SimpleApiKeyValidator};

use crate::messenger::WebMessenger;
use crate::messenger::WebMessengerCreator;
use crate::attachment::AttachmentStore;
use crate::http::{AppState, SseDispatcher};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // 1. 读取完整配置，构造 Creator（同时暴露 admin_key / user_key）
    let creator = Arc::new(
        WebMessengerCreator::new("kissbot-channel-web-config.json")
            .await
            .expect("Failed to load config")
    );

    // 2. 创建 ChannelManager（使用预读取的 user_key）
    let api_key = creator.api_key().await;
    let channel_manager = Arc::new(kissbot_channel::ChannelManager::new(
        "http://127.0.0.1:8102",
        api_key,
    ));

    // 3. 注册 WebMessenger，返回 Arc<WebMessenger>
    let mid = creator.messenger_id().await;
    let messenger = kissbot_channel::ChannelManager::register_messenger(
        channel_manager.clone(),
        mid.as_str(),
        creator.clone() as Arc<dyn kissbot_channel::MessengerCreator<WebMessenger>>,
    ).await.expect("Failed to register messenger");

    // 4. 创建附件存储
    let attachment_store = Arc::new(AttachmentStore::new("attachments"));

    // 5. 创建 SSE 分发器
    let sse = Arc::new(SseDispatcher::new());

    // 6. 启动 ChannelManager WSS 服务器（后台）
    let cm_clone = channel_manager.clone();
    tokio::spawn(async move {
        kissbot_channel::ChannelManager::start(cm_clone, "127.0.0.1:8201").await
            .expect("Failed to start ChannelManager");
    });

    // 7. 创建 HTTP 服务器
    let app_state = AppState {
        messenger: messenger.clone(),
        attachment_store,
        sse,
    };

    let app = http::create_router(app_state)
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(messenger.admin_key().await))));

    let addr = "127.0.0.1:8301";
    let listener = TcpListener::bind(addr).await.unwrap();
    println!("kissbot-channel-web HTTP server listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
