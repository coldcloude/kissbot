use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use arc_swap::ArcSwap;
use kissbot_api::ArcSwapHashMap;
use kissbot_api::OtherRoleEntry;
use kissbot_api::RoleKey;
use kissbot_api::RoleRelationEntry;
use kissbot_api::{OtherRole, Role, RolePlay, RoleRelation};
use kissbot_memory::DirectoryManager;
use tokio::sync::RwLock;

use crate::error::Result;
use crate::error::Error;
use crate::code::validate_code;
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

    async fn read_role_play(&self, agent_id: &str, role_name: &str) -> Result<Arc<RolePlay>> {
        let lock = self.get_or_create_lock(agent_id, role_name).await;

        {
            let guard = lock.read().await;
            if let Some(roles) = guard.as_ref() {
                return Ok(roles.clone());
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(roles) = guard.as_ref() {
                return Ok(roles.clone());
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
            Some(role) => return Ok(role.clone()),
            None => return Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string())),
        }
    }

    async fn read_role_play_other_role(&self, agent_id: &str, role_name: &str, other_role_name: &str) -> Result<Arc<OtherRole>> {
        let role = self.read_role_play(agent_id, role_name).await?;
        let entry = role.other_roles.get(other_role_name).ok_or_else(|| Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))?;
        Ok(entry.load().clone())
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
            if let Some(role) = role_or_none {
                if let Some(entry) = role.other_roles.get(other_role_name) {
                    let current = entry.load_full();
                    let updated = op(current)?;
                    entry.store(updated);
                    Ok(role)
                } else {
                    Err(Error::AgentRoleOtherRoleNotFound(agent_id.to_string(), role_name.to_string(), other_role_name.to_string()))
                }
            } else {
                Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
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

    pub async fn get_role_arc(&self, role_key: Arc<RoleKey>) -> Result<Arc<RolePlay>> {
        self.get_role(role_key.agent_id.as_str(), role_key.role_name.as_str()).await
    }

    pub async fn get_role(&self, agent_id: &str, role_name: &str) -> Result<Arc<RolePlay>> {
        self.read_role_play(agent_id, role_name).await
    }

    pub async fn create_role(&self, agent_id: &str, role_name: Arc<String>, description: Arc<String>) -> Result<()> {
        validate_code(role_name.as_str())?;
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
                            full_name: Arc::new(String::new()),
                            description,
                        }),
                        other_roles: Arc::new(ArcSwapHashMap::new()),
                    }))
                }
            }
        }).await?;
        SearchManager::get().await.mark_role_dirty(agent_id, role_name.as_str());
        Ok(())
    }

    pub async fn create_role_from(&self, agent_id: &str, role_name: &str, new_name: Arc<String>) -> Result<()> {
        validate_code(new_name.as_str())?;
        let role = self.get_role(agent_id, role_name).await?;
        self.create_role(agent_id, new_name.clone(), role.role.description.clone()).await?;
        SearchManager::get().await.mark_role_dirty(agent_id, new_name.as_str());
        Ok(())
    }

    pub async fn update_role_description(&self, agent_id: &str, role_name: &str, description: Arc<String>) -> Result<()> {
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            match role_or_none {
                Some(role) => {
                    Ok(Arc::new(RolePlay {
                        role: Arc::new(Role {
                            agent_id: role.role.agent_id.clone(),
                            role_name: role.role.role_name.clone(),
                            full_name: role.role.full_name.clone(),
                            description,
                        }),
                        other_roles: role.other_roles.clone()
                    }))
                },
                None => Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await?;
        SearchManager::get().await.mark_role_dirty(agent_id, role_name);
        Ok(())
    }

    pub async fn update_role_full_name(&self, agent_id: &str, role_name: &str, full_name: Arc<String>) -> Result<()> {
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            match role_or_none {
                Some(role) => {
                    Ok(Arc::new(RolePlay {
                        role: Arc::new(Role {
                            agent_id: role.role.agent_id.clone(),
                            role_name: role.role.role_name.clone(),
                            full_name,
                            description: role.role.description.clone(),
                        }),
                        other_roles: role.other_roles.clone()
                    }))
                },
                None => Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await?;
        SearchManager::get().await.mark_role_dirty(agent_id, role_name);
        Ok(())
    }

    pub async fn get_other_role(&self, agent_id: &str, role_name: &str, other_role_name: &str) -> Result<Arc<OtherRole>> {
        self.read_role_play_other_role(agent_id, role_name, other_role_name).await
    }

    pub async fn replace_other_roles(&self, agent_id: &str, role_name: &str, mut remove_other_roles: Vec<String>, mut insert_other_roles: Vec<OtherRoleEntry>) -> Result<()> {
        for entry in insert_other_roles.iter() {
            validate_code(&entry.role_name)?;
        }
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            if let Some(role) = role_or_none {
                let mut role_new_arc = role.clone();
                let role_new = Arc::make_mut(&mut role_new_arc);
                let other_roles = Arc::make_mut(&mut role_new.other_roles);
                for name in remove_other_roles.drain(..) {
                    other_roles.remove(name.as_str());
                }
                for entry in insert_other_roles.drain(..) {
                    other_roles.insert(entry.role_name, ArcSwap::new(entry.other_role));
                }
                Ok(role_new_arc)
            }
            else {
                Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await
    }

    pub async fn rename_other_role(&self, agent_id: &str, role_name: &str, other_role_name: &str, new_name: &str) -> Result<()> {
        // 校验新 key（role_name 代号）
        validate_code(new_name)?;
        self.write_role_play_ref(agent_id, role_name, |role_or_none| {
            if let Some(role) = role_or_none {
                if role.other_roles.contains_key(new_name) {
                    Err(Error::AgentRoleOtherRoleAlreadyExists(agent_id.to_string(), role_name.to_string(), new_name.to_string()))
                } else {
                    if let Some(other_role) = role.other_roles.get(other_role_name) {
                        let mut role_new_arc = role.clone();
                        let role_new = Arc::make_mut(&mut role_new_arc);
                        let other_roles = Arc::make_mut(&mut role_new.other_roles);
                        other_roles.remove(other_role_name);
                        other_roles.insert(new_name.to_string(), ArcSwap::new(other_role.load_full()));
                        Ok(role_new_arc)
                    } else {
                        Err(Error::AgentRoleOtherRoleNotFound(agent_id.to_string(), role_name.to_string(), other_role_name.to_string()))
                    }
                }
            }
            else {
                Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string()))
            }
        }).await
    }

    pub async fn update_other_role_individual_name(&self, agent_id: &str, role_name: &str, other_role_name: &str, new_individual_name: Arc<String>) -> Result<()> {
        validate_code(new_individual_name.as_str())?;
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

    pub async fn replace_other_role_relations(&self, agent_id: &str, role_name: &str, other_role_name: &str, mut remove_relations: Vec<String>, mut insert_relations: Vec<RoleRelationEntry>) -> Result<()> {
        for entry in insert_relations.iter() {
            validate_code(&entry.role_name)?;
        }
        self.write_role_play_other_role_ref(agent_id, role_name, other_role_name, |mut other_role_arc| {
            let other_role = Arc::make_mut(&mut other_role_arc);
            let other_role_relations = Arc::make_mut(&mut other_role.other_role_relations);
            for name in remove_relations.drain(..) {
                other_role_relations.remove(&name);
            }
            for entry in insert_relations.drain(..) {
                other_role_relations.insert(entry.role_name, ArcSwap::new(entry.relation));
            }
            Ok(other_role_arc)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() {
        crate::test_util::init_test_config();
        static SETUP: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        SETUP.get_or_init(|| async {
            let dm = kissbot_memory::DirectoryManager::get();
            dm.ensure_agent_dir("setup-agent").await.unwrap();
            dm.ensure_agent_ego_dir("setup-agent").await.unwrap();
        }).await;
    }

    #[test]
    fn test_ego_role_play_path() {
        let path = ego_role_play_path("/tmp/ego", "admin");
        assert_eq!(path, std::path::Path::new("/tmp/ego").join("role-play-admin.json"));
    }

    #[tokio::test]
    async fn test_create_role() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-role1").await.unwrap();
        dm.ensure_agent_ego_dir("agent-role1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-role1", Arc::new("admin".to_string()), Arc::new("Administrator".to_string())).await.unwrap();
        let role = manager.get_role("agent-role1", "admin").await.unwrap();
        assert_eq!(*role.role.role_name, "admin");
        assert_eq!(*role.role.description, "Administrator");
    }

    #[tokio::test]
    async fn test_create_role_duplicate() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-dup").await.unwrap();
        dm.ensure_agent_ego_dir("agent-dup").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-dup", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let result = manager.create_role("agent-dup", Arc::new("admin".to_string()), Arc::new("Other".to_string())).await;
        assert!(matches!(result, Err(Error::AgentRoleAlreadyExists(_, _))));
    }

    #[tokio::test]
    async fn test_create_role_from() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-from").await.unwrap();
        dm.ensure_agent_ego_dir("agent-from").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-from", Arc::new("admin".to_string()), Arc::new("Original desc".to_string())).await.unwrap();
        manager.create_role_from("agent-from", "admin", Arc::new("mod".to_string())).await.unwrap();
        let new_role = manager.get_role("agent-from", "mod").await.unwrap();
        assert_eq!(*new_role.role.description, "Original desc");
    }

    #[tokio::test]
    async fn test_list_roles() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-list").await.unwrap();
        dm.ensure_agent_ego_dir("agent-list").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-list", Arc::new("admin".to_string()), Arc::new("".to_string())).await.unwrap();
        manager.create_role("agent-list", Arc::new("mod".to_string()), Arc::new("".to_string())).await.unwrap();
        let roles = manager.list_roles("agent-list").await.unwrap();
        assert_eq!(roles.len(), 2);
        assert!(roles.contains(&"admin".to_string()));
        assert!(roles.contains(&"mod".to_string()));
    }

    #[tokio::test]
    async fn test_get_role_not_found() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-nf").await.unwrap();
        dm.ensure_agent_ego_dir("agent-nf").await.unwrap();
        let result = RolePlayManager::get().get_role("agent-nf", "nonexistent").await;
        assert!(matches!(result, Err(Error::AgentRoleNotFound(_, _))));
    }

    #[tokio::test]
    async fn test_update_role_description() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-upd-desc").await.unwrap();
        dm.ensure_agent_ego_dir("agent-upd-desc").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-upd-desc", Arc::new("admin".to_string()), Arc::new("Old".to_string())).await.unwrap();
        manager.update_role_description("agent-upd-desc", "admin", Arc::new("New desc".to_string())).await.unwrap();
        let role = manager.get_role("agent-upd-desc", "admin").await.unwrap();
        assert_eq!(*role.role.description, "New desc");
    }

    #[tokio::test]
    async fn test_update_role_full_name() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-upd-full-name").await.unwrap();
        dm.ensure_agent_ego_dir("agent-upd-full-name").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-upd-full-name", Arc::new("admin".to_string()), Arc::new("Old desc".to_string())).await.unwrap();
        manager.update_role_full_name("agent-upd-full-name", "admin", Arc::new("新展示名".to_string())).await.unwrap();
        let role = manager.get_role("agent-upd-full-name", "admin").await.unwrap();
        assert_eq!(*role.role.full_name, "新展示名");
        assert_eq!(*role.role.description, "Old desc");
    }

    #[tokio::test]
    async fn test_other_role_replace() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-other1").await.unwrap();
        dm.ensure_agent_ego_dir("agent-other1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-other1", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                full_name: Arc::new(String::new()),
                description: Arc::new("Works together".to_string()),
            }),
            other_role_relations: Arc::new(ArcSwapHashMap::new()),
            description: Arc::new("A colleague".to_string()),
        });
        manager.replace_other_roles(
            "agent-other1", "admin",
            vec![],
            vec![OtherRoleEntry { role_name: "Bob".to_string(), other_role }],
        ).await.unwrap();
        let result = manager.get_other_role("agent-other1", "admin", "Bob").await.unwrap();
        assert_eq!(*result.individual_name, "Bob");
    }

    #[tokio::test]
    async fn test_rename_other_role() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-other-rename").await.unwrap();
        dm.ensure_agent_ego_dir("agent-other-rename").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-other-rename", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                full_name: Arc::new(String::new()),
                description: Arc::new("".to_string()),
            }),
            other_role_relations: Arc::new(ArcSwapHashMap::new()),
            description: Arc::new("".to_string()),
        });
        manager.replace_other_roles(
            "agent-other-rename", "admin",
            vec![],
            vec![OtherRoleEntry { role_name: "Bob".to_string(), other_role }],
        ).await.unwrap();
        manager.rename_other_role("agent-other-rename", "admin", "Bob", "Robert").await.unwrap();
        let robert = manager.get_other_role("agent-other-rename", "admin", "Robert").await.unwrap();
        assert_eq!(*robert.individual_name, "Bob");
        let result = manager.get_other_role("agent-other-rename", "admin", "Bob").await;
        assert!(matches!(result, Err(Error::AgentRoleNotFound(_, _))));
    }

    #[tokio::test]
    async fn test_rename_other_role_rejects_invalid_code() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-other-rename-invalid").await.unwrap();
        dm.ensure_agent_ego_dir("agent-other-rename-invalid").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-other-rename-invalid", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                full_name: Arc::new(String::new()),
                description: Arc::new("".to_string()),
            }),
            other_role_relations: Arc::new(ArcSwapHashMap::new()),
            description: Arc::new("".to_string()),
        });
        manager.replace_other_roles(
            "agent-other-rename-invalid", "admin",
            vec![],
            vec![OtherRoleEntry { role_name: "Bob".to_string(), other_role }],
        ).await.unwrap();
        // 非法 new_name 应被拒
        let result = manager.rename_other_role("agent-other-rename-invalid", "admin", "Bob", "a b").await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
        // 合法 new_name 仍成功
        manager.rename_other_role("agent-other-rename-invalid", "admin", "Bob", "Robert2").await.unwrap();
        let robert = manager.get_other_role("agent-other-rename-invalid", "admin", "Robert2").await.unwrap();
        assert_eq!(*robert.individual_name, "Bob");
    }

    #[tokio::test]
    async fn test_replace_other_role_relations() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-other-rel").await.unwrap();
        dm.ensure_agent_ego_dir("agent-other-rel").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-other-rel", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                full_name: Arc::new(String::new()),
                description: Arc::new("".to_string()),
            }),
            other_role_relations: Arc::new(ArcSwapHashMap::new()),
            description: Arc::new("".to_string()),
        });
        manager.replace_other_roles(
            "agent-other-rel", "admin",
            vec![],
            vec![OtherRoleEntry { role_name: "Bob".to_string(), other_role }],
        ).await.unwrap();
        let relation = Arc::new(RoleRelation {
            relation: Arc::new("friend".to_string()),
            full_name: Arc::new(String::new()),
            description: Arc::new("close friend".to_string()),
        });
        manager.replace_other_role_relations(
            "agent-other-rel", "admin", "Bob",
            vec![],
            vec![RoleRelationEntry { role_name: "enemy".to_string(), relation }],
        ).await.unwrap();
        let bob = manager.get_other_role("agent-other-rel", "admin", "Bob").await.unwrap();
        let entry = bob.other_role_relations.get("enemy").unwrap();
        let rel = entry.load();
        assert_eq!(*rel.relation, "friend");
    }

    #[tokio::test]
    async fn test_create_role_rejects_invalid_code() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-invalid-role").await.unwrap();
        dm.ensure_agent_ego_dir("agent-invalid-role").await.unwrap();
        let result = RolePlayManager::get().create_role(
            "agent-invalid-role",
            Arc::new("a b".to_string()),
            Arc::new("Desc".to_string()),
        ).await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
    }

    #[tokio::test]
    async fn test_create_role_from_rejects_invalid_code() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-from-invalid").await.unwrap();
        dm.ensure_agent_ego_dir("agent-from-invalid").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-from-invalid", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let result = manager.create_role_from("agent-from-invalid", "admin", Arc::new("a b".to_string())).await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
    }

    #[tokio::test]
    async fn test_replace_other_roles_rejects_invalid_key() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-other-invalid").await.unwrap();
        dm.ensure_agent_ego_dir("agent-other-invalid").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-other-invalid", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                full_name: Arc::new(String::new()),
                description: Arc::new("".to_string()),
            }),
            other_role_relations: Arc::new(ArcSwapHashMap::new()),
            description: Arc::new("".to_string()),
        });
        let result = manager.replace_other_roles(
            "agent-other-invalid", "admin",
            vec![],
            vec![OtherRoleEntry { role_name: "a b".to_string(), other_role }],
        ).await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
    }

    #[tokio::test]
    async fn test_update_other_role_individual_name_rejects_invalid() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-ind-invalid").await.unwrap();
        dm.ensure_agent_ego_dir("agent-ind-invalid").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-ind-invalid", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                full_name: Arc::new(String::new()),
                description: Arc::new("".to_string()),
            }),
            other_role_relations: Arc::new(ArcSwapHashMap::new()),
            description: Arc::new("".to_string()),
        });
        manager.replace_other_roles(
            "agent-ind-invalid", "admin",
            vec![],
            vec![OtherRoleEntry { role_name: "Bob".to_string(), other_role }],
        ).await.unwrap();
        let result = manager.update_other_role_individual_name(
            "agent-ind-invalid", "admin", "Bob",
            Arc::new("a b".to_string()),
        ).await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
    }

    #[tokio::test]
    async fn test_replace_other_role_relations_rejects_invalid_key() {
        setup().await;
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent-rel-invalid").await.unwrap();
        dm.ensure_agent_ego_dir("agent-rel-invalid").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent-rel-invalid", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                full_name: Arc::new(String::new()),
                description: Arc::new("".to_string()),
            }),
            other_role_relations: Arc::new(ArcSwapHashMap::new()),
            description: Arc::new("".to_string()),
        });
        manager.replace_other_roles(
            "agent-rel-invalid", "admin",
            vec![],
            vec![OtherRoleEntry { role_name: "Bob".to_string(), other_role }],
        ).await.unwrap();
        let relation = Arc::new(RoleRelation {
            relation: Arc::new("friend".to_string()),
            full_name: Arc::new(String::new()),
            description: Arc::new("".to_string()),
        });
        let result = manager.replace_other_role_relations(
            "agent-rel-invalid", "admin", "Bob",
            vec![],
            vec![RoleRelationEntry { role_name: "a b".to_string(), relation }],
        ).await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
    }
}
