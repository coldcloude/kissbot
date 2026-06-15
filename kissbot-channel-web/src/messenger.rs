use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use kissbot_api::channel::{AttachmentDownloadRequestDTO, OutgoingMessageDTO};
use kissbot_channel::{
    AttachmentDownloadPayloadSender, AttachmentDownloadResponseHeader,
    AttachmentInfo, GroupChangeEvent, GroupChangeHandler, GroupChangeType,
    GroupInfo, IncomingMessage, IncomingMessageEvent, IncomingMessageHandler,
    Messenger, MessengerCreator, MessengerInfo, OutgoingMessageResponse, UserInfo,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::attachment::AttachmentStore;
use crate::error::{Error, Result};

const ADMIN_USER_GROUP_PREFIX: &str = "a_";
const USER_ID_PREFIX: &str = "u";
const GROUP_ID_PREFIX: &str = "g";

// ========== JSON 配置 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerConfig {
    pub messenger_id: Arc<String>,
    pub admin_key: Arc<String>,
    pub user_key: Arc<String>,
    pub admin: AdminInfo,
    pub users: DashMap<String, UserConfig>,
    pub groups: DashMap<String, GroupConfig>,
    pub next_user_seq: u32,
    pub next_group_seq: u32,
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

pub fn admin_user_group_id(user_id: &str) -> String {
    format!("{}{}", ADMIN_USER_GROUP_PREFIX, user_id)
}

// ========== WebMessenger ==========

pub struct WebMessenger {
    messenger_id: Arc<String>,
    config_path: PathBuf,
    config: Arc<RwLock<MessengerConfig>>,
    msg_id_seq: AtomicU32,
    pub(crate) on_group_change: Weak<dyn GroupChangeHandler>,
    pub(crate) on_incoming_messages: Weak<dyn IncomingMessageHandler>,
    pub(crate) on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
    sse_senders: DashMap<String, DashMap<String, flume::Sender<Arc<IncomingMessage>>>>,
}

impl WebMessenger {
    pub fn new(
        messenger_id: Arc<String>,
        config_path: PathBuf,
        config: Arc<RwLock<MessengerConfig>>,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
    ) -> Self {
        Self {
            messenger_id,
            config_path,
            config,
            msg_id_seq: AtomicU32::new(0),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            sse_senders: DashMap::new(),
        }
    }

    async fn load_config(path: &str) -> Result<(PathBuf, Arc<RwLock<MessengerConfig>>)> {
        let config_path = PathBuf::from(path);
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| Error::ConfigError(format!("Failed to read config file: {}", e)))?;
        let config: MessengerConfig = serde_json::from_str(&content)
            .map_err(|e| Error::ConfigError(format!("Failed to parse config file: {}", e)))?;
        Ok((config_path, Arc::new(RwLock::new(config))))
    }

    fn next_msg_id(&self) -> String {
        let now = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let seq = self.msg_id_seq.fetch_add(1, Ordering::SeqCst) % 1_000_000;
        format!("{}{:06}", now, seq)
    }

    async fn save(&self, cfg: &MessengerConfig) -> Result<()> {
        let json = serde_json::to_string_pretty(cfg)?;
        std::fs::write(&self.config_path, json)?;
        Ok(())
    }

    pub async fn admin_key(&self) -> Arc<String> {
        self.config.read().await.admin_key.clone()
    }

    pub async fn user_key(&self) -> Arc<String> {
        self.config.read().await.user_key.clone()
    }

    pub async fn admin_info(&self) -> AdminInfo {
        self.config.read().await.admin.clone()
    }

    pub async fn get_group(&self, group_id: &str) -> Option<GroupConfig> {
        self.config.read().await.groups.get(group_id).map(|g| g.clone())
    }

    pub async fn is_admin_user_group(&self, group_id: &str) -> bool {
        let cfg = self.config.read().await;
        match group_id.strip_prefix(ADMIN_USER_GROUP_PREFIX) {
            Some(user_id) => cfg.users.contains_key(user_id) && !cfg.groups.contains_key(group_id),
            None => false,
        }
    }

    pub async fn is_user(&self, user_id: &str) -> bool {
        self.config.read().await.users.contains_key(user_id)
    }

    pub async fn list_users(&self) -> Vec<UserConfig> {
        self.config.read().await.users.iter().map(|u| u.clone()).collect()
    }

    pub async fn list_groups_raw(&self) -> Vec<GroupConfig> {
        self.config.read().await.groups.iter().map(|g| g.clone()).collect()
    }

    fn alloc_user_id(cfg: &mut MessengerConfig) -> String {
        let n = cfg.next_user_seq;
        cfg.next_user_seq += 1;
        format!("{}{}", USER_ID_PREFIX, n)
    }

    fn alloc_group_id(cfg: &mut MessengerConfig) -> String {
        let n = cfg.next_group_seq;
        cfg.next_group_seq += 1;
        format!("{}{}", GROUP_ID_PREFIX, n)
    }

    pub async fn add_user(&self, user_name: &str) -> Result<String> {
        let mut cfg = self.config.write().await;
        let user_id = Self::alloc_user_id(&mut cfg);
        cfg.users.insert(user_id.clone(), UserConfig {
            user_id: Arc::new(user_id.clone()),
            user_name: Arc::new(user_name.to_string()),
        });
        self.save(&cfg).await?;
        Ok(user_id)
    }

    pub async fn remove_user(&self, user_id: &str) -> Result<()> {
        let mut cfg = self.config.write().await;
        if cfg.users.remove(user_id).is_none() {
            return Err(Error::UserNotFound(user_id.to_string()));
        }
        for mut g in cfg.groups.iter_mut() {
            g.members.retain(|m| m.as_str() != user_id);
        }
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn add_group(&self, group_name: &str, member_ids: Vec<String>) -> Result<String> {
        let mut cfg = self.config.write().await;
        let group_id = Self::alloc_group_id(&mut cfg);
        cfg.groups.insert(group_id.clone(), GroupConfig {
            group_id: Arc::new(group_id.clone()),
            group_name: Arc::new(group_name.to_string()),
            members: member_ids.into_iter().map(Arc::new).collect(),
        });
        self.save(&cfg).await?;
        Ok(group_id)
    }

    pub async fn rename_group(&self, group_id: &str, new_name: &str) -> Result<()> {
        let cfg = self.config.write().await;
        let mut g = cfg.groups.get_mut(group_id)
            .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
        g.group_name = Arc::new(new_name.to_string());
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn manage_members(&self, group_id: &str, add_ids: &[String], remove_ids: &[String]) -> Result<()> {
        let cfg = self.config.write().await;
        let mut g = cfg.groups.get_mut(group_id)
            .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
        for id in add_ids {
            if !g.members.iter().any(|m| m.as_str() == id) {
                g.members.push(Arc::new(id.clone()));
            }
        }
        g.members.retain(|m| !remove_ids.iter().any(|r| r.as_str() == m.as_str()));
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

    fn get_on_incoming(&self) -> Result<Arc<dyn IncomingMessageHandler>> {
        self.on_incoming_messages.upgrade()
            .ok_or_else(|| Error::InternalError("incoming message handler is None".to_string()))
    }

    fn get_on_group_change(&self) -> Result<Arc<dyn GroupChangeHandler>> {
        self.on_group_change.upgrade()
            .ok_or_else(|| Error::InternalError("group change handler is None".to_string()))
    }

    async fn fire_incoming(&self, messenger_id: &str, user_id: &str, group_id: &str, msg_id: &str, is_self: usize, msg_type: &str, content: &str, time: &str) {
        let handler = match self.get_on_incoming() {
            Ok(h) => h,
            Err(_) => return,
        };
        let incoming = Arc::new(IncomingMessage {
            msg_id: Arc::new(msg_id.to_string()),
            messenger_id: Arc::new(messenger_id.to_string()),
            user_id: Arc::new(user_id.to_string()),
            group_id: Arc::new(group_id.to_string()),
            is_self,
            msg_type: Arc::new(msg_type.to_string()),
            content: Arc::new(content.to_string()),
            time: Arc::new(time.to_string()),
        });
        let event = Arc::new(IncomingMessageEvent {
            messenger_id: Arc::new(messenger_id.to_string()),
            user_id: Arc::new(user_id.to_string()),
            group_id: Arc::new(group_id.to_string()),
            messages: Arc::new(vec![incoming]),
        });
        let _ = handler.handle_incoming_message(event).await;
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

        let msg_id = self.next_msg_id();
        let messenger_id = self.messenger_id.clone();

        for member_id in group.members.iter() {
            let is_self = if member_id.as_str() == admin_id.as_str() { 1 } else { 0 };
            self.fire_incoming(messenger_id.as_str(), &admin_id, group_id, &msg_id, is_self, msg_type, content, time).await;
        }

        Ok(())
    }

    pub async fn notify_group_change(&self, user_id: &str, group_id: &str, change_type: GroupChangeType, time: &str) {
        let handler = match self.get_on_group_change() {
            Ok(h) => h,
            Err(_) => return,
        };
        let event = Arc::new(GroupChangeEvent {
            msg_id: Arc::new(self.next_msg_id()),
            messenger_id: self.messenger_id.clone(),
            user_id: Arc::new(user_id.to_string()),
            group_id: Arc::new(group_id.to_string()),
            change_type,
            time: Arc::new(time.to_string()),
        });
        handler.handle_group_change(event).await;
    }

    pub async fn build_messenger_info(&self) -> MessengerInfo {
        let cfg = self.config.read().await;
        let full_user_map: Arc<DashMap<String, Arc<UserInfo>>> = Arc::new(DashMap::new());

        for user_ref in cfg.users.iter() {
            let ug_map: Arc<DashMap<String, Arc<GroupInfo>>> = Arc::new(DashMap::new());

            for g_ref in cfg.groups.iter() {
                if g_ref.members.iter().any(|m| m.as_str() == user_ref.user_id.as_str()) {
                    ug_map.insert(g_ref.group_id.to_string(), Arc::new(GroupInfo {
                        group_id: g_ref.group_id.clone(),
                        group_name: g_ref.group_name.clone(),
                    }));
                }
            }

            let gid = admin_user_group_id(&user_ref.user_id);
            if !cfg.groups.contains_key(&gid) {
                ug_map.insert(gid.clone(), Arc::new(GroupInfo {
                    group_id: Arc::new(gid),
                    group_name: user_ref.user_name.clone(),
                }));
            }

            full_user_map.insert(user_ref.user_id.to_string(), Arc::new(UserInfo {
                user_id: user_ref.user_id.clone(),
                user_name: user_ref.user_name.clone(),
                group_map: ug_map,
            }));
        }

        MessengerInfo {
            messenger_id: self.messenger_id.clone(),
            messenger_name: Arc::new("Web Chat".to_string()),
            user_map: full_user_map,
        }
    }
}

// ========== Creator 与 Messenger trait 实现 ==========

/// 持有完整配置和路径，create() 时用预读的配置构造 WebMessenger。
pub struct WebMessengerCreator {
    config_path: PathBuf,
    config: Arc<RwLock<MessengerConfig>>,
}

impl WebMessengerCreator {
    pub async fn new(config_path: &str) -> Result<Self> {
        let path = PathBuf::from(config_path);
        let content = std::fs::read_to_string(&path)?;
        let config: MessengerConfig = serde_json::from_str(&content)?;
        Ok(Self {
            config_path: path,
            config: Arc::new(RwLock::new(config)),
        })
    }

    pub async fn api_key(&self) -> Arc<String> {
        let config = self.config.read().await;
        config.user_key.clone()
    }

    pub async fn messenger_id(&self) -> Arc<String> {
        let config = self.config.read().await;
        config.messenger_id.clone()
    }
}

#[async_trait]
impl kissbot_channel::MessengerCreator<WebMessenger> for WebMessengerCreator {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
    ) -> kissbot_channel::error::Result<Arc<WebMessenger>> {
        let mid = self.config.read().await.messenger_id.clone();
        let messenger = Arc::new(WebMessenger::new(
            mid,
            self.config_path.clone(),
            self.config.clone(),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
        ));

        Ok(messenger)
    }
}

#[async_trait]
impl Messenger for WebMessenger {
    async fn get_info(&self) -> kissbot_channel::error::Result<Arc<MessengerInfo>> {
        let info = self.build_messenger_info().await;
        Ok(Arc::new(info))
    }

    async fn send_message(&self, message: OutgoingMessageDTO, _attachment_sn: Arc<AtomicU32>) -> kissbot_channel::error::Result<Arc<OutgoingMessageResponse>> {
        let msg_id = self.next_msg_id();
        let messenger_id = self.messenger_id.clone();

        let cfg = self.config.read().await;
        let group = cfg.groups.get(message.group_id.as_str())
            .map(|g| g.clone())
            .ok_or_else(|| kissbot_channel::Error::InternalError(format!("group not found: {}", message.group_id)))?;
        let sender_id = message.user_id.clone();
        drop(cfg);

        for member_id in group.members.iter() {
            let is_self = if member_id.as_str() == sender_id.as_str() { 1 } else { 0 };
            let incoming = Arc::new(IncomingMessage {
                msg_id: Arc::new(msg_id.clone()),
                messenger_id: messenger_id.clone(),
                user_id: Arc::new(sender_id.clone()),
                group_id: Arc::new(message.group_id.clone()),
                is_self,
                msg_type: Arc::new(message.msg_type.clone()),
                content: Arc::new(message.content.clone()),
                time: Arc::new(message.time.clone()),
            });
            let event = Arc::new(IncomingMessageEvent {
                messenger_id: messenger_id.clone(),
                user_id: Arc::new(sender_id.clone()),
                group_id: Arc::new(message.group_id.clone()),
                messages: Arc::new(vec![incoming]),
            });

            if let Some(handler) = self.on_incoming_messages.upgrade() {
                handler.handle_incoming_message(event).await;
            }
        }

        let upload_id_map: Arc<DashMap<String, u32>> = Arc::new(DashMap::new());
        Ok(Arc::new(OutgoingMessageResponse {
            msg_id: Arc::new(msg_id),
            attachment_upload_id_map: upload_id_map,
        }))
    }

    async fn send_attachment_payload(&self, _id: u32, _size: u32, _pos: u64, _data: &[u8]) -> kissbot_channel::error::Result<()> {
        Ok(())
    }

    async fn download_attachment_header(&self, request: AttachmentDownloadRequestDTO, _attachment_sn: Arc<AtomicU32>) -> kissbot_channel::error::Result<Arc<AttachmentDownloadResponseHeader>> {
        let store = AttachmentStore::new("attachments");
        let meta = store.get_meta_by_key(&request.key)
            .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;

        Ok(Arc::new(AttachmentDownloadResponseHeader {
            download_id: 0,
            metadata: Arc::new(AttachmentInfo {
                att_id: meta.att_id,
                mime_type: meta.mime_type,
                size_bytes: meta.size_bytes,
            }),
        }))
    }
}
