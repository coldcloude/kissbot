use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use dashmap::DashMap;
use kissbot_channel::{
    Channel, GroupChangeEvent, GroupChangeHandler, GroupChangeType,
    Messenger, ChannelInfo as ChInfo,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::channel::WebChannel;
use crate::error::{Error, Result};

/// 单聊群组 suffix: "{user_id}_admin"
const ADMIN_USER_GROUP_SUFFIX: &str = "_admin";

// ========== JSON 配置格式（DashMap 序列化为 JSON 对象） ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerConfig {
    pub admin_key: Arc<String>,
    pub user_key: Arc<String>,
    pub admin: AdminInfo,
    /// key: user_id, value: UserConfig
    pub users: DashMap<String, UserConfig>,
    /// key: group_id, value: GroupConfig
    pub groups: DashMap<String, GroupConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminInfo {
    pub user_id: Arc<String>,
    pub user_name: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    pub user_id: Arc<String>,
    pub user_name: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    pub group_id: Arc<String>,
    pub group_name: Arc<String>,
    pub members: Vec<Arc<String>>,
}

// ========== WebMessenger ==========

pub struct WebMessenger {
    messenger_id: String,
    config_path: PathBuf,
    config: Arc<RwLock<MessengerConfig>>,
    channels: DashMap<String, DashMap<String, Arc<WebChannel>>>,
    on_group_change: tokio::sync::RwLock<Option<Weak<dyn GroupChangeHandler>>>,
    sse_receivers: DashMap<String, DashMap<String, flume::Receiver<Arc<kissbot_channel::IncomingMessage>>>>,
}

impl WebMessenger {
    pub async fn load(path: &str) -> Result<Self> {
        let config_path = PathBuf::from(path);
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| Error::ConfigError(format!("Failed to read config file: {}", e)))?;
        let config: MessengerConfig = serde_json::from_str(&content)
            .map_err(|e| Error::ConfigError(format!("Failed to parse config file: {}", e)))?;

        Ok(Self {
            messenger_id: "web".to_string(),
            config_path,
            config: Arc::new(RwLock::new(config)),
            channels: DashMap::new(),
            on_group_change: tokio::sync::RwLock::new(None),
            sse_receivers: DashMap::new(),
        })
    }

    async fn save(&self, cfg: &MessengerConfig) -> Result<()> {
        let json = serde_json::to_string_pretty(cfg)?;
        std::fs::write(&self.config_path, json)?;
        Ok(())
    }

    // ========== 认证信息 ==========

    pub async fn admin_key(&self) -> Arc<String> {
        self.config.read().await.admin_key.clone()
    }

    pub async fn user_key(&self) -> Arc<String> {
        self.config.read().await.user_key.clone()
    }

    pub async fn admin_info(&self) -> AdminInfo {
        self.config.read().await.admin.clone()
    }

    // ========== 读操作（持有读锁期间访问 DashMap） ==========

    /// 获取群组
    pub async fn get_group(&self, group_id: &str) -> Option<GroupConfig> {
        let cfg = self.config.read().await;
        cfg.groups.get(group_id).map(|g| g.clone())
    }

    /// 判断是否为 admin-user 自动生成群组
    pub async fn is_admin_user_group(&self, group_id: &str) -> bool {
        let cfg = self.config.read().await;
        let suffix = ADMIN_USER_GROUP_SUFFIX;
        if let Some(gid) = group_id.strip_suffix(suffix) {
            cfg.users.contains_key(gid) && !cfg.groups.contains_key(group_id)
        } else {
            false
        }
    }

    /// admin 是否在 user DashMap 中（仅用于判断是否是普通 user）
    pub async fn is_user(&self, user_id: &str) -> bool {
        self.config.read().await.users.contains_key(user_id)
    }

    /// 获取所有 user 列表（用于 API 返回）
    pub async fn list_users(&self) -> Vec<UserConfig> {
        self.config.read().await.users.iter().map(|u| u.clone()).collect()
    }

    /// 获取所有配置群组列表（用于 API 返回）
    pub async fn list_groups_raw(&self) -> Vec<GroupConfig> {
        self.config.read().await.groups.iter().map(|g| g.clone()).collect()
    }

    /// 生成下一个可用 group_id
    pub async fn next_group_id(&self) -> String {
        let cfg = self.config.read().await;
        let mut i = 1;
        loop {
            let gid = format!("group_{}", i);
            if !cfg.groups.contains_key(&gid) {
                // 检查是否与可能的 admin-user 单聊群组 ID 冲突
                let dm_conflict = cfg.users.iter().any(|u| {
                    format!("{}{}", u.user_id, ADMIN_USER_GROUP_SUFFIX) == gid
                });
                if !dm_conflict {
                    return gid;
                }
            }
            i += 1;
        }
    }

    // ========== 写操作 ==========

    pub async fn add_user(&self, user_id: &str, user_name: &str) -> Result<()> {
        let cfg = self.config.write().await;
        if cfg.users.contains_key(user_id) {
            return Err(Error::ConfigError(format!("User already exists: {}", user_id)));
        }
        cfg.users.insert(user_id.to_string(), UserConfig {
            user_id: Arc::new(user_id.to_string()),
            user_name: Arc::new(user_name.to_string()),
        });
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn remove_user(&self, user_id: &str) -> Result<()> {
        let cfg = self.config.write().await;
        if cfg.users.remove(user_id).is_none() {
            return Err(Error::UserNotFound(user_id.to_string()));
        }
        // 同时从所有群组中移除该 user
        for mut g in cfg.groups.iter_mut() {
            g.members.retain(|m| m.as_str() != user_id);
        }
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn add_group(&self, group_id: &str, group_name: &str, member_ids: Vec<String>) -> Result<()> {
        let cfg = self.config.write().await;
        if cfg.groups.contains_key(group_id) {
            return Err(Error::ConfigError(format!("Group already exists: {}", group_id)));
        }
        cfg.groups.insert(group_id.to_string(), GroupConfig {
            group_id: Arc::new(group_id.to_string()),
            group_name: Arc::new(group_name.to_string()),
            members: member_ids.into_iter().map(Arc::new).collect(),
        });
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn rename_group(&self, group_id: &str, new_name: &str) -> Result<()> {
        let cfg = self.config.write().await;
        if let Some(mut g) = cfg.groups.get_mut(group_id) {
            g.group_name = Arc::new(new_name.to_string());
        } else {
            return Err(Error::GroupNotFound(group_id.to_string()));
        }
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn manage_members(&self, group_id: &str, add_ids: &[String], remove_ids: &[String]) -> Result<()> {
        let cfg = self.config.write().await;
        if let Some(mut g) = cfg.groups.get_mut(group_id) {
            for id in add_ids {
                if !g.members.iter().any(|m| m.as_str() == id) {
                    g.members.push(Arc::new(id.clone()));
                }
            }
            g.members.retain(|m| !remove_ids.iter().any(|r| r.as_str() == m.as_str()));
        } else {
            return Err(Error::GroupNotFound(group_id.to_string()));
        }
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn delete_group(&self, group_id: &str) -> Result<()> {
        let cfg = self.config.write().await;
        if cfg.groups.remove(group_id).is_none() {
            return Err(Error::GroupNotFound(group_id.to_string()));
        }
        self.save(&cfg).await?;
        Ok(())
    }

    // ========== Messenger 接口实现 ==========

    #[allow(dead_code)]
    pub fn get_sse_receiver(&self, user_id: &str, group_id: &str) -> Option<flume::Receiver<Arc<kissbot_channel::IncomingMessage>>> {
        if let Some(group_map) = self.sse_receivers.get(user_id) {
            if let Some(receiver) = group_map.get(group_id) {
                return Some(receiver.clone());
            }
        }
        None
    }

    pub fn get_channel(&self, user_id: &str, group_id: &str) -> Option<Arc<WebChannel>> {
        if let Some(group_map) = self.channels.get(user_id) {
            if let Some(channel) = group_map.get(group_id) {
                return Some(channel.clone());
            }
        }
        None
    }

    pub async fn admin_send_message(
        &self,
        group_id: &str,
        content: &str,
        msg_type: &str,
        time: &str,
    ) -> Result<()> {
        let cfg = self.config.read().await;
        let group = cfg.groups.get(group_id)
            .map(|g| g.clone())
            .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
        let admin_id = cfg.admin.user_id.clone();
        drop(cfg);

        let msg_id = uuid::Uuid::new_v4().to_string();

        for member_id in group.members.iter() {
            if member_id.as_str() == admin_id.as_str() {
                continue;
            }
            if let Some(channel) = self.get_channel(member_id, group_id) {
                let incoming = Arc::new(kissbot_channel::IncomingMessage {
                    msg_id: Arc::new(msg_id.clone()),
                    messenger_id: Arc::new(self.messenger_id.clone()),
                    user_id: admin_id.clone(),
                    group_id: Arc::new(group_id.to_string()),
                    is_self: 1,
                    msg_type: Arc::new(msg_type.to_string()),
                    content: Arc::new(content.to_string()),
                    time: Arc::new(time.to_string()),
                });
                channel.trigger_incoming_message(incoming).await;
            }
        }

        Ok(())
    }

    pub async fn notify_group_change(&self, user_id: &str, group_id: &str, change_type: GroupChangeType, time: &str) {
        let handler = self.on_group_change.read().await;
        if let Some(weak) = handler.as_ref() {
            if let Some(handler) = weak.upgrade() {
                let event = Arc::new(GroupChangeEvent {
                    messenger_id: Arc::new(self.messenger_id.clone()),
                    user_id: Arc::new(user_id.to_string()),
                    group_id: Arc::new(group_id.to_string()),
                    change_type,
                    time: Arc::new(time.to_string()),
                });
                handler.handle_group_change(event).await;
            }
        }
    }

    /// 根据 MessengerConfig 实时组装 MessengerInfo
    pub async fn build_messenger_info(&self) -> kissbot_channel::MessengerInfo {
        let cfg = self.config.read().await;
        let mut user_entries: Vec<Arc<kissbot_channel::UserInfo>> = Vec::new();

        for user_ref in cfg.users.iter() {
            let ug_map: Arc<DashMap<String, Arc<kissbot_channel::GroupInfo>>> = Arc::new(DashMap::new());

            // 配置群组
            for g_ref in cfg.groups.iter() {
                if g_ref.members.iter().any(|m| m.as_str() == user_ref.user_id.as_str()) {
                    ug_map.insert(g_ref.group_id.to_string(), Arc::new(kissbot_channel::GroupInfo {
                        group_id: g_ref.group_id.clone(),
                        group_name: g_ref.group_name.clone(),
                    }));
                }
            }

            // 自动注入 admin-user 单聊群组
            let gid = format!("{}{}", user_ref.user_id, ADMIN_USER_GROUP_SUFFIX);
            if !cfg.groups.contains_key(&gid) {
                ug_map.insert(gid.clone(), Arc::new(kissbot_channel::GroupInfo {
                    group_id: Arc::new(gid),
                    group_name: user_ref.user_name.clone(),
                }));
            }

            user_entries.push(Arc::new(kissbot_channel::UserInfo {
                user_id: user_ref.user_id.clone(),
                user_name: user_ref.user_name.clone(),
                group_map: ug_map,
            }));
        }

        let full_user_map: Arc<DashMap<String, Arc<kissbot_channel::UserInfo>>> = Arc::new(DashMap::new());
        for u in &user_entries {
            full_user_map.insert(u.user_id.to_string(), u.clone());
        }

        kissbot_channel::MessengerInfo {
            messenger_id: Arc::new(self.messenger_id.clone()),
            messenger_name: Arc::new("Web Chat".to_string()),
            user_map: full_user_map,
        }
    }
}

// ========== Messenger trait 实现 ==========

#[async_trait]
impl Messenger for WebMessenger {
    fn messenger_id(&self) -> &str {
        &self.messenger_id
    }

    async fn get_info(&self) -> kissbot_channel::error::Result<Arc<kissbot_channel::MessengerInfo>> {
        let info = self.build_messenger_info().await;
        Ok(Arc::new(info))
    }

    async fn create_channel(&self, user_id: &str, group_id: &str) -> kissbot_channel::error::Result<Arc<dyn Channel>> {
        // admin 不在 user DashMap 中，所以自然地拒绝 admin 创建 channel
        if !self.is_user(user_id).await {
            return Err(kissbot_channel::Error::InternalError(format!("user not found: {}", user_id)));
        }

        let info = Arc::new(ChInfo {
            messenger_id: Arc::new(self.messenger_id.clone()),
            group_id: Arc::new(group_id.to_string()),
            user_id: Arc::new(user_id.to_string()),
        });

        if let Some(channel) = self.get_channel(user_id, group_id) {
            return Ok(channel as Arc<dyn Channel>);
        }

        let (channel, rx) = WebChannel::new(info);
        let channel = Arc::new(channel);

        self.channels
            .entry(user_id.to_string())
            .or_insert_with(DashMap::new)
            .insert(group_id.to_string(), channel.clone());

        self.sse_receivers
            .entry(user_id.to_string())
            .or_insert_with(DashMap::new)
            .insert(group_id.to_string(), rx);

        Ok(channel as Arc<dyn Channel>)
    }

    fn register_on_group_change(&self, callback: Weak<dyn GroupChangeHandler>) {
        *self.on_group_change.blocking_write() = Some(callback);
    }
}
