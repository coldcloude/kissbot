use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::Arc;
use parking_lot::RwLock;
use uuid::Uuid;

use crate::error::Result;
use crate::path;
use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
}

pub struct AgentManager {
    root_dir: PathBuf,
    file_locks: Arc<RwLock<HashMap<String, Arc<RwLock<()>>>>>,
}

static AGENT_MANAGER_INSTANCE: OnceLock<AgentManager> = OnceLock::new();

impl AgentManager {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
            file_locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get() -> &'static Self {
        AGENT_MANAGER_INSTANCE.get_or_init(|| {
            let config = Config::get();
            AgentManager::new(&config.root_dir)
        })
    }

    fn get_or_create_lock(&self, agent_id: &str) -> Arc<RwLock<()>> {
        {
            let locks = self.file_locks.read();
            if let Some(lock) = locks.get(agent_id) {
                return lock.clone();
            }
        }
        
        let mut locks = self.file_locks.write();
        locks.entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    }

    fn dir_exists(&self, path: impl AsRef<Path>) -> bool {
        path.as_ref().exists() && path.as_ref().is_dir()
    }

    async fn ensure_dir_exists(&self, path: PathBuf) -> Result<PathBuf> {
        if !self.dir_exists(&path) {
            tokio::fs::create_dir_all(&path).await?;
        }
        Ok(path)
    }

    pub async fn ensure_root_dir(&self) -> Result<PathBuf> {
        let path = self.root_dir.clone();
        self.ensure_dir_exists(path).await
    }

    pub async fn ensure_agent_dir(&self, agent_id: &str) -> Result<PathBuf> {
        let path = path::agent_dir(&self.root_dir, agent_id);
        self.ensure_dir_exists(path).await
    }

    pub async fn ensure_agent_ego_dir(&self, agent_id: &str) -> Result<PathBuf> {
        let path = path::agent_ego_dir(&self.root_dir, agent_id);
        self.ensure_dir_exists(path).await
    }

    pub async fn ensure_agent_store_dir(&self, agent_id: &str) -> Result<PathBuf> {
        let path = path::agent_store_dir(&self.root_dir, agent_id);
        self.ensure_dir_exists(path).await
    }

    pub async fn ensure_agent_struct_dir(&self, agent_id: &str, struct_name: &str) -> Result<PathBuf> {
        let path = path::agent_struct_dir(&self.root_dir, agent_id, struct_name);
        self.ensure_dir_exists(path).await
    }

    async fn read_agent_metadata(&self, agent_id: &str) -> Result<AgentMetadata> {
        let metadata_path = path::agent_metadata_path(&self.root_dir, agent_id);
        
        if !metadata_path.exists() {
            return Err(crate::error::Error::AgentNotFound(agent_id.to_string()));
        }
        
        let content = tokio::fs::read_to_string(metadata_path).await?;
        let metadata = serde_json::from_str(&content)?;
        Ok(metadata)
    }

    async fn write_agent_metadata(&self, metadata: &AgentMetadata) -> Result<()> {
        self.ensure_agent_dir(&metadata.id).await?;
        let metadata_path = path::agent_metadata_path(&self.root_dir, &metadata.id);
        
        let content = serde_json::to_string_pretty(metadata)?;
        tokio::fs::write(metadata_path, content).await?;
        Ok(())
    }

    pub async fn create_agent(&self, name: String, description: String) -> Result<AgentMetadata> {
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
        let metadata = AgentMetadata {
            id,
            name,
            description,
            created_at,
        };
        
        let lock = self.get_or_create_lock(&metadata.id);
        let _guard = lock.write();
        
        self.write_agent_metadata(&metadata).await?;
        
        self.ensure_agent_ego_dir(&metadata.id).await?;
        self.ensure_agent_store_dir(&metadata.id).await?;
        
        Ok(metadata)
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<AgentMetadata> {
        let lock = self.get_or_create_lock(agent_id);
        let _guard = lock.read();
        
        self.read_agent_metadata(agent_id).await
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentMetadata>> {
        let mut agents = Vec::new();
        
        let root_dir = self.ensure_root_dir().await?;
        
        let mut entries = tokio::fs::read_dir(root_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                if let Some(agent_id) = path.file_name().and_then(|n| n.to_str()) {
                    let lock = self.get_or_create_lock(agent_id);
                    let _guard = lock.read();

                    match self.read_agent_metadata(agent_id).await {
                        Ok(metadata) => {
                            agents.push(metadata);
                        }
                        Err(_e) => {
                            //ignore
                        }
                    }
                }
            }
        }
        
        agents.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        
        Ok(agents)
    }

    pub async fn update_agent_name(&self, agent_id: &str, name: String) -> Result<AgentMetadata> {
        let lock = self.get_or_create_lock(agent_id);
        let _guard = lock.write();
        
        let mut metadata = self.read_agent_metadata(agent_id).await?;
        metadata.name = name;
        self.write_agent_metadata(&metadata).await?;
        
        Ok(metadata)
    }

    pub async fn update_agent_description(&self, agent_id: &str, description: String) -> Result<AgentMetadata> {
        let lock = self.get_or_create_lock(agent_id);
        let _guard = lock.write();
        
        let mut metadata = self.read_agent_metadata(agent_id).await?;
        metadata.description = description;
        self.write_agent_metadata(&metadata).await?;
        
        Ok(metadata)
    }
}
