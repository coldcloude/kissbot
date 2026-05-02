use chrono::Utc;
use kissbot_api::AgentMetadataGeneric;
use kissbot_api::SyncString;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::Result;
use crate::error::Error;
use kissbot_memory::DirectoryManager;

pub const AGENT_METADATA_JSON: &str = "metadata.json";

async fn agent_metadata_path(agent_id: &str) -> Result<PathBuf> {
    let agent_dir = DirectoryManager::get().ensure_agent_dir(agent_id).await?;
    let metadata_path = agent_dir.join(AGENT_METADATA_JSON);
    Ok(metadata_path)
}

pub type AgentMetadata = AgentMetadataGeneric<SyncString>;

type AgentLock = Arc<RwLock<Option<Arc<AgentMetadata>>>>;

pub struct AgentManager {
    manager_lock: dashmap::DashMap<String, AgentLock>,
}

static AGENT_MANAGER_INSTANCE: OnceLock<AgentManager> = OnceLock::new();

impl AgentManager {
    pub fn new() -> Self {
        Self {
            manager_lock: dashmap::DashMap::new(),
        }
    }

    pub fn get() -> &'static Self {
        AGENT_MANAGER_INSTANCE.get_or_init(|| {
            AgentManager::new()
        })
    }

    async fn get_or_create_lock(&self, agent_id: &str) -> AgentLock {
        self.manager_lock
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(None)))
            .clone()
    }

    async fn read_agent_metadata_ref(&self, agent_id: &str, mut op: impl FnMut(Arc<AgentMetadata>) -> Result<()>) -> Result<()> {
        let lock = self.get_or_create_lock(agent_id).await;

        {
            let guard = lock.read().await;
            if let Some(metadata) = guard.as_ref() {
                return op(metadata.clone());
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(metadata) = guard.as_ref() {
                return op(metadata.clone());
            }

            let metadata_path = agent_metadata_path(agent_id).await?;

            if !metadata_path.exists() {
                return Err(Error::AgentNotFound(agent_id.to_string()));
            }
            
            let content = tokio::fs::read_to_string(metadata_path).await?;
            let metadata: AgentMetadata = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(metadata));
        }

        let guard = lock.read().await;
        match guard.as_ref() {
            Some(metadata) => return op(metadata.clone()),
            None => return Err(Error::AgentNotFound(agent_id.to_string())),
        }
    }

    async fn write_agent_metadata_ref<F>(&self, agent_id: &str, op: F) -> Result<()>
    where
        F: FnOnce(Option<Arc<AgentMetadata>>) -> Result<Arc<AgentMetadata>>,
    {
        let metadata_path = agent_metadata_path(agent_id).await?;

        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;

        if guard.is_none() && metadata_path.exists() {
            let content = tokio::fs::read_to_string(&metadata_path).await?;
            let entity = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(entity));
        }

        let metadata = guard.take();
        match op(metadata.clone()) {
            Ok(new_metadata) => {
                *guard = Some(new_metadata);
            }
            Err(e) => {
                *guard = metadata;
                return Err(e);
            }
        }
        
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

    pub async fn create_agent(&self, name: Arc<String>, description: Arc<String>) -> Result<()> {
        let id = Arc::new(Uuid::new_v4().to_string());
        let created_at = Arc::new(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());

        let metadata = AgentMetadata {
            id,
            name,
            description,
            created_at,
        };

        self.write_agent_metadata_ref(metadata.id.clone().as_str(), |_| {
            Ok(Arc::new(metadata))
        }).await
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<Arc<AgentMetadata>> {
        let mut result = Err(Error::AgentNotFound(agent_id.to_string()));
        self.read_agent_metadata_ref(agent_id, |metadata| {
            result = Ok(metadata.clone());
            Ok(())
        }).await?;
        result
    }

    pub async fn copy_agent(&self, agent_id: &str) -> Result<()> {
        let metadata = self.get_agent(agent_id).await?;
        self.create_agent(metadata.name.clone(), metadata.description.clone()).await
    }

    pub async fn update_agent_name(&self, agent_id: &str, name: Arc<String>) -> Result<()> {
        self.write_agent_metadata_ref(agent_id, |metadata| {
            match metadata {
                Some(metadata) => {
                    Ok(Arc::new(AgentMetadata {
                        id: metadata.id.clone(),
                        name,
                        description: metadata.description.clone(),
                        created_at: metadata.created_at.clone(),
                    }))
                }
                None => Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }

    pub async fn update_agent_description(&self, agent_id: &str, description: Arc<String>) -> Result<()> {
        self.write_agent_metadata_ref(agent_id, |metadata| {
            match metadata {
                Some(metadata) => {
                    Ok(Arc::new(AgentMetadata {
                        id: metadata.id.clone(),
                        name: metadata.name.clone(),
                        description,
                        created_at: metadata.created_at.clone(),
                    }))
                }
                None => Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }
}
