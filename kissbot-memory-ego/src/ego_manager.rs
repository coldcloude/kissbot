use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

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
    identity_dirty: AtomicBool,
    agent_manager: &'static AgentManager,
}

impl EgoManager {
    pub fn new() -> Self {
        Self {
            identity_dirty: AtomicBool::new(true),
            agent_manager: AgentManager::get(),
        }
    }

    pub async fn sync_identity_md(&self, agent_id: &str) -> Result<()> {
        match self.identity_dirty.compare_exchange_weak(
            true, false, Ordering::Relaxed, Ordering::Relaxed
        ) {
            Ok(_) => {
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
            _ => {
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
}
