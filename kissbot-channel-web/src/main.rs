mod error;
mod attachment;
mod messenger;
mod http;

use std::sync::Arc;

use tokio::net::TcpListener;
use kissbot_security::{AuthLayer, SimpleApiKeyValidator};

use crate::messenger::{WebMessenger, WebMessengerCreator};
use crate::http::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // 1. 读取完整配置，构造 Creator
    let creator = Arc::new(
        WebMessengerCreator::new("kissbot-channel-web-config.json")
            .await
            .expect("Failed to load config")
    );

    // 2. 创建 ChannelManager
    let channel_manager = Arc::new(kissbot_channel::ChannelManager::new(
        "http://127.0.0.1:8102",
        creator.user_key().await,
    ));

    // 3. 注册 WebMessenger，返回 Arc<WebMessenger>
    //    WebMessenger 内部自行创建 SseDispatcher 和 AttachmentStore，
    //    通过 messenger.sse / messenger.attachment_store 公开访问
    let mid = creator.messenger_id().await;
    let messenger = kissbot_channel::ChannelManager::register_messenger(
        channel_manager.clone(),
        &mid,
        creator.clone() as Arc<dyn kissbot_channel::MessengerCreator<WebMessenger>>,
    ).await.expect("Failed to register messenger");

    // 4. 启动 ChannelManager WSS 服务器（后台）
    let cm_clone = channel_manager.clone();
    tokio::spawn(async move {
        kissbot_channel::ChannelManager::start(cm_clone, "127.0.0.1:8201").await
            .expect("Failed to start ChannelManager");
    });

    // 5. 创建 HTTP 服务器
    let app_state = AppState {
        messenger: messenger.clone(),
    };

    let app = http::create_router(app_state)
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(messenger.admin_key().await))));

    let addr = "127.0.0.1:8301";
    let listener = TcpListener::bind(addr).await.unwrap();
    println!("kissbot-channel-web HTTP server listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
