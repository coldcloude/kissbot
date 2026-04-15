use std::sync::OnceLock;

use crate::agent::AgentManager;
use crate::config::Config;
use crate::directory::DirectoryManager;

static CONFIG: OnceLock<Config> = OnceLock::new();
static DIRECTORY_MANAGER: OnceLock<DirectoryManager> = OnceLock::new();
static AGENT_MANAGER: tokio::sync::OnceCell<AgentManager> = tokio::sync::OnceCell::const_new();

pub fn get_config() -> &'static Config {
    CONFIG.get_or_init(|| {
        Config::load().expect("Failed to load config from file")
    })
}

pub fn get_directory_manager() -> &'static DirectoryManager {
    DIRECTORY_MANAGER.get_or_init(|| {
        let config = get_config();
        DirectoryManager::new(&config.root_dir)
    })
}

pub async fn get_agent_manager() -> &'static AgentManager {
    AGENT_MANAGER.get_or_init(|| async {
        let config = get_config();
        AgentManager::new(&config.root_dir).await.expect("Failed to create AgentManager")
    }).await
}
