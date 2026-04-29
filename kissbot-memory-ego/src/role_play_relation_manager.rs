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

// 角色关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRelation {
    pub other_role_name: String,
    pub relation: String,
    pub description: Option<String>,
}

// 角色扮演关系信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlayRelation {
    pub id: String,
    pub name: String,
    pub associated_user_name: Option<String>,
    pub relation_with_agent_role: String,
    pub relations_with_other_roles: Vec<RoleRelation>,
    pub description: Option<String>,
}

// 文件路径常量
pub const EGO_ROLE_PLAY_RELATION_JSON_PREFIX: &str = "role-play-relation-";
pub const EGO_ROLE_PLAY_RELATION_MD_SUFFIX: &str = ".md";

// 路径辅助函数
pub fn ego_role_play_relation_json_path(ego_dir: impl AsRef<std::path::Path>, role_id: &str) -> PathBuf {
    ego_dir
        .as_ref()
        .to_path_buf()
        .join(format!("{}{}.json", EGO_ROLE_PLAY_RELATION_JSON_PREFIX, role_id))
}

pub fn ego_role_play_relation_md_path(ego_dir: impl AsRef<std::path::Path>, role_id: &str) -> PathBuf {
    ego_dir
        .as_ref()
        .to_path_buf()
        .join(format!("{}{}{}", EGO_ROLE_PLAY_RELATION_JSON_PREFIX, role_id, EGO_ROLE_PLAY_RELATION_MD_SUFFIX))
}

// 角色扮演关系管理器
type RolePlayRelationLock = Arc<RwLock<Option<Vec<RolePlayRelation>>>>;

pub struct RolePlayRelationManager {
    manager_lock: DashMap<String, RolePlayRelationLock>,
}

static ROLE_PLAY_RELATION_MANAGER_INSTANCE: OnceLock<RolePlayRelationManager> = OnceLock::new();

impl RolePlayRelationManager {
    pub fn new() -> Self {
        Self {
            manager_lock: DashMap::new(),
        }
    }

    pub fn get() -> &'static Self {
        ROLE_PLAY_RELATION_MANAGER_INSTANCE.get_or_init(|| {
            RolePlayRelationManager::new()
        })
    }

    async fn get_or_create_lock(&self, agent_id: &str) -> RolePlayRelationLock {
        self.manager_lock
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(None)))
            .clone()
    }

    async fn read_role_play_relation_ref<F>(&self, agent_id: &str, mut op: F) -> Result<()>
    where
        F: FnMut(&[RolePlayRelation]) -> Result<()>,
    {
        let lock = self.get_or_create_lock(agent_id).await;

        {
            let guard = lock.read().await;
            if let Some(relations) = guard.as_ref() {
                return op(relations);
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(relations) = guard.as_ref() {
                return op(relations);
            }

            let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
            let mut relations = Vec::new();
            
            if let Ok(mut entries) = tokio::fs::read_dir(&ego_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if let Some(filename) = entry.file_name().to_str() {
                        if filename.starts_with(EGO_ROLE_PLAY_RELATION_JSON_PREFIX) && filename.ends_with(".json") {
                            let content = tokio::fs::read_to_string(entry.path()).await?;
                            let relation: RolePlayRelation = serde_json::from_str(&content)?;
                            relations.push(relation);
                        }
                    }
                }
            }

            *guard = Some(relations);
        }

        let guard = lock.read().await;
        match guard.as_ref() {
            Some(relations) => op(relations),
            None => Err(Error::AgentNotFound(agent_id.to_string())),
        }
    }

    async fn write_role_play_relation(&self, agent_id: &str, relation: &RolePlayRelation) -> Result<()> {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let json_path = ego_role_play_relation_json_path(&ego_dir, &relation.id);
        let content = serde_json::to_string_pretty(relation)?;
        tokio::fs::write(json_path, content).await?;
        Ok(())
    }

    pub async fn get_relations(&self, agent_id: &str) -> Result<Vec<RolePlayRelation>> {
        let mut result = Ok(Vec::new());
        self.read_role_play_relation_ref(agent_id, |relations| {
            result = Ok(relations.to_vec());
            Ok(())
        }).await?;
        result
    }

    pub async fn get_relation(&self, agent_id: &str, relation_id: &str) -> Result<RolePlayRelation> {
        let mut result = Err(Error::SettingNotFound(format!("Relation {} not found", relation_id)));
        self.read_role_play_relation_ref(agent_id, |relations| {
            if let Some(relation) = relations.iter().find(|r| r.id == relation_id) {
                result = Ok(relation.clone());
            }
            Ok(())
        }).await?;
        result
    }

    pub async fn add_relation(&self, agent_id: &str, mut relation: RolePlayRelation) -> Result<String> {
        relation.id = Uuid::new_v4().to_string();
        self.write_role_play_relation(agent_id, &relation).await?;
        
        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;
        let mut relations = guard.take().unwrap_or_default();
        relations.push(relation.clone());
        *guard = Some(relations);
        
        Ok(relation.id)
    }

    pub async fn update_relation(&self, agent_id: &str, relation_id: &str, mut relation: RolePlayRelation) -> Result<()> {
        relation.id = relation_id.to_string();
        self.write_role_play_relation(agent_id, &relation).await?;
        
        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;
        let mut relations = guard.take().unwrap_or_default();
        if let Some(pos) = relations.iter().position(|r| r.id == relation_id) {
            relations[pos] = relation;
        }
        *guard = Some(relations);
        
        Ok(())
    }
}
