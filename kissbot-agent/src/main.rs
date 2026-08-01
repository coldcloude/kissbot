use std::sync::Arc;

use tracing::info;

mod command_router;
mod config_manager;
mod coordinator;
mod http_server;
mod model_client;
mod provider;
mod memory_reader;
mod memory_store_client;
mod memory_writer;
mod session_manager;
mod station_client;
mod station_router;
mod types;

#[tokio::main]
async fn main() {
    // 初始化日志（使用环境变量 RUST_LOG 控制级别）
    tracing_subscriber::fmt::init();

    info!("kissbot-agent 启动");

    // 1. 加载配置（KISSBOT_CONFIG agent 段 → AgentConfig，按 data_dir 引导 NexusRepo/StationRepo）
    let config = Arc::new(
        config_manager::ConfigManager::new().await
            .expect("初始化配置失败")
    );

    // 运行状态由 coordinator 持有，此处记录默认 agent id
    info!("Agent ID: {}", config.default_agent_id().await);

    // 2. 初始化 MemoryWriter（思考/工具调用推送，非 channel 消息）
    let memory_writer = memory_writer::MemoryWriter::start();

    // 3. 初始化 Coordinator（内部启动 ChannelClient 连接和 MemoryStoreClient）
    let coordinator = coordinator::AgentCoordinator::new(config.clone(), memory_writer)
        .await
        .expect("初始化 Coordinator 失败");

    // 4. 启动管理 API 服务器（后台，监听 config 的 mgmt_host:mgmt_port）
    let mgr_config = config.clone();
    tokio::spawn(async move {
        let server = http_server::HttpServer::new(mgr_config);
        if let Err(e) = server.start().await {
            tracing::error!("管理 API 服务器退出: {:?}", e);
        }
    });

    // 5. 运行主循环
    info!("进入主循环");
    coordinator.run().await;
}
