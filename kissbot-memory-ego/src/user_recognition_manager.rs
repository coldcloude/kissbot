use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use dashmap::DashMap;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::Result;
use crate::error::Error;

// 用户身份枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserIdentity {
    Owner,
    Administrator,
    Other,
}

// 用户关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRelation {
    pub other_user: String,
    pub relation: String,
    pub description: Option<String>,
}

// 用户识别信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecognition {
    pub name: String,
    pub identity: UserIdentity,
    pub associated_identifiers: Vec<String>,
    pub relations: Vec<UserRelation>,
    pub description: Option<String>,
}

// 文件路径常量
pub const EGO_USER_RECOGNITION_JSON: &str = "user-recognition.json";
pub const EGO_USER_RECOGNITION_MD: &str = "user-recognition.md";

// 路径辅助函数
pub fn ego_user_recognition_json_path(ego_dir: impl AsRef<std::path::Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(EGO_USER_RECOGNITION_JSON)
}

pub fn ego_user_recognition_md_path(ego_dir: impl AsRef<std::path::Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(EGO_USER_RECOGNITION_MD)
}

// 用户识别信息管理器
type UserRecognitionLock = Arc<RwLock<Option<Vec<UserRecognition>>>>;

pub struct UserRecognitionManager {
    manager_lock: DashMap<String, UserRecognitionLock>,
}

static USER_RECOGNITION_MANAGER_INSTANCE: OnceLock<UserRecognitionManager> = OnceLock::new();

impl UserRecognitionManager {
    pub fn new() -> Self {
        Self {
            manager_lock: DashMap::new(),
        }
    }

    pub fn get() -> &'static Self {
        USER_RECOGNITION_MANAGER_INSTANCE.get_or_init(|| {
            UserRecognitionManager::new()
        })
    }

    async fn get_or_create_lock(&self, agent_id: &str) -> UserRecognitionLock {
        self.manager_lock
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(None)))
            .clone()
    }

    async fn read_user_recognition_ref<F>(&self, agent_id: &str, mut op: F) -> Result<()>
    where
        F: FnMut(&[UserRecognition]) -> Result<()>,
    {
        let lock = self.get_or_create_lock(agent_id).await;

        {
            let guard = lock.read().await;
            if let Some(users) = guard.as_ref() {
                return op(users);
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(users) = guard.as_ref() {
                return op(users);
            }

            let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
            let json_path = ego_user_recognition_json_path(&ego_dir);

            if json_path.exists() {
                let content = tokio::fs::read_to_string(json_path).await?;
                let users: Vec<UserRecognition> = serde_json::from_str(&content)?;
                *guard = Some(users);
            } else {
                *guard = Some(Vec::new());
            }
        }

        let guard = lock.read().await;
        match guard.as_ref() {
            Some(users) => op(users),
            None => Err(Error::AgentNotFound(agent_id.to_string())),
        }
    }

    async fn write_user_recognition_ref<F>(&self, agent_id: &str, op: F) -> Result<()>
    where
        F: FnOnce(Option<Vec<UserRecognition>>) -> Option<Vec<UserRecognition>>,
    {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let json_path = ego_user_recognition_json_path(&ego_dir);

        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;

        *guard = op(guard.take());

        match guard.as_ref() {
            Some(users) => {
                let content = serde_json::to_string_pretty(users)?;
                tokio::fs::write(json_path, content).await?;
                Ok(())
            }
            None => Err(Error::AgentNotFound(agent_id.to_string())),
        }
    }

    pub async fn get_users(&self, agent_id: &str) -> Result<Vec<UserRecognition>> {
        let mut result = Ok(Vec::new());
        self.read_user_recognition_ref(agent_id, |users| {
            result = Ok(users.to_vec());
            Ok(())
        }).await?;
        result
    }

    pub async fn add_user(&self, agent_id: &str, user: UserRecognition) -> Result<()> {
        self.write_user_recognition_ref(agent_id, |users| {
            let mut users = users.unwrap_or_default();
            users.push(user);
            Some(users)
        }).await
    }

    pub async fn update_user(&self, agent_id: &str, user_name: &str, user: UserRecognition) -> Result<()> {
        self.write_user_recognition_ref(agent_id, |users| {
            let mut users = users.unwrap_or_default();
            if let Some(pos) = users.iter().position(|u| u.name == user_name) {
                users[pos] = user;
            }
            Some(users)
        }).await
    }
}
