use std::path::{Path, PathBuf};

pub const MEMORY_EGO: &str = "memory-ego";
pub const MEMORY_STORE: &str = "memory-store";
pub const MEMORY_AGENT_DB: &str = "memory-agent.db";

pub fn agent_db_path(root_dir: impl AsRef<Path>) -> PathBuf {
    root_dir.as_ref().join(MEMORY_AGENT_DB)
}

pub fn agent_dir(root_dir: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    root_dir.as_ref().join(agent_id)
}

pub fn agent_ego_dir(root_dir: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    agent_dir(root_dir, agent_id).join(MEMORY_EGO)
}

pub fn agent_store_dir(root_dir: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    agent_dir(root_dir, agent_id).join(MEMORY_STORE)
}

pub fn agent_struct_dir(root_dir: impl AsRef<Path>, agent_id: &str, struct_name: &str) -> PathBuf {
    agent_dir(root_dir, agent_id).join(struct_name)
}
