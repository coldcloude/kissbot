use std::path::{Path, PathBuf};

pub const IDENTITY_MD: &str = "identity.md";
pub const USER_RECOGNITION_MD: &str = "user-recognition.md";

pub fn identity_md_path(ego_dir: impl AsRef<Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(IDENTITY_MD)
}

pub fn user_recognition_md_path(ego_dir: impl AsRef<Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(USER_RECOGNITION_MD)
}
