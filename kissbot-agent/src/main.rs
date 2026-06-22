use std::sync::Arc;

use tracing::info;

mod nexus;

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
        nexus::config_manager::ConfigManager::load(&config_path).await
            .expect("加载配置失败")
    );

    let agent_id = config.agent_id().await;
    info!("Agent ID: {}", agent_id);

    // 2. 初始化 MemoryWriter
    let memory_store_url = config.memory_store_url().await;
    let memory_writer = nexus::memory_writer::MemoryWriter::start(memory_store_url);

    // 3. 初始化 WSClient
    let (ws_client, external_rx) = nexus::ws_client::WSClient::new();
    let ws_client = Arc::new(ws_client);

    // 4. 连接所有配置的 channel
    let bindings = config.channel_bindings().await;
    for binding in &bindings {
        let messenger_id = binding.messenger_id.clone();
        let user_id = binding.user_id.clone();
        let ws_url = format!("ws://localhost:8080/ws"); // 占位，需要从配置获取
        let role_name = config.current_role().await;
        let agent_id = agent_id.clone();

        let client = ws_client.clone();
        tokio::spawn(async move {
            client.connect_channel(
                &ws_url, &messenger_id, &user_id, &agent_id, &role_name,
            ).await;
        });
    }

    // 5. 初始化 AgentCoordinator
    let coordinator = nexus::coordinator::AgentCoordinator::new(
        config.clone(),
        external_rx,
        ws_client.clone(),
        memory_writer,
    ).await.expect("初始化 Coordinator 失败");

    // 6. 启动管理 API 服务器（后台）
    let mgr_config = config.clone();
    tokio::spawn(async move {
        let server = nexus::http_server::HttpServer::new(mgr_config, 9090);
        if let Err(e) = server.start().await {
            tracing::error!("管理 API 服务器退出: {:?}", e);
        }
    });

    // 7. 运行主循环
    info!("进入主循环");
    coordinator.run().await;
}
