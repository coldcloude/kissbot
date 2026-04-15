use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::path;

pub struct DirectoryManager {
    root_dir: PathBuf,
}

impl DirectoryManager {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
        }
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

    pub fn dir_exists(&self, path: impl AsRef<Path>) -> bool {
        path.as_ref().exists() && path.as_ref().is_dir()
    }

    async fn ensure_dir_exists(&self, path: PathBuf) -> Result<PathBuf> {
        if !self.dir_exists(&path) {
            tokio::fs::create_dir_all(&path).await?;
        }
        Ok(path)
    }
}
