use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::{DashMap, DashSet, Entry};
use futures::future;
use indicium::simple::{Indexable, SearchIndex};
use kissbot_memory::DirectoryManager;
use tokio::sync::{OnceCell, RwLock};

use crate::error::Result;
use crate::agent::{AgentManager, AgentMetadata};

struct SearchMetadata {
    metadata: Arc<AgentMetadata>,
}

impl Indexable for SearchMetadata {
    fn strings(&self) -> Vec<String> {
        vec![
            self.metadata.name.clone(),
            self.metadata.description.clone()
        ]
    }
}

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
    search_index: Arc<RwLock<SearchIndex<String>>>,
    search_metadata: DashMap<String, SearchMetadata>,
}

static EGO_MANAGER_INSTANCE: OnceCell<EgoManager> = OnceCell::const_new();

impl EgoManager {
    pub fn new() -> Self {
        Self {
            identity_dirty: DashSet::new(),
            search_index: Arc::new(RwLock::new(SearchIndex::default())),
            search_metadata: DashMap::new(),
        }
    }

    pub async fn get() -> Result<&'static Self> {
        EGO_MANAGER_INSTANCE.get_or_try_init(|| async {
            let instance = EgoManager::new();
            let agents = DirectoryManager::get().list_agents().await?;
            for agent_id in agents {
                instance.force_sync_identity_md(&agent_id).await?;
            }
            Ok(instance)
        }).await
    }

    pub async fn force_sync_identity_md(&self, agent_id: &str) -> Result<()> {
        let metadata = AgentManager::get().get_metadata(agent_id).await?;
        //索引
        let search_metadata = SearchMetadata {
            metadata: metadata.clone(),
        };
        {
            let mut guard = self.search_index.write().await;
            match self.search_metadata.entry(agent_id.to_string()) {
                Entry::Occupied(mut entry) => {
                    guard.replace(&agent_id.to_string(), entry.get(), &search_metadata);
                    entry.insert(search_metadata);
                },
                Entry::Vacant(entry) => {
                    guard.insert(&agent_id.to_string(), &search_metadata);
                    entry.insert(search_metadata);
                }
            }
        }
        //构造MD
        let content = format!(
            "- **Name**\n {}\n- **Created At**\n {}\n- **Description**\n {}\n",
            metadata.name, metadata.created_at, metadata.description
        );
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;        
        let identity_path = ego_identity_md_path(&ego_dir);
        tokio::fs::write(identity_path, content).await?;
        Ok(())
    }

    pub async fn sync_identity_md(&self, agent_id: &str) -> Result<()> {
        match self.identity_dirty.remove(&agent_id.to_string()) {
            Some(_) => {
                self.force_sync_identity_md(agent_id).await
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

    pub async fn search_by_name(&self, name: &str) -> Vec<Arc<AgentMetadata>> {
        self.search_by_description(name).await
    }

    pub async fn search_by_description(&self, description: &str) -> Vec<Arc<AgentMetadata>> {
        //先同步脏数据
        while !self.identity_dirty.is_empty() {
            let agent_ids: Vec<String> = self.identity_dirty.iter().map(|id| id.clone()).collect();
            let mut futs = Vec::new();
            agent_ids.iter().for_each(|id| {
                let fut = self.sync_identity_md(id.as_str());
                futs.push(fut);
            });
            future::join_all(futs).await;
        }
        //搜索
        let agent_ids: Vec<String> = {
            let guard = self.search_index.read().await;
            guard.search(description).iter().map(|id| id.to_string()).collect()
        };
        //反查结果
        let mut results = Vec::new();
        let mut futs = Vec::new();
        agent_ids.iter().for_each(|id| {
            let fut = AgentManager::get().get_metadata(id.as_str());
            futs.push(fut);
        });
        for result in future::join_all(futs).await {
            if let Ok(metadata) = result {
                results.push(metadata);
            }
        }
        results
    }
}
