use std::sync::Arc;

use tracing::info;

mod command_router;
mod config_manager;
mod context_builder;
mod coordinator;
mod http_server;
mod model_client;
mod memory_reader;
mod memory_store_client;
mod memory_writer;
mod mode_manager;
mod repo;
mod station_client;
mod station_router;
mod types;

#[tokio::main]
async fn main() {
    // 初始化日志（使用环境变量 RUST_LOG 控制级别）
    tracing_subscriber::fmt::init();

    info!("kissbot-agent 启动");

    // 配置文件路径（使用环境变量或默认路径）
    let config_path = std::env::var("CONFIG_PATH")
        .unwrap_or_else(|_| "config.json".to_string());

    // 1. 加载配置
    info!("加载配置: {}", config_path);
    let config = Arc::new(
        config_manager::ConfigManager::load(&config_path).await
            .expect("加载配置失败")
    );

    info!("Agent ID: {}", config.agent_id());

    // 2. 初始化 MemoryWriter（思考/工具调用推送，非 channel 消息）
    let memory_writer = memory_writer::MemoryWriter::start();

    // 3. 初始化 Coordinator（内部启动 ChannelClient 连接和 MemoryStoreClient）
    let coordinator = coordinator::AgentCoordinator::new(config.clone(), memory_writer)
        .await
        .expect("初始化 Coordinator 失败");

    // 4. 启动管理 API 服务器（后台）
    let mgr_config = config.clone();
    tokio::spawn(async move {
        let server = http_server::HttpServer::new(mgr_config, 9090);
        if let Err(e) = server.start().await {
            tracing::error!("管理 API 服务器退出: {:?}", e);
        }
    });

    // 5. 运行主循环
    info!("进入主循环");
    coordinator.run().await;
}
