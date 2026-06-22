use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use kissbot_api::{OtherRole, Role, RolePlay, RoleRelation};
use kissbot_memory::DirectoryManager;
use tokio::sync::RwLock;

use crate::error::Result;
use crate::error::Error;
use crate::search::SearchManager;

pub const EGO_ROLE_PLAY_PREFIX: &str = "role-play-";
pub const JSON_SUFFIX: &str = ".json";

pub fn ego_role_play_path(ego_dir: impl AsRef<std::path::Path>, role_name: &str) -> PathBuf {
    ego_dir
        .as_ref()
        .to_path_buf()
        .join(format!("{}{}{}", EGO_ROLE_PLAY_PREFIX, role_name, JSON_SUFFIX))
}

type RolePlayLock = Arc<RwLock<Option<Arc<RolePlay>>>>;

pub struct RolePlayManager {
    manager_lock: dashmap::DashMap<String, dashmap::DashMap<String, RolePlayLock>>,
}

static ROLE_PLAY_MANAGER_INSTANCE: OnceLock<RolePlayManager> = OnceLock::new();

impl RolePlayManager {
    pub fn new() -> Self {
        Self {
            manager_lock: dashmap::DashMap::new(),
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
            .or_insert_with(|| dashmap::DashMap::new())
            .entry(role_name.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(None)))
            .clone()
    }

    async fn read_role_play_ref<F>(&self, agent_id: &str, role_name: &str, mut op: F) -> Result<()>
    where
        F: FnMut(Arc<RolePlay>) -> Result<()>,
    {
        let lock = self.get_or_create_lock(agent_id, role_name).await;

        {
            let guard = lock.read().await;
            if let Some(roles) = guard.as_ref() {
                return op(roles.clone());
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(roles) = guard.as_ref() {
                return op(roles.clone());
            }

            let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
            let json_path = ego_role_play_path(&ego_dir, role_name);
            
            if !json_path.exists() {
                return Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()));
            }
            
            let content = tokio::fs::read_to_string(json_path).await?;
            let role: RolePlay = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(role));
        }

        let guard = lock.read().await;
        match guard.as_ref() {
            Some(role) => return op(role.clone()),
            None => return Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string())),
        }
    }

    async fn read_role_play_other_role_ref<F>(&self, agent_id: &str, role_name: &str, other_role_name: &str, mut op: F) -> Result<()>
    where
        F: FnMut(Arc<OtherRole>) -> Result<()>,
    {
        self.read_role_play_ref(agent_id, role_name, |role| {
            if let Some(other_role) = role.other_roles.get(other_role_name) {
                op(other_role.clone())?;
            }
            Ok(())
        }).await
    }

    async fn remove_role_play_ref(&self, agent_id: &str, role_name: &str) -> Result<()> {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let json_path = ego_role_play_path(&ego_dir, role_name);

        let lock = self.get_or_create_lock(agent_id, role_name).await;
        let mut guard = lock.write().await;

        guard.take();

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

        if guard.is_none() && json_path.exists() {
            let content = tokio::fs::read_to_string(&json_path).await?;
            let entity = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(entity));
        }

        let role = guard.take();
        match op(role.clone()) {
            Ok(new_role) => {
                *guard = Some(new_role);
            }
            Err(e) => {
                *guard = role;
                return Err(e);
            }
        }

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

    async fn write_role_play_other_role_ref<F>(&self, agent_id: &str, role_name: &str, other_role_name: &str, op: F) -> Result<()>
    where
        F: FnOnce(Arc<OtherRole>) -> Result<Arc<OtherRole>>,
    {
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            match role_or_none {
                Some(role) => {
                    if let Some(mut other_role) = role.other_roles.get_mut(other_role_name) {
                        *other_role = op(other_role.clone())?;
                    }
                    Ok(role)
                },
                None => Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await
    }

    pub async fn list_roles(&self, agent_id: &str) -> Result<Vec<String>> {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;

        let mut roles = Vec::new();
        
        let mut entries = tokio::fs::read_dir(&ego_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let role_file = entry.path();
            if role_file.is_file() {
                if let Some(role_file_name) = role_file.file_name().and_then(|n| n.to_str()) {
                    if role_file_name.starts_with(EGO_ROLE_PLAY_PREFIX) && role_file_name.ends_with(JSON_SUFFIX) {
                        if let Some(role_name) = role_file_name.get(EGO_ROLE_PLAY_PREFIX.len() .. role_file_name.len() - JSON_SUFFIX.len()) {
                            roles.push(role_name.trim().to_string());
                        }
                    }
                }
            }
        }

        Ok(roles)
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
        let search_manager = SearchManager::get().await?;
        self.write_role_play_ref(agent_id, role_name.clone().as_str(), |old| {
            match old {
                Some(_) => {
                    Err(Error::AgentRoleAlreadyExists(agent_id.to_string(), role_name.to_string()))
                }
                None => {
                    Ok(Arc::new(RolePlay {
                        role: Arc::new(Role {
                            agent_id: Arc::new(agent_id.to_string()),
                            role_name: role_name.clone(),
                            description,
                        }),
                        other_roles: Arc::new(dashmap::DashMap::new()),
                    }))
                }
            }
        }).await?;
        search_manager.mark_role_dirty(agent_id, role_name.as_str());
        Ok(())
    }

    pub async fn create_role_from(&self, agent_id: &str, role_name: &str, new_name: Arc<String>) -> Result<()> {
        let search_manager = SearchManager::get().await?;
        let role = self.get_role(agent_id, role_name).await?;
        self.create_role(agent_id, new_name.clone(), role.role.description.clone()).await?;
        search_manager.mark_role_dirty(agent_id, new_name.as_str());
        Ok(())
    }

    pub async fn remove_role(&self, agent_id: &str, role_name: &str) -> Result<()> {
        let search_manager = SearchManager::get().await?;
        self.remove_role_play_ref(agent_id, role_name).await?;
        search_manager.mark_role_dirty(agent_id, role_name);
        Ok(())
    }

    pub async fn rename_role(&self, agent_id: &str, role_name: &str, new_name: Arc<String>) -> Result<()> {
        let search_manager = SearchManager::get().await?;
        let role = self.get_role(agent_id, role_name).await?;
        self.write_role_play_ref(agent_id, new_name.as_str(), |old| {
            match old {
                Some(_) => {
                    Err(Error::AgentRoleAlreadyExists(agent_id.to_string(), new_name.to_string()))
                }
                None => {
                    Ok(Arc::new(RolePlay {
                        role: Arc::new(Role {
                            agent_id: role.role.agent_id.clone(),
                            role_name: new_name.clone(),
                            description: role.role.description.clone(),
                        }),
                        other_roles: role.other_roles.clone()
                    }))
                }
            }
        }).await?;
        search_manager.mark_role_dirty(agent_id, new_name.as_str());
        self.remove_role(agent_id, role_name).await?;
        search_manager.mark_role_dirty(agent_id, role_name);
        Ok(())
    }

    pub async fn update_role_description(&self, agent_id: &str, role_name: &str, description: Arc<String>) -> Result<()> {
        let search_manager = SearchManager::get().await?;
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            match role_or_none {
                Some(role) => {
                    Ok(Arc::new(RolePlay {
                        role: Arc::new(Role {
                            agent_id: role.role.agent_id.clone(),
                            role_name: role.role.role_name.clone(),
                            description,
                        }),
                        other_roles: role.other_roles.clone()
                    }))
                },
                None => Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await?;
        search_manager.mark_role_dirty(agent_id, role_name);
        Ok(())
    }

    pub async fn get_other_role(&self, agent_id: &str, role_name: &str, other_role_name: &str) -> Result<Arc<OtherRole>> {
        let mut result = Err(Error::AgentRoleOtherRoleNotFound(agent_id.to_string(), role_name.to_string(), other_role_name.to_string()));
        self.read_role_play_other_role_ref(agent_id, role_name, other_role_name, |other_role| {
            result = Ok(other_role.clone());
            Ok(())
        }).await?;
        result
    }

    pub async fn replace_other_roles(&self, agent_id: &str, role_name: &str, mut remove_other_roles: HashSet<String>, mut insert_other_roles: HashMap<String, Arc<OtherRole>>) -> Result<()> {
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            if let Some(role) = role_or_none {
                for other_role_name in remove_other_roles.drain() {
                    role.other_roles.remove(&other_role_name);
                }
                for (other_role_name, other_role) in insert_other_roles.drain() {
                    role.other_roles.insert(other_role_name, other_role);
                }
                Ok(role)
            }
            else {
                Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await
    }

    pub async fn rename_other_role(&self, agent_id: &str, role_name: &str, other_role_name: &str, new_name: &str) -> Result<()> {
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            if let Some(role) = role_or_none {
                if role.other_roles.contains_key(new_name) {
                    Err(Error::AgentRoleOtherRoleAlreadyExists(agent_id.to_string(), role_name.to_string(), new_name.to_string()))
                } else if let Some((_, other_role)) = role.other_roles.remove(other_role_name) {
                    role.other_roles.insert(new_name.to_string(), other_role);
                    Ok(role)
                } else {
                    Err(Error::AgentRoleOtherRoleNotFound(agent_id.to_string(), role_name.to_string(), other_role_name.to_string()))
                }
            }
            else {
                Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await
    }

    pub async fn update_other_role_individual_name(&self, agent_id: &str, role_name: &str, other_role_name: &str, new_individual_name: Arc<String>) -> Result<()> {
        self.write_role_play_other_role_ref(agent_id, role_name, other_role_name, |other_role| {
            Ok(Arc::new(OtherRole {
                individual_name: new_individual_name,
                description: other_role.description.clone(),
                role_relation: other_role.role_relation.clone(),
                other_role_relations: other_role.other_role_relations.clone(),
            }))
        }).await
    }

    pub async fn update_other_role_description(&self, agent_id: &str, role_name: &str, other_role_name: &str, new_description: Arc<String>) -> Result<()> {
        self.write_role_play_other_role_ref(agent_id, role_name, other_role_name, |other_role| {
            Ok(Arc::new(OtherRole {
                individual_name: other_role.individual_name.clone(),
                description: new_description,
                role_relation: other_role.role_relation.clone(),
                other_role_relations: other_role.other_role_relations.clone(),
            }))
        }).await
    }

    pub async fn update_other_role_relation(&self, agent_id: &str, role_name: &str, other_role_name: &str, new_relation: Arc<RoleRelation>) -> Result<()> {
        self.write_role_play_other_role_ref(agent_id, role_name, other_role_name, |other_role| {
            Ok(Arc::new(OtherRole {
                individual_name: other_role.individual_name.clone(),
                description: other_role.description.clone(),
                role_relation: new_relation,
                other_role_relations: other_role.other_role_relations.clone(),
            }))
        }).await
    }

    pub async fn replace_other_role_relations(&self, agent_id: &str, role_name: &str, other_role_name: &str, mut remove_relations: HashSet<String>, mut insert_relations: HashMap<String, Arc<RoleRelation>>) -> Result<()> {
        self.write_role_play_other_role_ref(agent_id, role_name, other_role_name, |other_role| {
            for relation_name in remove_relations.drain() {
                other_role.other_role_relations.remove(&relation_name);
            }
            for (relation_name, relation) in insert_relations.drain() {
                other_role.other_role_relations.insert(relation_name, relation);
            }
            Ok(other_role)
        }).await
    }
}
