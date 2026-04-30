use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use dashmap::DashMap;
use dashmap::DashSet;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::Result;
use crate::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserPrivilege {
    Owner,
    Admin,
    Normal,
}

#[derive(PartialEq, Eq, Hash, Debug, Clone, Serialize, Deserialize)]
pub struct UserIdentifier {
    pub channel_id: String,
    pub user_id: String,
}

// 用户关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRelation {
    pub relation: Arc<String>,
    pub description: Arc<String>,
}

// 用户信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub privilege: Arc<UserPrivilege>,
    pub identifiers: Arc<DashSet<UserIdentifier>>,
    pub relations: Arc<DashMap<String, Arc<UserRelation>>>,
    pub description: Arc<String>,
}

// 用户识别信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecognition {
    pub id: Arc<String>,
    pub user_map: Arc<DashMap<String, Arc<User>>>,
}

// 文件路径常量
pub const EGO_USER_RECOGNITION_PREFIX: &str = "user-recognition-";

// 路径辅助函数
pub fn ego_user_recognition_path(ego_dir: impl AsRef<std::path::Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(format!("{}.json", EGO_USER_RECOGNITION_PREFIX))
}

// 用户识别信息管理器
type UserRecognitionLock = Arc<RwLock<Option<Arc<UserRecognition>>>>;

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
        F: FnMut(Arc<UserRecognition>) -> Result<()>,
    {
        let lock = self.get_or_create_lock(agent_id).await;

        {
            let guard = lock.read().await;
            if let Some(users) = guard.as_ref() {
                return op(users.clone());
            }
        }

        {
            let mut guard = lock.write().await;

            //双重锁定
            if let Some(users) = guard.as_ref() {
                return op(users.clone());
            }

            //准备从文件读取
            let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
            let json_path = ego_user_recognition_path(&ego_dir);

            //文件不存在，新建空用户列表，并写入文件
            if !json_path.exists() {
                let users = UserRecognition {
                    id: Arc::new(agent_id.to_string()),
                    user_map: Arc::new(DashMap::new()),
                };
                let content = serde_json::to_string_pretty(&users)?;
                tokio::fs::write(&json_path, content).await?;
            }

            //重新读取文件
            if !json_path.exists() {
                return Err(Error::AgentNotFound(agent_id.to_string()));
            }
            
            let content = tokio::fs::read_to_string(json_path).await?;
            let users: UserRecognition = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(users));
        }

        //读文件写入后重新读取
        let guard = lock.read().await;
        match guard.as_ref() {
            Some(users) => op(users.clone()),
            None => Err(Error::AgentNotFound(agent_id.to_string())),
        }
    }

    async fn write_user_recognition_ref<F>(&self, agent_id: &str, op: F) -> Result<()>
    where
        F: FnOnce(Option<Arc<UserRecognition>>) -> Result<Arc<UserRecognition>>,
    {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let json_path = ego_user_recognition_path(&ego_dir);

        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;

        //如果内存中没有，先尝试读取
        if guard.is_none() && json_path.exists() {
            let content = tokio::fs::read_to_string(&json_path).await?;
            let entity = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(entity));
        }

        //更新
        let users = guard.take();
        match op(users.clone()) {
            Ok(new_users) => {
                *guard = Some(new_users);
            }
            Err(e) => {
                //失败时要先把原来的放回内存
                *guard = users;
                return Err(e);
            }
        }

        //写入文件
        match guard.as_ref() {
            Some(users) => {
                let content = serde_json::to_string_pretty(users)?;
                tokio::fs::write(json_path, content).await?;
                Ok(())
            }
            None => {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }
    }

    async fn write_user_ref<F>(&self, agent_id: &str, user_name: &str, op: F) -> Result<()>
    where
        F: FnOnce(Arc<User>) -> Result<Arc<User>>,
    {
        self.write_user_recognition_ref(agent_id, |users_or_none| {
            match users_or_none {
                Some(users) => {
                    if let Some(mut user) = users.user_map.get_mut(user_name) {
                        *user = op(user.clone())?;
                    }
                    Ok(users)
                },
                None => return Err(Error::AgentNotFound(agent_id.to_string())),
            }
        }).await
    }

    pub async fn get_users(&self, agent_id: &str) -> Result<Arc<UserRecognition>> {
        let mut result = Err(Error::AgentNotFound(agent_id.to_string()));
        self.read_user_recognition_ref(agent_id, |users| {
            result = Ok(users.clone());
            Ok(())
        }).await?;
        result
    }

    pub async fn get_user(&self, agent_id: &str, user_name: &str) -> Result<Arc<User>> {
        let mut result = Err(Error::AgentUserNotFound(agent_id.to_string(), user_name.to_string()));
        self.read_user_recognition_ref(agent_id, |users| {
            if let Some(user) = users.user_map.get(user_name) {
                result = Ok(user.clone());
            }
            Ok(())
        }).await?;
        result
    }

    pub async fn replace_users(&self, agent_id: &str, mut remove_user_name_set: HashSet<String>, mut insert_user_map: HashMap<String, Arc<User>>) -> Result<()> {
        self.write_user_recognition_ref(agent_id, |users_or_none| {
            if let Some(users) = users_or_none {
                for user_name in remove_user_name_set.drain() {
                    users.user_map.remove(&user_name);
                }
                for (user_name, user) in insert_user_map.drain() {
                    users.user_map.insert(user_name, user);
                }
                Ok(users)
            }
            else {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }

    pub async fn rename_user(&self, agent_id: &str, user_name: &str, new_name: &str) -> Result<()> {
        self.write_user_recognition_ref(agent_id, |users_or_none| {
            if let Some(users) = users_or_none {
                if users.user_map.contains_key(new_name) {
                    if let Some((_,user)) = users.user_map.remove(user_name) {
                        users.user_map.insert(new_name.to_string(), user);
                        Ok(users)
                    }
                    else {
                        Err(Error::AgentUserNotFound(agent_id.to_string(), user_name.to_string()))
                    }
                }
                else {
                    Err(Error::AgentUserAlreadyExists(agent_id.to_string(), new_name.to_string()))
                }
            }
            else {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }

    pub async fn update_user_privilege(&self, agent_id: &str, user_name: &str, privilege: Arc<UserPrivilege>) -> Result<()> {
        self.write_user_ref(agent_id, user_name, |user| {
            Ok(Arc::new(User {
                privilege: privilege.clone(),
                identifiers: user.identifiers.clone(),
                relations: user.relations.clone(),
                description: user.description.clone(),
            }))
        }).await
    }

    pub async fn update_user_description(&self, agent_id: &str, user_name: &str, description: Arc<String>) -> Result<()> {
        self.write_user_ref(agent_id, user_name, |user| {
            Ok(Arc::new(User {
                privilege: user.privilege.clone(),
                identifiers: user.identifiers.clone(),
                relations: user.relations.clone(),
                description: description.clone(),
            }))
        }).await
    }

    pub async fn replace_user_identifiers(&self, agent_id: &str, user_name: &str, mut remove_identifiers: HashSet<UserIdentifier>, mut insert_identifiers: HashSet<UserIdentifier>) -> Result<()> {
        self.write_user_ref(agent_id, user_name, |user| {
            for identifier in remove_identifiers.drain() {
                user.identifiers.remove(&identifier);
            }
            for identifier in insert_identifiers.drain() {
                user.identifiers.insert(identifier);
            }
            Ok(user)
        }).await
    }

    pub async fn replace_user_relations(&self, agent_id: &str, user_name: &str, mut remove_relations: HashSet<String>, mut insert_relations: HashMap<String,Arc<UserRelation>>) -> Result<()> {
        self.write_user_ref(agent_id, user_name, |user| {
            for identifier in remove_relations.drain() {
                user.relations.remove(&identifier);
            }
            for (identifier, relation) in insert_relations.drain() {
                user.relations.insert(identifier, relation);
            }
            Ok(user)
        }).await
    }
}
