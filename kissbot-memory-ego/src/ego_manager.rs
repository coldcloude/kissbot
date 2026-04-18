use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use dashmap::DashSet;
use kissbot_memory::DirectoryManager;

use crate::error::Result;
use crate::agent::AgentManager;

pub const EGO_IDENTITY_MD: &str = "identity.md";
pub const EGO_USER_RECOGNITION_MD: &str = "user-recognition.md";

pub fn ego_identity_md_path(ego_dir: impl AsRef<Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(EGO_IDENTITY_MD)
}

pub fn ego_user_recognition_md_path(ego_dir: impl AsRef<Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(EGO_USER_RECOGNITION_MD)
}

pub struct EgoManager {
    identity_dirty: DashSet<String>,
    agent_manager: &'static AgentManager,
}

static EGO_MANAGER_INSTANCE: OnceLock<EgoManager> = OnceLock::new();

impl EgoManager {
    pub fn new() -> Self {
        Self {
            identity_dirty: DashSet::new(),
            agent_manager: AgentManager::get(),
        }
    }

    pub fn get() -> &'static Self {
        EGO_MANAGER_INSTANCE.get_or_init(|| Self::new())
    }

    pub async fn sync_identity_md(&self, agent_id: &str) -> Result<()> {
        match self.identity_dirty.remove(&agent_id.to_string()) {
            Some(_) => {
                let mut content = String::from("# Agent Identity\n\n");
                self.agent_manager.get_metadata(agent_id, |metadata| {
                    content += & format!(
                        "- **Name**\n {}\n- **Created At**\n {}\n- **Description**\n {}\n",
                        metadata.name, metadata.created_at, metadata.description
                    );
                    Ok(())
                }).await?;
                Ok(())
            },
            None => {
                Ok(())
            }
        }
    }

    pub async fn get_identity_md(&self, agent_id: &str) -> Result<String> {
        self.sync_identity_md(agent_id).await?;
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;        
        let identity_path = ego_identity_md_path(&ego_dir);
        let content = tokio::fs::read_to_string(identity_path).await?;
        Ok(content)
    }

    pub async fn get_user_recognition_md(&self, agent_id: &str) -> Result<String> {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;        
        let user_recognition_path = ego_user_recognition_md_path(&ego_dir);

        if !user_recognition_path.exists() {
            return Err(crate::error::Error::SettingNotFound(
                "user-recognition.md".to_string()
            ));
        }

        let content = tokio::fs::read_to_string(user_recognition_path).await?;
        Ok(content)
    }

    pub async fn search_by_name(&self, name: &str) -> Result<Vec<String>> {
        Err(crate::error::Error::AgentNotFound(name.to_string().to_string()))
    }

    pub async fn search_by_description(&self, description: &str) -> Result<Vec<String>> {
        Err(crate::error::Error::AgentNotFound(description.to_string().to_string()))
    }
}
