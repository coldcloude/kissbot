use chrono::Utc;
use std::sync::Arc;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::RwLock;

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

    pub async fn create_agent(&self, agent_id: Arc<String>, description: Arc<String>) -> Result<Arc<String>> {
        validate_code(agent_id.as_str())?;
        // 查重：agent 目录下 metadata.json 已存在则报错，不覆盖已有数据
        let metadata_path = agent_metadata_path(agent_id.as_str()).await?;
        if metadata_path.exists() {
            return Err(Error::AgentAlreadyExists(agent_id.to_string()));
        }
        let created_at = Arc::new(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());

        let metadata = AgentMetadata {
            agent_id: agent_id.clone(),
            description,
            created_at,
        };

        self.write_agent_metadata_ref(metadata.agent_id.clone().as_str(), |_| {
            Ok(Arc::new(metadata))
        }).await?;
        // 新 agent 需入搜索索引（name_completion/name_descr_index 依赖 agent_id；
        // 与 update_agent_description 的 mark_identity_dirty 对齐）
        SearchManager::get().await.mark_identity_dirty(agent_id.as_str());
        Ok(agent_id)
    }

    pub async fn get_agent_arc(&self, agent_id: Arc<String>) -> Result<Arc<AgentMetadata>> {
        self.read_agent_metadata(agent_id.as_str()).await
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<Arc<AgentMetadata>> {
        self.read_agent_metadata(agent_id).await
    }

    pub async fn copy_agent(&self, agent_id: &str, new_agent_id: Arc<String>) -> Result<Arc<String>> {
        let metadata = self.get_agent(agent_id).await?;
        self.create_agent(new_agent_id, metadata.description.clone()).await
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
            Arc::new("alice".to_string()),
            Arc::new("Test agent".to_string()),
        ).await.unwrap();
        assert_eq!(*agent_id, "alice");
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.agent_id, "alice");
        assert_eq!(*agent.description, "Test agent");
    }

    #[tokio::test]
    async fn test_create_agent_duplicate() {
        setup().await;
        let manager = AgentManager::get();
        manager.create_agent(
            Arc::new("dup-alice".to_string()),
            Arc::new("Test agent".to_string()),
        ).await.unwrap();
        let result = manager.create_agent(
            Arc::new("dup-alice".to_string()),
            Arc::new("Another agent".to_string()),
        ).await;
        assert!(matches!(result, Err(Error::AgentAlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_get_agent_not_found() {
        setup().await;
        let result = AgentManager::get().get_agent("nonexistent").await;
        assert!(matches!(result, Err(Error::AgentNotFound(_))));
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
    }

    #[tokio::test]
    async fn test_copy_agent() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("alice-orig".to_string()),
            Arc::new("Test".to_string()),
        ).await.unwrap();
        let new_id = manager.copy_agent(&agent_id, Arc::new("alice-copy".to_string())).await.unwrap();
        assert_eq!(*new_id, "alice-copy");
        let original = manager.get_agent(&agent_id).await.unwrap();
        let copy = manager.get_agent(&new_id).await.unwrap();
        assert_eq!(*original.description, *copy.description);
    }

    #[tokio::test]
    async fn test_crud_chain() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("alice-crud".to_string()),
            Arc::new("Original".to_string()),
        ).await.unwrap();
        manager.update_agent_description(&agent_id, Arc::new("Updated".to_string())).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
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
        assert_eq!(*agent.agent_id, "alice_01");
    }
}
