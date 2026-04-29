use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use dashmap::DashMap;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::Result;
use crate::error::Error;

// 角色关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtherRoleRelation {
    pub relation: Arc<String>,
    pub description: Arc<String>,
}

// 角色扮演关系信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlayRelation {
    pub user_name: Arc<String>,
    pub role_relation: Arc<OtherRoleRelation>,
    pub other_role_relations: Arc<DashMap<String, Arc<OtherRoleRelation>>>,
    pub description: Arc<String>,
}

// 角色扮演信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlay {
    pub id: Arc<String>,
    pub name: Arc<String>,
    pub description: Arc<String>,
    pub relations: Arc<DashMap<String, Arc<RolePlayRelation>>>,
}

// 文件路径常量
pub const EGO_ROLE_PLAY_PREFIX: &str = "role-play-";

// 路径辅助函数
pub fn ego_role_play_path(ego_dir: impl AsRef<std::path::Path>, role_name: &str) -> PathBuf {
    ego_dir
        .as_ref()
        .to_path_buf()
        .join(format!("{}{}.json", EGO_ROLE_PLAY_PREFIX, role_name))
}

// 角色扮演管理器
type RolePlayLock = Arc<RwLock<Option<Arc<RolePlay>>>>;

pub struct RolePlayManager {
    manager_lock: DashMap<String, DashMap<String, RolePlayLock>>,
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

    async fn get_or_create_lock(&self, agent_id: &str, role_name: &str) -> RolePlayLock {
        self.manager_lock
        .entry(agent_id.to_string())
        .or_insert_with(|| DashMap::new())
        .entry(role_name.to_string())
        .or_insert_with(|| Arc::new(RwLock::new(None)))
        .clone()
    }

    async fn read_role_play_ref<F>(&self, agent_id: &str, role_name: &str, mut op: F) -> Result<()>
    where
        F: FnMut(Arc<RolePlay>) -> Result<()>,
    {
        let lock = self.get_or_create_lock(agent_id, role_name).await;

        //先尝试读内存
        {
            let guard = lock.read().await;
            if let Some(roles) = guard.as_ref() {
                return op(roles.clone());
            }
        }

        //无数据，从文件读取
        {
            let mut guard = lock.write().await;

            //双重锁定
            if let Some(roles) = guard.as_ref() {
                return op(roles.clone());
            }

            //从文件读取
            let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
            let json_path = ego_role_play_path(&ego_dir, role_name);
            
            if !json_path.exists() {
                return Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()));
            }
            
            let content = tokio::fs::read_to_string(json_path).await?;
            let role: RolePlay = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(role));
        }

        //读文件写入后重新读取
        let guard = lock.read().await;
        match guard.as_ref() {
            Some(role) => return op(role.clone()),
            None => return Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string())),
        }
    }

    async fn remove_role_play_ref(&self, agent_id: &str, role_name: &str) -> Result<()> {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let json_path = ego_role_play_path(&ego_dir, role_name);

        let lock = self.get_or_create_lock(agent_id, role_name).await;
        let mut guard = lock.write().await;

        //删除内存
        guard.take();

        //删除文件
        if json_path.exists() {
            tokio::fs::remove_file(json_path).await?;
        }

        Ok(())
    }

    async fn write_role_play_ref<F>(&self, agent_id: &str, role_name: &str, op: F) -> Result<()>
    where
        F: FnOnce(Option<Arc<RolePlay>>) -> Result<Arc<RolePlay>>,
    {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let json_path = ego_role_play_path(&ego_dir, role_name);

        let lock = self.get_or_create_lock(agent_id, role_name).await;
        let mut guard = lock.write().await;

        //更新
        let new_role = op(guard.take())?;
        *guard = Some(new_role);

        //写入文件
        match guard.as_ref() {
            Some(role) => {
                let content = serde_json::to_string_pretty(role)?;
                tokio::fs::write(json_path, content).await?;
                Ok(())
            }
            None => {
                Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }
    }

    async fn write_role_play_relation_ref<F>(&self, agent_id: &str, role_name: &str, other_role_name: &str, op: F) -> Result<()>
    where
        F: FnOnce(Arc<RolePlayRelation>) -> Result<Arc<RolePlayRelation>>,
    {
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            match role_or_none {
                Some(role) => {
                    if let Some(mut relation) = role.relations.get_mut(other_role_name) {
                        *relation = op(relation.clone())?;
                    }
                    Ok(role)
                },
                None => Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await
    }

    pub async fn get_role(&self, agent_id: &str, role_name: &str) -> Result<Arc<RolePlay>> {
        let mut result = Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()));
        self.read_role_play_ref(agent_id, role_name, |role| {
            result = Ok(role.clone());
            Ok(())
        }).await?;
        result
    }

    pub async fn create_role(&self, agent_id: &str, role_name: Arc<String>, description: Arc<String>) -> Result<()> {
        self.write_role_play_ref(agent_id, role_name.clone().as_str(), |_| {
            Ok(Arc::new(RolePlay {
                id: Arc::new(agent_id.to_string()),
                name: role_name,
                relations: Arc::new(DashMap::new()),
                description: description,
            }))
        }).await
    }

    pub async fn remove_role(&self, agent_id: &str, role_name: &str) -> Result<()> {
        self.remove_role_play_ref(agent_id, role_name).await
    }

    pub async fn rename_role(&self, agent_id: &str, role_name: &str, new_name: Arc<String>) -> Result<()> {
        let role = self.get_role(agent_id, role_name).await?;
        self.remove_role(agent_id, role_name).await?;
        self.write_role_play_ref(agent_id, new_name.as_str(), |_| {
            Ok(Arc::new(RolePlay {
                id: role.id.clone(),
                name: new_name.clone(),
                description: role.description.clone(),
                relations: role.relations.clone()
            }))
        }).await
    }

    pub async fn copy_role(&self, agent_id: &str, role_name: &str, new_name: Arc<String>) -> Result<()> {
        let role = self.get_role(agent_id, role_name).await?;
        self.create_role(agent_id, new_name, role.description.clone()).await
    }

    pub async fn update_role_description(&self, agent_id: &str, role_name: &str, description: Arc<String>) -> Result<()> {
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            match role_or_none {
                Some(role) => {
                    Ok(Arc::new(RolePlay {
                        id: role.id.clone(),
                        name: role.name.clone(),
                        description: description,
                        relations: role.relations.clone()
                    }))
                },
                None => Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await
    }

    pub async fn update_role_relations(&self, agent_id: &str, role_name: &str, mut remove_relations: HashSet<String>, mut insert_relations: HashMap<String,Arc<RolePlayRelation>>) -> Result<()> {
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            match role_or_none {
                Some(role) => {
                    for identifier in remove_relations.drain() {
                        role.relations.remove(&identifier);
                    }
                    for (identifier, relation) in insert_relations.drain() {
                        role.relations.insert(identifier, relation);
                    }
                    Ok(role)
                },
                None => Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await
    }
}
