use chrono::Utc;
use std::sync::Arc;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::Result;
use crate::error::Error;
use crate::code::validate_code;
use crate::search::SearchManager;
use kissbot_api::AgentMetadata;
use kissbot_memory::DirectoryManager;

pub const AGENT_METADATA_JSON: &str = "metadata.json";

async fn agent_metadata_path(agent_id: &str) -> Result<PathBuf> {
    let agent_dir = DirectoryManager::get().ensure_agent_dir(agent_id).await?;
    let metadata_path = agent_dir.join(AGENT_METADATA_JSON);
    Ok(metadata_path)
}

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

    async fn read_agent_metadata(&self, agent_id: &str) -> Result<Arc<AgentMetadata>> {
        let lock = self.get_or_create_lock(agent_id).await;

        {
            let guard = lock.read().await;
            if let Some(metadata) = guard.as_ref() {
                return Ok(metadata.clone());
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(metadata) = guard.as_ref() {
                return Ok(metadata.clone());
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
            Some(metadata) => return Ok(metadata.clone()),
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

    pub async fn create_agent(&self, individual_name: Arc<String>, description: Arc<String>) -> Result<Arc<String>> {
        validate_code(individual_name.as_str())?;
        let agent_id = Arc::new(Uuid::new_v4().to_string());
        let created_at = Arc::new(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());

        let metadata = AgentMetadata {
            agent_id: agent_id.clone(),
            individual_name,
            description,
            created_at,
        };

        self.write_agent_metadata_ref(metadata.agent_id.clone().as_str(), |_| {
            Ok(Arc::new(metadata))
        }).await?;
        Ok(agent_id)
    }

    pub async fn get_agent_arc(&self, agent_id: Arc<String>) -> Result<Arc<AgentMetadata>> {
        self.read_agent_metadata(agent_id.as_str()).await
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<Arc<AgentMetadata>> {
        self.read_agent_metadata(agent_id).await
    }

    pub async fn copy_agent(&self, agent_id: &str) -> Result<Arc<String>> {
        let metadata = self.get_agent(agent_id).await?;
        self.create_agent(metadata.individual_name.clone(), metadata.description.clone()).await
    }

    pub async fn update_agent_name(&self, agent_id: &str, individual_name: Arc<String>) -> Result<()> {
        validate_code(individual_name.as_str())?;
        self.write_agent_metadata_ref(agent_id, |metadata| {
            match metadata {
                Some(metadata) => {
                    if individual_name.as_str() != metadata.individual_name.as_str() {
                        let mut metadata_new_arc = metadata.clone();
                        let metadata_new = Arc::make_mut(&mut metadata_new_arc);
                        metadata_new.individual_name = individual_name.clone();
                        Ok(metadata_new_arc)
                    } else {
                        Ok(metadata)
                    }
                }
                None => Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await?;
        SearchManager::get().await.mark_identity_dirty(agent_id);
        Ok(())
    }

    pub async fn update_agent_description(&self, agent_id: &str, description: Arc<String>) -> Result<()> {
        self.write_agent_metadata_ref(agent_id, |metadata| {
            match metadata {
                Some(metadata) => {
                    if description.as_str() != metadata.description.as_str() {
                        let mut metadata_new_arc = metadata.clone();
                        let metadata_new = Arc::make_mut(&mut metadata_new_arc);
                        metadata_new.description = description;
                        Ok(metadata_new_arc)
                    } else {
                        Ok(metadata)
                    }
                }
                None => Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await?;
        SearchManager::get().await.mark_identity_dirty(agent_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 初始化测试环境，创建基础 agent 目录结构供 SearchManager 使用。
    async fn setup() {
        crate::test_util::init_test_config();
        // 基础 setup 只执行一次
        static SETUP: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        SETUP.get_or_init(|| async {
            let dm = kissbot_memory::DirectoryManager::get();
            dm.ensure_agent_dir("setup-agent").await.unwrap();
            let agent_dir = dm.ensure_agent_dir("setup-agent").await.unwrap();
            let metadata = serde_json::json!({
                "agent_id": "setup-agent",
                "individual_name": "Setup",
                "description": "Setup agent for SearchManager init",
                "created_at": "2026-06-25 10:00:00"
            });
            tokio::fs::write(
                agent_dir.join("metadata.json"),
                serde_json::to_string_pretty(&metadata).unwrap(),
            ).await.unwrap();
            dm.ensure_agent_ego_dir("setup-agent").await.unwrap();
        }).await;
    }

    #[tokio::test]
    async fn test_create_agent() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Test agent".to_string()),
        ).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.individual_name, "Alice");
        assert_eq!(*agent.description, "Test agent");
        assert_eq!(*agent.agent_id, *agent_id);
    }

    #[tokio::test]
    async fn test_get_agent_not_found() {
        setup().await;
        let result = AgentManager::get().get_agent("nonexistent").await;
        assert!(matches!(result, Err(Error::AgentNotFound(_))));
    }

    #[tokio::test]
    async fn test_update_agent_name() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Original description".to_string()),
        ).await.unwrap();
        manager.update_agent_name(&agent_id, Arc::new("Alice2".to_string())).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.individual_name, "Alice2");
        assert_eq!(*agent.description, "Original description");
    }

    #[tokio::test]
    async fn test_update_agent_description() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Original description".to_string()),
        ).await.unwrap();
        manager.update_agent_description(&agent_id, Arc::new("New description".to_string())).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.description, "New description");
        assert_eq!(*agent.individual_name, "Alice");
    }

    #[tokio::test]
    async fn test_copy_agent() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Test".to_string()),
        ).await.unwrap();
        let new_id = manager.copy_agent(&agent_id).await.unwrap();
        assert_ne!(*agent_id, *new_id);
        let original = manager.get_agent(&agent_id).await.unwrap();
        let copy = manager.get_agent(&new_id).await.unwrap();
        assert_eq!(*original.individual_name, *copy.individual_name);
    }

    #[tokio::test]
    async fn test_crud_chain() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Original".to_string()),
        ).await.unwrap();
        manager.update_agent_name(&agent_id, Arc::new("Alice2".to_string())).await.unwrap();
        manager.update_agent_description(&agent_id, Arc::new("Updated".to_string())).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.individual_name, "Alice2");
        assert_eq!(*agent.description, "Updated");
    }

    #[tokio::test]
    async fn test_create_agent_rejects_invalid_code() {
        setup().await;
        let result = AgentManager::get().create_agent(
            Arc::new("a b c".to_string()),
            Arc::new("Test agent".to_string()),
        ).await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
    }

    #[tokio::test]
    async fn test_create_agent_valid_code() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("alice_01".to_string()),
            Arc::new("Test agent".to_string()),
        ).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.individual_name, "alice_01");
    }
}
