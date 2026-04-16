use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::error::Result;
use crate::{Config, Error, path};

pub struct DirectoryManager {
    root_dir: PathBuf,
}

static DIRECTORY_MANAGER: OnceLock<DirectoryManager> = OnceLock::new();

impl DirectoryManager {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
        }
    }

    pub fn get() -> &'static Self {
        DIRECTORY_MANAGER.get_or_init(|| {
            let config = Config::get();
            DirectoryManager::new(&config.root_dir)
        })
    }

    pub async fn list_agents(&self) -> Result<Vec<String>> {
        self.ensure_dir_exists(&self.root_dir).await?;

        let mut agents = Vec::new();
        
        let mut entries = tokio::fs::read_dir(&self.root_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let agent_path = entry.path();
            if agent_path.is_dir() {
                if let Some(agent_id) = agent_path.file_name().and_then(|n| n.to_str()) {
                    let metadata_path = path::agent_metadata_path(&agent_path);
                    if metadata_path.exists() {
                        agents.push(agent_id.to_string());
                    }
                }
            }
        }

        Ok(agents)
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

    async fn ensure_dir_exists(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        let p = path.as_ref();
        if !(p.exists() && p.is_dir()) {
            match tokio::fs::create_dir_all(p).await {
                Ok(_) => {},
                Err(_) => {},
            }
        }
        if !p.exists() {
            return Err(Error::DirectoryNotExist(p.to_string_lossy().to_string()));
        }
        Ok(p.to_path_buf())
    }
}
