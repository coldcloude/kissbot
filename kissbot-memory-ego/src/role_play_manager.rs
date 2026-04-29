use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use dashmap::DashMap;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::Result;
use crate::error::Error;

// 角色扮演信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlay {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

// 文件路径常量
pub const EGO_ROLE_PLAY_JSON_PREFIX: &str = "role-play-";
pub const EGO_ROLE_PLAY_MD_SUFFIX: &str = ".md";

// 路径辅助函数
pub fn ego_role_play_json_path(ego_dir: impl AsRef<std::path::Path>, role_id: &str) -> PathBuf {
    ego_dir
        .as_ref()
        .to_path_buf()
        .join(format!("{}{}.json", EGO_ROLE_PLAY_JSON_PREFIX, role_id))
}

pub fn ego_role_play_md_path(ego_dir: impl AsRef<std::path::Path>, role_id: &str) -> PathBuf {
    ego_dir
        .as_ref()
        .to_path_buf()
        .join(format!("{}{}{}", EGO_ROLE_PLAY_JSON_PREFIX, role_id, EGO_ROLE_PLAY_MD_SUFFIX))
}

// 角色扮演管理器
type RolePlayLock = Arc<RwLock<Option<Vec<RolePlay>>>>;

pub struct RolePlayManager {
    manager_lock: DashMap<String, RolePlayLock>,
}

static ROLE_PLAY_MANAGER_INSTANCE: OnceLock<RolePlayManager> = OnceLock::new();

impl RolePlayManager {
    pub fn new() -> Self {
        Self {
            manager_lock: DashMap::new(),
        }
    }

    pub fn get() -> &'static Self {
        ROLE_PLAY_MANAGER_INSTANCE.get_or_init(|| {
            RolePlayManager::new()
        })
    }

    async fn get_or_create_lock(&self, agent_id: &str) -> RolePlayLock {
        self.manager_lock
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(None)))
            .clone()
    }

    async fn read_role_play_ref<F>(&self, agent_id: &str, mut op: F) -> Result<()>
    where
        F: FnMut(&[RolePlay]) -> Result<()>,
    {
        let lock = self.get_or_create_lock(agent_id).await;

        {
            let guard = lock.read().await;
            if let Some(roles) = guard.as_ref() {
                return op(roles);
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(roles) = guard.as_ref() {
                return op(roles);
            }

            let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
            let mut roles = Vec::new();
            
            if let Ok(mut entries) = tokio::fs::read_dir(&ego_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if let Some(filename) = entry.file_name().to_str() {
                        if filename.starts_with(EGO_ROLE_PLAY_JSON_PREFIX) && filename.ends_with(".json") {
                            let content = tokio::fs::read_to_string(entry.path()).await?;
                            let role: RolePlay = serde_json::from_str(&content)?;
                            roles.push(role);
                        }
                    }
                }
            }

            *guard = Some(roles);
        }

        let guard = lock.read().await;
        match guard.as_ref() {
            Some(roles) => op(roles),
            None => Err(Error::AgentNotFound(agent_id.to_string())),
        }
    }

    async fn write_role_play(&self, agent_id: &str, role: &RolePlay) -> Result<()> {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let json_path = ego_role_play_json_path(&ego_dir, &role.id);
        let content = serde_json::to_string_pretty(role)?;
        tokio::fs::write(json_path, content).await?;
        Ok(())
    }

    pub async fn get_roles(&self, agent_id: &str) -> Result<Vec<RolePlay>> {
        let mut result = Ok(Vec::new());
        self.read_role_play_ref(agent_id, |roles| {
            result = Ok(roles.to_vec());
            Ok(())
        }).await?;
        result
    }

    pub async fn get_role(&self, agent_id: &str, role_id: &str) -> Result<RolePlay> {
        let mut result = Err(Error::SettingNotFound(format!("Role {} not found", role_id)));
        self.read_role_play_ref(agent_id, |roles| {
            if let Some(role) = roles.iter().find(|r| r.id == role_id) {
                result = Ok(role.clone());
            }
            Ok(())
        }).await?;
        result
    }

    pub async fn add_role(&self, agent_id: &str, mut role: RolePlay) -> Result<String> {
        role.id = Uuid::new_v4().to_string();
        self.write_role_play(agent_id, &role).await?;
        
        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;
        let mut roles = guard.take().unwrap_or_default();
        roles.push(role.clone());
        *guard = Some(roles);
        
        Ok(role.id)
    }

    pub async fn update_role(&self, agent_id: &str, role_id: &str, mut role: RolePlay) -> Result<()> {
        role.id = role_id.to_string();
        self.write_role_play(agent_id, &role).await?;
        
        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;
        let mut roles = guard.take().unwrap_or_default();
        if let Some(pos) = roles.iter().position(|r| r.id == role_id) {
            roles[pos] = role;
        }
        *guard = Some(roles);
        
        Ok(())
    }
}
