mod error;
mod attachment;
mod channel;
mod messenger;
mod http;

use std::sync::Arc;

use tokio::net::TcpListener;
use kissbot_security::{AuthLayer, SimpleApiKeyValidator};

use crate::messenger::WebMessenger;
use crate::attachment::AttachmentStore;
use crate::http::{AppState, SseDispatcher};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // 1. 读取配置并创建 WebMessenger（内部持有 Arc<RwLock<MessengerConfig>>）
    let messenger = Arc::new(
        WebMessenger::load("kissbot-channel-web-config.json")
            .await
            .expect("Failed to load messenger config")
    );

    // 2. 创建附件存储
    let attachment_store = Arc::new(AttachmentStore::new("attachments"));

    // 3. 创建 SSE 分发器（独立于 Channel 体系，只为 admin 前端推送）
    let sse = Arc::new(SseDispatcher::new());

    // 4. 创建 ChannelManager（启动 WSS 服务供 nexus 连接）
    let channel_manager = Arc::new(kissbot_channel::ChannelManager::new(
        "http://127.0.0.1:8102", // memory-store 地址
        messenger.user_key().await,
    ));

    // 5. 注册 WebMessenger 到 ChannelManager
    kissbot_channel::ChannelManager::register_messenger(
        channel_manager.clone(),
        "web",
        messenger.clone() as Arc<dyn kissbot_channel::Messenger>,
    ).expect("Failed to register messenger");

    // 6. 启动 ChannelManager WSS 服务器（后台）
    let cm_clone = channel_manager.clone();
    tokio::spawn(async move {
        kissbot_channel::ChannelManager::start(cm_clone, "127.0.0.1:8201").await
            .expect("Failed to start ChannelManager");
    });

    // 7. 创建 HTTP 服务器（REST API + SSE）
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
