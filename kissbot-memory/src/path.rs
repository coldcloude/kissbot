use std::path::{Path, PathBuf};

pub const MEMORY_EGO: &str = "memory-ego";
pub const MEMORY_STORE: &str = "memory-store";
pub const MEMORY_AGENT_DB: &str = "memory-agent.db";

pub struct PathConstants;

impl PathConstants {
    pub fn memory_ego() -> &'static str {
        MEMORY_EGO
    }

    pub fn memory_store() -> &'static str {
        MEMORY_STORE
    }

    pub fn memory_agent_db() -> &'static str {
        MEMORY_AGENT_DB
    }
}

pub struct PathBuilder {
    root_dir: PathBuf,
}

impl PathBuilder {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
        }
    }

    pub fn root_dir(&self) -> &PathBuf {
        &self.root_dir
    }

    pub fn agent_db_path(&self) -> PathBuf {
        self.root_dir.join(MEMORY_AGENT_DB)
    }

    pub fn agent_dir(&self, agent_id: &str) -> PathBuf {
        self.root_dir.join(agent_id)
    }

    pub fn agent_ego_dir(&self, agent_id: &str) -> PathBuf {
        self.agent_dir(agent_id).join(MEMORY_EGO)
    }

    pub fn agent_store_dir(&self, agent_id: &str) -> PathBuf {
        self.agent_dir(agent_id).join(MEMORY_STORE)
    }

    pub fn agent_struct_dir(&self, agent_id: &str, struct_name: &str) -> PathBuf {
        self.agent_dir(agent_id).join(struct_name)
    }
}
