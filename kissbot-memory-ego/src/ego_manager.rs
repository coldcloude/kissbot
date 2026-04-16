use std::path::PathBuf;

use crate::error::Result;
use crate::path;
use kissbot_memory::{AgentManager, AgentMetadata, Config};

pub struct EgoManager {
    root_dir: PathBuf,
    agent_manager: &'static AgentManager,
}

impl EgoManager {
    pub fn new() -> Self {
        let config = Config::get();
        Self {
            root_dir: config.root_dir.clone(),
            agent_manager: AgentManager::get(),
        }
    }

    async fn generate_identity_md(&self, agent: &AgentMetadata) -> String {
        format!(
            "# Agent Identity\n\n- **Name**: {}\n- **ID**: {}\n- **Created At**: {}\n",
            agent.name, agent.id, agent.created_at
        )
    }

    pub async fn ensure_identity_md(&self, agent_id: &str) -> Result<()> {
        let agent = self.agent_manager.get_agent(agent_id).await?;
        
        let identity_path = path::identity_md_path(&self.root_dir, agent_id);
        
        if !identity_path.exists() {
            let content = self.generate_identity_md(&agent).await;
            tokio::fs::write(identity_path, content).await?;
        }
        
        Ok(())
    }

    pub async fn get_identity_md(&self, agent_id: &str) -> Result<String> {
        self.ensure_identity_md(agent_id).await?;
        let identity_path = path::identity_md_path(&self.root_dir, agent_id);
        let content = tokio::fs::read_to_string(identity_path).await?;
        Ok(content)
    }

    pub async fn get_user_recognition_md(&self, agent_id: &str) -> Result<String> {
        let user_recognition_path = path::user_recognition_md_path(&self.root_dir, agent_id);
        
        if !user_recognition_path.exists() {
            return Err(crate::error::Error::SettingNotFound(
                "user-recognition.md".to_string()
            ));
        }
        
        let content = tokio::fs::read_to_string(user_recognition_path).await?;
        Ok(content)
    }
}
