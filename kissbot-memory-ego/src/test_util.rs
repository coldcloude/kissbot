use std::sync::Once;

/// 初始化 kissbot_memory::Config 使其指向一个临时目录。
/// 多次调用只生效一次。
/// TempDir 在闭包结束时 drop，但 Config::get() 已将 root_dir 读入 OnceLock 内存。
pub fn init_test_config() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let dir = tempfile::tempdir().expect("create tempdir");
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        std::fs::write(&config_path, format!(r#"{{"root_dir":"{}"}}"#, root_dir_str))
            .expect("write config");
        // SAFETY: 单线程测试环境，无并发 env 访问
        unsafe { std::env::set_var("KISSBOT_MEMORY_CONFIG", config_path.to_str().unwrap()); }
        kissbot_memory::Config::get();
    });
}

/// 在 init_test_config 基础上，进一步为所有已有 agent 目录补充 metadata.json。
/// 确保后续 SearchManager::get() 初始化时不会因缺少 metadata.json 而失败。
pub async fn ensure_agent_metadata() {
    init_test_config();
    let dm = kissbot_memory::DirectoryManager::get();
    let agents = dm.list_agents().await.unwrap_or_default();
    for agent_id in &agents {
        let agent_dir = dm.ensure_agent_dir(agent_id).await.unwrap();
        let meta_path = agent_dir.join("metadata.json");
        if !meta_path.exists() {
            let metadata = serde_json::json!({
                "agent_id": agent_id,
                "individual_name": agent_id,
                "description": "setup agent",
                "created_at": "2026-06-25 10:00:00"
            });
            tokio::fs::write(&meta_path, serde_json::to_string_pretty(&metadata).unwrap())
                .await
                .unwrap();
        }
        dm.ensure_agent_ego_dir(agent_id).await.unwrap();
    }
}
