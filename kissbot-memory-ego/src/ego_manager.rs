use std::{sync::atomic::{AtomicBool, Ordering}};

use crate::error::Result;
use crate::path;
use kissbot_memory::{AgentManager, AgentMetadata};

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

    async fn generate_identity_md(&self, agent: &AgentMetadata) -> String {
        format!(
            "# Agent Identity\n\n- **Name**\n {}\n- **Created At**\n {}\n- **Description**\n {}\n",
            agent.name, agent.created_at, agent.description
        )
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
        let ego_dir = self.agent_manager.ensure_agent_ego_dir(agent_id).await?;        
        let identity_path = path::identity_md_path(&ego_dir);
        let content = tokio::fs::read_to_string(identity_path).await?;
        Ok(content)
    }

    pub async fn get_user_recognition_md(&self, agent_id: &str) -> Result<String> {
        let ego_dir = self.agent_manager.ensure_agent_ego_dir(agent_id).await?;        
        let user_recognition_path = path::user_recognition_md_path(&ego_dir);
        
        if !user_recognition_path.exists() {
            return Err(crate::error::Error::SettingNotFound(
                "user-recognition.md".to_string()
            ));
        }
        
        let content = tokio::fs::read_to_string(user_recognition_path).await?;
        Ok(content)
    }
}
