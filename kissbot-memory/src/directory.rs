use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::path::PathBuilder;

pub struct DirectoryManager {
    path_builder: PathBuilder,
}

impl DirectoryManager {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            path_builder: PathBuilder::new(root_dir),
        }
    }

    pub fn path_builder(&self) -> &PathBuilder {
        &self.path_builder
    }

    pub async fn ensure_root_dir(&self) -> Result<()> {
        self.ensure_dir_exists(self.path_builder.root_dir()).await
    }

    pub async fn ensure_agent_dir(&self, agent_id: &str) -> Result<()> {
        self.ensure_dir_exists(&self.path_builder.agent_dir(agent_id))
            .await
    }

    pub async fn ensure_agent_ego_dir(&self, agent_id: &str) -> Result<()> {
        self.ensure_dir_exists(&self.path_builder.agent_ego_dir(agent_id))
            .await
    }

    pub async fn ensure_agent_store_dir(&self, agent_id: &str) -> Result<()> {
        self.ensure_dir_exists(&self.path_builder.agent_store_dir(agent_id))
            .await
    }

    pub async fn ensure_agent_struct_dir(&self, agent_id: &str, struct_name: &str) -> Result<()> {
        self.ensure_dir_exists(&self.path_builder.agent_struct_dir(agent_id, struct_name))
            .await
    }

    pub fn dir_exists(&self, path: impl AsRef<Path>) -> bool {
        path.as_ref().exists() && path.as_ref().is_dir()
    }

    async fn ensure_dir_exists(&self, path: &PathBuf) -> Result<()> {
        if !self.dir_exists(path) {
            tokio::fs::create_dir_all(path).await?;
        }
        Ok(())
    }
}
