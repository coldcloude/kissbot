use std::path::{Path, PathBuf};

pub const IDENTITY_MD: &str = "identity.md";
pub const USER_RECOGNITION_MD: &str = "user-recognition.md";

pub fn ego_dir(root_dir: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    kissbot_memory::path::agent_ego_dir(root_dir, agent_id)
}

pub fn identity_md_path(root_dir: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    ego_dir(root_dir, agent_id).join(IDENTITY_MD)
}

pub fn user_recognition_md_path(root_dir: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    ego_dir(root_dir, agent_id).join(USER_RECOGNITION_MD)
}
