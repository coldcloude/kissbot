use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::Result;
use crate::error::Error;
use kissbot_memory::{DirectoryManager};

pub const AGENT_METADATA_JSON: &str = "metadata.json";

async fn agent_metadata_path(agent_id: &str) -> Result<PathBuf> {
    let agent_dir = DirectoryManager::get().ensure_agent_dir(agent_id).await?;
    let metadata_path = agent_dir.join(AGENT_METADATA_JSON);
    Ok(metadata_path)
}

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
    manager_lock: ManagerLock,
}

static AGENT_MANAGER_INSTANCE: OnceLock<AgentManager> = OnceLock::new();

impl AgentManager {
    pub fn new() -> Self {
        Self {
            manager_lock: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get() -> &'static Self {
        AGENT_MANAGER_INSTANCE.get_or_init(|| {
            AgentManager::new()
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

    async fn read_agent_metadata_ref(&self, agent_id: &str, mut op: impl FnMut(&AgentMetadata) -> Result<()>) -> Result<()> {
        let lock = self.get_or_create_lock(agent_id).await;

        //先尝试读内存
        {
            let guard = lock.read().await;
            if let Some(metadata) = guard.as_ref() {
                return op(metadata);
            }
        }

        //无数据，从文件读取
        {
            let mut guard = lock.write().await;

            //双重锁定
            if let Some(metadata) = guard.as_ref() {
                return op(metadata);
            }

            //从文件读取
            let metadata_path = agent_metadata_path(agent_id).await?;

            if !metadata_path.exists() {
                return Err(Error::AgentNotFound(agent_id.to_string()));
            }
            
            let content = tokio::fs::read_to_string(metadata_path).await?;
            let metadata: AgentMetadata = serde_json::from_str(&content)?;
            *guard = Some(metadata);
        }

        //读文件写入后重新读取
        let guard = lock.read().await;
        match guard.as_ref() {
            Some(metadata) => return op(metadata),
            None => return Err(Error::AgentNotFound(agent_id.to_string())),
        }
    }

    async fn write_agent_metadata_ref(&self, agent_id: &str, op: impl FnOnce(Option<AgentMetadata>) -> Option<AgentMetadata>) -> Result<()> {
        let metadata_path = agent_metadata_path(agent_id).await?;

        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;

        //更新
        *guard = op(guard.take());
        
        //写入文件
        match guard.as_ref() {
            Some(metadata) => {
                let content = serde_json::to_string_pretty(metadata)?;
                tokio::fs::write(metadata_path, content).await?;
                Ok(())
            }
            None => {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }
    }

    async fn write_agent_metadata(&self, metadata: AgentMetadata) -> Result<()> {
        let lock = self.get_or_create_lock(&metadata.id).await;
        let mut guard = lock.write().await;

        let metadata_path = agent_metadata_path(&metadata.id).await?;

        let content = serde_json::to_string_pretty(&metadata)?;
        tokio::fs::write(metadata_path, content).await?;
        *guard = Some(metadata);
        Ok(())
    }

    pub async fn create_agent(&self, name: String, description: String) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let metadata = AgentMetadata {
            id,
            name,
            description,
            created_at,
        };

        self.write_agent_metadata(metadata).await
    }

    pub async fn get_metadata_clone(&self, agent_id: &str) -> Result<AgentMetadata> {
        let mut metadata_option: Option<AgentMetadata> = None;
        
        self.read_agent_metadata_ref(agent_id, |metadata| {
            metadata_option = Some(metadata.clone());
            Ok(())
        }).await?;
        
        match metadata_option {
            Some(metadata) => Ok(metadata),
            None => Err(Error::AgentNotFound(agent_id.to_string()))
        }
    }

    pub async fn get_metadata(&self, agent_id: &str, op: impl FnMut(&AgentMetadata) -> Result<()>) -> Result<()> {
        self.read_agent_metadata_ref(agent_id, op).await
    }

    pub async fn update_agent_name(&self, agent_id: &str, name: String) -> Result<()> {
        self.write_agent_metadata_ref(agent_id, |metadata| {
            match metadata {
                Some(metadata) => {
                    Some(AgentMetadata {
                        name: name,
                        ..metadata
                    })
                }
                None => None
            }
        }).await
    }

    pub async fn update_agent_description(&self, agent_id: &str, description: String) -> Result<()> {
        self.write_agent_metadata_ref(agent_id, |metadata| {
            match metadata {
                Some(metadata) => {
                    Some(AgentMetadata {
                        description,
                        ..metadata
                    })
                }
                None => None
            }
        }).await
    }

    pub async fn update_agent_name_description(&self, agent_id: &str, name: String, description: String) -> Result<()> {
        self.write_agent_metadata_ref(agent_id, |metadata| {
            match metadata {
                Some(metadata) => {
                    Some(AgentMetadata {
                        name,
                        description,
                        ..metadata
                    })
                }
                None => None
            }
        }).await
    }
}
