use std::sync::Arc;

use tracing::info;

mod channel_manager;
mod command_router;
mod config_manager;
mod coordinator;
mod ego_md;
mod http_server;
mod model_client;
mod provider;
mod memory_reader;
mod memory_store_client;
mod session_manager;
mod station;
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

    // 2. 初始化 Coordinator（装配 + 注册单例；连接与启动动作在 run() 中执行）
    coordinator::AgentCoordinator::new(config.clone())
        .await
        .expect("初始化 Coordinator 失败");

    // 3. 启动管理 API 服务器（后台，监听 config 的 mgmt_host:mgmt_port）
    let mgr_config = config.clone();
    tokio::spawn(async move {
        let server = http_server::HttpServer::new(mgr_config);
        if let Err(e) = server.start().await {
            tracing::error!("管理 API 服务器退出: {:?}", e);
        }
    });

    // 4. 运行主循环（内部：绑定 agent/会话 + 连接全部 channel + 保持进程）
    info!("进入主循环");
    coordinator::AgentCoordinator::instance().run().await;
}
