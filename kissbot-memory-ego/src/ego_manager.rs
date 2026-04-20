use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use dashmap::{DashMap, DashSet, Entry};
use indicium::simple::{Indexable, SearchIndex};
use kissbot_memory::DirectoryManager;
use tokio::sync::OnceCell;

use crate::error::Result;
use crate::agent::AgentManager;

struct SearchMetadata {
    name: String,
    description: String,
}

impl Indexable for SearchMetadata {
    fn strings(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.description.clone()
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
    search_index: SearchIndex<String>,
    search_metadata: DashMap<String, SearchMetadata>,
}

static EGO_MANAGER_INSTANCE: OnceCell<EgoManager> = OnceCell::const_new();

impl EgoManager {
    pub fn new() -> Self {
        Self {
            identity_dirty: DashSet::new(),
            search_index: SearchIndex::default(),
            search_metadata: DashMap::new(),
        }
    }

    pub async fn get() -> Result<&'static Self> {
        EGO_MANAGER_INSTANCE.get_or_try_init(|| async {
            let mut instance = EgoManager::new();
            let agents = DirectoryManager::get().list_agents().await?;
            for agent_id in agents {
                instance.force_sync_identity_md(&agent_id).await?;
            }
            Ok(instance)
        }).await
    }

    pub async fn force_sync_identity_md(&mut self, agent_id: &str) -> Result<()> {
        let mut content = String::from("# Agent Identity\n\n");
        AgentManager::get().get_metadata(agent_id, |metadata| {
            //索引
            let search_metadata = SearchMetadata {
                name: metadata.name.clone(),
                description: metadata.description.clone(),
            };
            match self.search_metadata.entry(agent_id.to_string()) {
                Entry::Occupied(mut entry) => {
                    self.search_index.replace(&agent_id.to_string(), entry.get(), &search_metadata);
                    entry.insert(search_metadata);
                },
                Entry::Vacant(entry) => {
                    self.search_index.insert(&agent_id.to_string(), &search_metadata);
                    entry.insert(search_metadata);
                }
            }
            //构造MD
            content += & format!(
                "- **Name**\n {}\n- **Created At**\n {}\n- **Description**\n {}\n",
                metadata.name, metadata.created_at, metadata.description
            );
            Ok(())
        }).await?;
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;        
        let identity_path = ego_identity_md_path(&ego_dir);
        tokio::fs::write(identity_path, content).await?;
        Ok(())
    }

    pub async fn sync_identity_md(&mut self, agent_id: &str) -> Result<()> {
        match self.identity_dirty.remove(&agent_id.to_string()) {
            Some(_) => {
                self.force_sync_identity_md(agent_id).await
            },
            None => {
                Ok(())
            }
        }
    }

    pub async fn get_identity_md(&mut self, agent_id: &str) -> Result<String> {
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

    pub async fn search_by_name(&mut self, name: &str) -> Vec<String> {
        self.search_by_description(name).await
    }

    pub async fn search_by_description(&mut self, description: &str) -> Vec<String> {
        while !self.identity_dirty.is_empty() {
            let agent_ids: Vec<String> = self.identity_dirty.iter().map(|id| id.clone()).collect();
            for agent_id in agent_ids {
                match self.sync_identity_md(agent_id.as_str()).await {
                    Ok(_) => {},
                    Err(_) => {},
                }
            }
        }
        self.search_index.search(description).iter().map(|id| id.to_string()).collect()
    }
}
