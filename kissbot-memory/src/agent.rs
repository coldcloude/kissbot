use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::Result;
use crate::error::Error;
use crate::path;
use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
}

type AgentLock = Arc<RwLock<Option<AgentMetadata>>>;
type ManagerLock = Arc<RwLock<HashMap<String, AgentLock>>>;

pub struct AgentManager {
    root_dir: PathBuf,
    manager_lock: ManagerLock,
}

static AGENT_MANAGER_INSTANCE: OnceLock<AgentManager> = OnceLock::new();

impl AgentManager {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
            manager_lock: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get() -> &'static Self {
        AGENT_MANAGER_INSTANCE.get_or_init(|| {
            let config = Config::get();
            AgentManager::new(&config.root_dir)
        })
    }

    async fn get_or_create_lock(&self, agent_id: &str) -> AgentLock {
        //先尝试读取已存在的锁
        {
            let locks = self.manager_lock.read().await;
            if let Some(lock) = locks.get(agent_id) {
                return lock.clone();
            }
        }
        
        //否则新建锁
        let mut locks = self.manager_lock.write().await;
        locks.entry(agent_id.to_string()).or_insert_with(|| Arc::new(RwLock::new(None))).clone()
    }

    async fn ensure_dir_exists(&self, path: PathBuf) -> Result<PathBuf> {
        if !(path.exists() && path.is_dir()) {
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
        //先尝试读内存
        {
            let lock = self.get_or_create_lock(agent_id).await;
            let guard = lock.read().await;
            if let Some(metadata) = guard.clone() {
                return Ok(metadata);
            }
        }

        //无数据，从文件读取
        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;

        let metadata_path = path::agent_metadata_path(&self.root_dir, agent_id);

        if !metadata_path.exists() {
            return Err(Error::AgentNotFound(agent_id.to_string()));
        }
        
        let content = tokio::fs::read_to_string(metadata_path).await?;
        let metadata: AgentMetadata = serde_json::from_str(&content)?;
        *guard = Some(metadata.clone());
        Ok(metadata)
    }

    async fn write_agent_metadata(&self, metadata: &AgentMetadata) -> Result<()> {
        let lock = self.get_or_create_lock(&metadata.id).await;
        let mut guard = lock.write().await;

        self.ensure_agent_dir(&metadata.id).await?;
        let metadata_path = path::agent_metadata_path(&self.root_dir, &metadata.id);
        
        let content = serde_json::to_string_pretty(metadata)?;
        tokio::fs::write(metadata_path, content).await?;
        *guard = Some(metadata.clone());
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
        
        self.write_agent_metadata(&metadata).await?;
        
        Ok(metadata)
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<AgentMetadata> {
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
                    match self.read_agent_metadata(agent_id).await {
                        Ok(metadata) => {
                            agents.push(metadata);
                        }
                        Err(_e) => {
                        }
                    }
                }
            }
        }

        Ok(agents)
    }

    pub async fn update_agent_name(&self, agent_id: &str, name: String) -> Result<AgentMetadata> {
        let metadata = self.read_agent_metadata(agent_id).await?;

        let new_metadata = AgentMetadata {
            name: name,
            ..metadata
        };

        self.write_agent_metadata(&new_metadata).await?;

        Ok(new_metadata)
    }

    pub async fn update_agent_description(&self, agent_id: &str, description: String) -> Result<AgentMetadata> {
        let metadata = self.read_agent_metadata(agent_id).await?;

        let new_metadata = AgentMetadata {
            description: description,
            ..metadata
        };

        self.write_agent_metadata(&new_metadata).await?;

        Ok(new_metadata)
    }

    pub async fn update_agent_name_description(&self, agent_id: &str, name: String, description: String) -> Result<AgentMetadata> {
        let metadata = self.read_agent_metadata(agent_id).await?;

        let new_metadata = AgentMetadata {
            name: name,
            description: description,
            ..metadata
        };

        self.write_agent_metadata(&new_metadata).await?;

        Ok(new_metadata)
    }
}
