use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::error::Result;
use crate::{Config, Error};

const AGENT_UUID_PRREFIX: &str = "agent-";

const MEMORY_EGO: &str = "memory-ego";
const MEMORY_STORE: &str = "memory-store";

fn agent_dir(root_dir: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    root_dir.as_ref().join(agent_id)
}

fn agent_uuid_file(agent_dir: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    agent_dir.as_ref().to_path_buf().join(format!("{}{}", AGENT_UUID_PRREFIX, agent_id))
}

fn agent_ego_dir(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().to_path_buf().join(MEMORY_EGO)
}

fn agent_store_dir(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().to_path_buf().join(MEMORY_STORE)
}

fn agent_struct_dir(agent_dir: impl AsRef<Path>, struct_name: &str) -> PathBuf {
    agent_dir.as_ref().to_path_buf().join(struct_name)
}

async fn ensure_dir_exists(path: impl AsRef<Path>) -> Result<PathBuf> {
    let p = path.as_ref();
    if !(p.exists() && p.is_dir()) {
        let _ = tokio::fs::create_dir_all(p).await;
    }
    if !p.exists() {
        return Err(Error::PathNotExist(p.to_string_lossy().to_string()));
    }
    Ok(p.to_path_buf())
}

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
        ensure_dir_exists(&self.root_dir).await?;

        let mut agents = Vec::new();
        
        let mut entries = tokio::fs::read_dir(&self.root_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let agent_dir = entry.path();
            if agent_dir.is_dir() {
                if let Some(agent_id) = agent_dir.file_name().and_then(|n| n.to_str()) {
                    let uuid_file = agent_uuid_file(&agent_dir, agent_id);
                    if uuid_file.exists() {
                        agents.push(agent_id.to_string());
                    }
                }
            }
        }

        Ok(agents)
    }

    pub async fn ensure_agent_dir(&self, agent_id: &str) -> Result<PathBuf> {
        let path = agent_dir(&self.root_dir, agent_id);
        ensure_dir_exists(&path).await?;
        let uuid_file = agent_uuid_file(&path, agent_id);
        if !uuid_file.exists() {
            let _ = tokio::fs::File::create(&uuid_file).await;
        }
        if !uuid_file.exists() {
            return Err(Error::PathNotExist(uuid_file.to_string_lossy().to_string()));
        }
        Ok(path)
    }

    pub async fn ensure_agent_ego_dir(&self, agent_id: &str) -> Result<PathBuf> {
        let agent_path = agent_dir(&self.root_dir, agent_id);
        let path = agent_ego_dir(&agent_path);
        ensure_dir_exists(path).await
    }

    pub async fn ensure_agent_store_dir(&self, agent_id: &str) -> Result<PathBuf> {
        let agent_path = agent_dir(&self.root_dir, agent_id);
        let path = agent_store_dir(&agent_path);
        ensure_dir_exists(path).await
    }

    pub async fn ensure_agent_struct_dir(&self, agent_id: &str, struct_name: &str) -> Result<PathBuf> {
        let agent_path = agent_dir(&self.root_dir, agent_id);
        let path = agent_struct_dir(&agent_path, struct_name);
        ensure_dir_exists(path).await
    }
}
