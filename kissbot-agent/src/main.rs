use tracing::info;

mod channel_manager;
mod command_router;
mod config_manager;
mod nexus;
mod ego_md;
mod http_server;
mod model_client;
mod provider;
mod memory_ego_client;
mod memory_store_client;
mod message;
mod session_manager;
mod station;
mod station_client;
mod station_http;
mod types;

#[tokio::main]
async fn main() {
    // 初始化日志（使用环境变量 RUST_LOG 控制级别）
    tracing_subscriber::fmt::init();

    info!("kissbot-agent 启动");

    // 1. 加载配置并注册全局单例（KISSBOT_CONFIG agent 段 → AgentConfig，按 data_dir 引导 NexusRepo/StationRepo）
    config_manager::ConfigManager::new().await
        .expect("初始化配置失败");

    // 2. 构建全局 Station 单例（读 station.json：本地 toolkit + 直接子 Station）
    station::Station::new().await
        .expect("初始化 Station 失败");

    // 3. 初始化 Nexus（装配 + 注册单例；连接与启动动作在 run() 中执行）
    nexus::Nexus::new().await
        .expect("初始化 Nexus 失败");

    // 4. 启动管理 API 服务器（后台，监听 ConfigManager 单例的 mgmt_host:mgmt_port）
    tokio::spawn(async move {
        let server = http_server::HttpServer::new();
        if let Err(e) = server.start().await {
            tracing::error!("管理 API 服务器退出: {:?}", e);
        }
    });

    // 5. 启动 station HTTP 服务器（后台，独立 station_host:station_port；供其他 station 作为 sub 调用）
    tokio::spawn(async move {
        let server = station_http::StationHttpServer::new();
        if let Err(e) = server.start().await {
            tracing::error!("station HTTP 服务器退出: {:?}", e);
        }
    });

    // 6. 运行主循环（内部：绑定 agent/会话 + 连接全部 channel + 保持进程）
    info!("进入主循环");
    nexus::Nexus::get().run().await;
}
