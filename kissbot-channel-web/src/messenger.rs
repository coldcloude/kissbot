use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use uuid::Uuid;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use dashmap::{DashMap, DashSet};
use kissbot_api::channel::{
    AttachmentDownloadRequest, AttachmentPayloadResponse, GroupInfo, IncomingMessage, MessengerInfo, OutgoingMessage, OutgoingMessageResponse,
    UserInfo,
};
use kissbot_api::message::{AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel::{
    AttachmentDownloadPayloadSender, GroupChangeEvent, GroupChangeHandler, GroupChangeType,
    IncomingMessageHandler, UserRemoveEvent, UserRemoveHandler,
    Messenger, MessengerCreator,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::attachment::AttachmentStore;
use crate::error::{Error, Result};
use crate::message_store::MessageStore;

// =========== SSE 分发器（给 admin 前端推送） ===========

pub struct SseDispatcher {
    senders: Arc<Mutex<HashMap<Uuid, flume::Sender<String>>>>,
}

impl SseDispatcher {
    pub fn new() -> Self {
        Self { senders: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn register(&self) -> flume::Receiver<String> {
        let (tx, rx) = flume::unbounded();
        let id = Uuid::new_v4();
        self.senders.lock().unwrap().insert(id, tx);
        rx
    }

    pub fn push(&self, data: &str) {
        let mut senders = self.senders.lock().unwrap();
        senders.retain(|_, tx| tx.try_send(data.to_string()).is_ok());
    }
}

const ADMIN_USER_GROUP_PREFIX: &str = "a_";
pub static ADMIN_USER_ID: LazyLock<Arc<String>> = LazyLock::new(|| Arc::new("admin".to_string()));
const USER_ID_PREFIX: &str = "u";
const GROUP_ID_PREFIX: &str = "g";

// ========== JSON 配置 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebMessengerRepo {
    pub messenger_id: Arc<String>,
    pub admin_name: Arc<String>,
    pub users: Arc<DashMap<String, Arc<UserConfig>>>,
    pub groups: Arc<DashMap<String, Arc<GroupConfig>>>,
    pub next_user_seq: u32,
    pub next_group_seq: u32,
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
    pub members: Arc<DashSet<String>>,
}

pub fn admin_user_group_id(user_id: &str) -> String {
    format!("{}{}", ADMIN_USER_GROUP_PREFIX, user_id)
}

// ========== WebMessenger ==========

pub struct WebMessenger {
    pub messenger_id: Arc<String>,
    repo_path: PathBuf,
    config: Arc<RwLock<WebMessengerRepo>>,
    msg_id_seq: AtomicU32,
    on_group_change: Weak<dyn GroupChangeHandler>,
    on_incoming_messages: Weak<dyn IncomingMessageHandler>,
    on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
    on_user_remove: Weak<dyn UserRemoveHandler>,
    pub sse: Arc<SseDispatcher>,
    pub attachment_store: Arc<AttachmentStore>,
    pub message_store: Arc<MessageStore>,
}

impl WebMessenger {
    pub fn new(
        messenger_id: Arc<String>,
        repo_path: PathBuf,
        config: Arc<RwLock<WebMessengerRepo>>,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
        attachment_dir: &str,
        message_store: Arc<MessageStore>,
    ) -> Self {
        Self {
            messenger_id,
            repo_path,
            config,
            msg_id_seq: AtomicU32::new(0),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            on_user_remove,
            sse: Arc::new(SseDispatcher::new()),
            attachment_store: Arc::new(AttachmentStore::new(attachment_dir)),
            message_store,
        }
    }

    pub fn next_msg_id(&self) -> Arc<String> {
        let now = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let seq = self.msg_id_seq.fetch_add(1, Ordering::SeqCst) % 1_000_000;
        Arc::new(format!("{}{:06}", now, seq))
    }

    async fn save(&self, cfg: &WebMessengerRepo) -> Result<()> {
        let json = serde_json::to_string_pretty(cfg)?;
        std::fs::write(&self.repo_path, json)?;
        Ok(())
    }

    pub async fn admin_name(&self) -> Arc<String> {
        self.config.read().await.admin_name.clone()
    }

    pub async fn config_users(&self) -> Arc<DashMap<String, Arc<UserConfig>>> {
        self.config.read().await.users.clone()
    }

    pub async fn config_groups(&self) -> Arc<DashMap<String, Arc<GroupConfig>>> {
        self.config.read().await.groups.clone()
    }

    /// 判断 group_id 是否为 user_id 的 admin-user 单聊组（a_{user_id}）。
    /// 纯格式判断，不读 config。
    pub fn is_admin_user_group_for(user_id: &str, group_id: &str) -> bool {
        group_id == admin_user_group_id(user_id)
    }

    /// group_id 是 admin-user 单聊组时返回对应的 user_id，否则 None。
    /// 验证前缀匹配、user 存在于 config。
    fn parse_admin_user_group_ref(cfg: &WebMessengerRepo, group_id: &str) -> Option<String> {
        let uid = group_id.strip_prefix(ADMIN_USER_GROUP_PREFIX)?;
        if cfg.users.contains_key(uid) {
            Some(uid.to_string())
        } else {
            None
        }
    }

    pub async fn update_admin_name(&self, new_name: &str) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.admin_name = Arc::new(new_name.to_string());
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn rename_user(&self, user_id: &str, new_name: &str) -> Result<()> {
        let cfg = self.config.write().await;
        let mut u = cfg.users.get_mut(user_id)
            .ok_or_else(|| Error::UserNotFound(user_id.to_string()))?;
        let old = u.clone();
        *u = Arc::new(UserConfig {
            user_id: old.user_id.clone(),
            user_name: Arc::new(new_name.to_string()),
        });
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn add_user(&self, user_name: &str) -> Result<String> {
        let mut cfg = self.config.write().await;
        let n = cfg.next_user_seq;
        cfg.next_user_seq += 1;
        let user_id = format!("{}{}", USER_ID_PREFIX, n);
        cfg.users.insert(user_id.clone(), Arc::new(UserConfig {
            user_id: Arc::new(user_id.clone()),
            user_name: Arc::new(user_name.to_string()),
        }));
        self.save(&cfg).await?;
        Ok(user_id)
    }

    pub async fn remove_user(&self, user_id: &str) -> Result<()> {
        let cfg = self.config.write().await;
        if cfg.users.remove(user_id).is_none() {
            return Err(Error::UserNotFound(user_id.to_string()));
        }
        // 从所有群组中移除该成员
        for g in cfg.groups.iter_mut() {
            g.members.remove(user_id);
        }
        self.save(&cfg).await?;
        drop(cfg);

        // 通知 agent 用户已删除
        if let Some(handler) = self.on_user_remove.upgrade() {
            let event = Arc::new(UserRemoveEvent {
                msg_id: self.next_msg_id(),
                notification: Arc::new(UserRemoveNotification {
                    messenger_id: self.messenger_id.clone(),
                    user_id: Arc::new(user_id.to_string()),
                }),
                time: Arc::new(Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
            });
            handler.handle_user_remove(event).await;
        }

        Ok(())
    }

    pub async fn add_group(&self, group_name: &str, member_ids: Vec<String>) -> Result<String> {
        let mut cfg = self.config.write().await;
        let n = cfg.next_group_seq;
        cfg.next_group_seq += 1;
        let group_id = format!("{}{}", GROUP_ID_PREFIX, n);
        let members = Arc::new(member_ids.clone().into_iter().collect::<DashSet<String>>());
        cfg.groups.insert(group_id.clone(), Arc::new(GroupConfig {
            group_id: Arc::new(group_id.clone()),
            group_name: Arc::new(group_name.to_string()),
            members,
        }));
        self.save(&cfg).await?;
        drop(cfg);

        // 通知新成员
        let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        for m in &member_ids {
            if m != ADMIN_USER_ID.as_str() {
                self.notify_group_change(m, &group_id, GroupChangeType::Joined, &time).await;
            }
        }

        Ok(group_id)
    }

    pub async fn rename_group(&self, group_id: &str, new_name: &str) -> Result<()> {
        if group_id.starts_with(ADMIN_USER_GROUP_PREFIX) {
            return Err(Error::GroupNotFound(group_id.to_string()));
        }
        let cfg = self.config.write().await;
        let mut g = cfg.groups.get_mut(group_id)
            .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
        let old = g.clone();
        *g = Arc::new(GroupConfig {
            group_id: old.group_id.clone(),
            group_name: Arc::new(new_name.to_string()),
            members: old.members.clone(),
        });
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn manage_members(&self, group_id: &str, add_ids: &[String], remove_ids: &[String]) -> Result<()> {
        if group_id.starts_with(ADMIN_USER_GROUP_PREFIX) {
            return Err(Error::GroupNotFound(group_id.to_string()));
        }
        {
            let cfg = self.config.write().await;
            let mut g = cfg.groups.get_mut(group_id)
                .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
            let old = g.clone();
            for id in add_ids {
                old.members.insert(id.clone());
            }
            for id in remove_ids {
                old.members.remove(id);
            }
            *g = Arc::new(GroupConfig {
                group_id: old.group_id.clone(),
                group_name: old.group_name.clone(),
                members: old.members.clone(),
            });
            self.save(&cfg).await?;
        }

        // 通知成员变更
        let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        for add_id in add_ids {
            self.notify_group_change(add_id, group_id, GroupChangeType::Joined, &time).await;
        }
        for remove_id in remove_ids {
            self.notify_group_change(remove_id, group_id, GroupChangeType::Left, &time).await;
        }

        Ok(())
    }

    pub async fn delete_group(&self, group_id: &str) -> Result<()> {
        if group_id.starts_with(ADMIN_USER_GROUP_PREFIX) {
            return Err(Error::GroupNotFound(group_id.to_string()));
        }
        let members: Vec<String> = {
            let cfg = self.config.write().await;
            let group = cfg.groups.get(group_id).map(|g| g.members.iter().map(|m| m.clone()).collect());
            if cfg.groups.remove(group_id).is_none() {
                return Err(Error::GroupNotFound(group_id.to_string()));
            }
            self.save(&cfg).await?;
            group.unwrap_or_default()
        };

        // 通知成员退出
        let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        for m in &members {
            if m.as_str() != ADMIN_USER_ID.as_str() {
                self.notify_group_change(m, group_id, GroupChangeType::Left, &time).await;
            }
        }

        Ok(())
    }

    fn get_on_group_change(&self) -> Result<Arc<dyn GroupChangeHandler>> {
        self.on_group_change.upgrade()
            .ok_or_else(|| Error::InternalError("group change handler is None".to_string()))
    }

    /// 发送消息（统一入口）。接收 OutgoingMessage 并：
    /// 1. 验证 messenger_id 匹配自己
    /// 2. 验证发送者权限
    /// 3. 确定群组成员（排除 admin），分发 IncomingMessage 给各成员
    /// 4. 推 SSE（admin 可看所有群组消息）
    /// 5. 返回 OutgoingMessageResponse
    pub async fn send(&self, outgoing: Arc<OutgoingMessage>) -> Result<Arc<OutgoingMessageResponse>> {
        // 1. 验证 messenger_id
        if outgoing.messenger_id.as_str() != self.messenger_id.as_str() {
            return Err(Error::InvalidMessage("messenger_id mismatch".to_string()));
        }

        let msg_id = self.next_msg_id();
        let time = Arc::new(Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());

        let cfg = self.config.read().await;

        // 确定群组成员（不含 admin）
        let members: Vec<Arc<String>> = if outgoing.group_id.starts_with(ADMIN_USER_GROUP_PREFIX) {
            if outgoing.user_id.as_str() == ADMIN_USER_ID.as_str() {
                // admin 对 admin-user 单聊组发消息：用 parse 验证用户存在并提取 uid
                match Self::parse_admin_user_group_ref(&cfg, outgoing.group_id.as_str()) {
                    Some(uid) => vec![Arc::new(uid)],
                    None => return Err(Error::GroupNotFound(outgoing.group_id.to_string())),
                }
            } else {
                // 普通用户对 admin-user 单聊组发消息：验证 group_id == a_{user_id} 且用户存在
                if !Self::is_admin_user_group_for(outgoing.user_id.as_str(), outgoing.group_id.as_str())
                    || !cfg.users.contains_key(outgoing.user_id.as_str())
                {
                    return Err(Error::GroupNotFound(outgoing.group_id.to_string()));
                }
                // 发送者发给自己（agent 会识别 is_self）
                vec![outgoing.user_id.clone()]
            }
        } else {
            // 普通群组：admin 和普通用户逻辑相同——检查群组存在且发送者在成员中
            let group = cfg.groups.get(outgoing.group_id.as_str())
                .ok_or_else(|| Error::GroupNotFound(outgoing.group_id.to_string()))?;
            if !group.members.contains(outgoing.user_id.as_str()) {
                return Err(Error::GroupNotFound(outgoing.group_id.to_string()));
            }
            group.members.iter().filter(|m| m.as_str() != ADMIN_USER_ID.as_str()).map(|m| Arc::new(m.clone())).collect()
        };
        drop(cfg);

        // 处理附件消息：解析 content、生成 key（在成员分发之前执行）
        let attachment_item = kissbot_channel::process_attachment_message(
            outgoing.clone(),
            &*self.attachment_store,
        ).await.map_err(|e| Error::InternalError(e.to_string()))?;
        let new_content = attachment_item.content.clone();

        let messenger_id = self.messenger_id.clone();
        for member_id in &members {
            let is_self = if member_id.as_str() == outgoing.user_id.as_str() { 1 } else { 0 };
            let incoming = Arc::new(IncomingMessage {
                msg_id: msg_id.clone(),
                messenger_id: messenger_id.clone(),
                user_id: outgoing.user_id.clone(),
                group_id: outgoing.group_id.clone(),
                is_self,
                msg_type: outgoing.msg_type.clone(),
                content: new_content.clone(),
                time: time.clone(),
            });

            if let Some(handler) = self.on_incoming_messages.upgrade() {
                handler.handle_incoming_message(incoming).await;
            }
        }

        // 推 SSE + 写入存储
        let response_content = new_content;
        let admin_msg = Arc::new(IncomingMessage {
            msg_id: msg_id.clone(),
            messenger_id: messenger_id.clone(),
            user_id: ADMIN_USER_ID.clone(),
            group_id: outgoing.group_id.clone(),
            is_self: 1,
            msg_type: outgoing.msg_type.clone(),
            content: response_content.clone(),
            time: time.clone(),
        });
        if let Ok(json) = serde_json::to_string(&admin_msg) {
            self.sse.push(&json);
        }
        self.message_store.append(admin_msg).await;

        Ok(Arc::new(OutgoingMessageResponse {
            msg_id,
            time,
            msg_type: outgoing.msg_type.clone(),
            content: response_content,
        }))
    }

    async fn notify_group_change(&self, user_id: &str, group_id: &str, change_type: GroupChangeType, time: &str) {
        let handler = match self.get_on_group_change() {
            Ok(h) => h,
            Err(_) => return,
        };
        let event = Arc::new(GroupChangeEvent {
            msg_id: self.next_msg_id(),
            notification: Arc::new(GroupChangeNotification {
                messenger_id: self.messenger_id.clone(),
                user_id: Arc::new(user_id.to_string()),
                group_id: Arc::new(group_id.to_string()),
            }),
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
                if g_ref.members.iter().any(|m| m.as_str() == user_ref.key().as_str()) {
                    ug_map.insert(g_ref.key().to_string(), Arc::new(GroupInfo {
                        group_id: g_ref.group_id.clone(),
                        group_name: g_ref.group_name.clone(),
                    }));
                }
            }

            let gid = admin_user_group_id(user_ref.key().as_str());
            if !cfg.groups.contains_key(&gid) {
                ug_map.insert(gid.clone(), Arc::new(GroupInfo {
                    group_id: Arc::new(gid),
                    group_name: user_ref.user_name.clone(),
                }));
            }

            full_user_map.insert(user_ref.key().to_string(), Arc::new(UserInfo {
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
    repo_path: PathBuf,
    config: Arc<RwLock<WebMessengerRepo>>,
    attachment_dir: String,
    message_store: Arc<MessageStore>,
}

impl WebMessengerCreator {
    pub async fn new(repo_path: &str, attachment_dir: &str) -> Result<Self> {
        let path = PathBuf::from(repo_path);
        let content = std::fs::read_to_string(&path)?;
        let config: WebMessengerRepo = serde_json::from_str(&content)?;
        let base_dir = path.parent().unwrap().join("messages");
        let message_store = MessageStore::new(base_dir);
        Ok(Self {
            repo_path: path,
            config: Arc::new(RwLock::new(config)),
            attachment_dir: attachment_dir.to_string(),
            message_store,
        })
    }

    pub async fn messenger_id(&self) -> Arc<String> {
        let config = self.config.read().await;
        config.messenger_id.clone()
    }
}

#[async_trait]
impl MessengerCreator<WebMessenger> for WebMessengerCreator {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
    ) -> std::result::Result<Arc<WebMessenger>, kissbot_channel::Error> {
        let mid = self.config.read().await.messenger_id.clone();
        let messenger = Arc::new(WebMessenger::new(
            mid,
            self.repo_path.clone(),
            self.config.clone(),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            on_user_remove,
            &self.attachment_dir,
            self.message_store.clone(),
        ));

        Ok(messenger)
    }
}

#[async_trait]
impl Messenger for WebMessenger {
    async fn get_info(&self) -> std::result::Result<Arc<MessengerInfo>, kissbot_channel::Error> {
        let info = self.build_messenger_info().await;
        Ok(Arc::new(info))
    }

    async fn send_message(&self, message: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel::Error> {
        Ok(self.send(Arc::new(message)).await?)
    }

    async fn send_attachment_payload(&self, transfer_id: u32, size: u32, pos: u64, data: Bytes) -> std::result::Result<AttachmentPayloadResponse, kissbot_channel::Error> {
        let response = self.attachment_store.write_chunk(transfer_id, pos, size, data).await?;
        Ok(response)
    }

    async fn download_attachment_header(&self, request: AttachmentDownloadRequest) -> std::result::Result<Arc<AttachmentInfoResponse>, kissbot_channel::Error> {
        let meta = self.attachment_store.get_meta(request.key.as_str())
            .map_err(|e| kissbot_channel::Error::AttachmentNotFound(e.to_string()))?;
        let info = meta.info.clone();
        // 生成 transfer_id 并存储 key 映射（下载时由 transfer_id 反查 key）
        let transfer_id = self.attachment_store.next_transfer_id_for(request.key.clone());

        Ok(Arc::new(AttachmentInfoResponse {
            key: Arc::clone(&request.key),
            info,
            transfer_id,
        }))
    }

    async fn start_send_download_attachment_payload(&self, transfer_id: u32) -> std::result::Result<(), kissbot_channel::Error> {
        let sender = self.on_download_attachment_payload.upgrade()
            .ok_or_else(|| kissbot_channel::Error::InternalError("download payload sender unavailable".to_string()))?;
        let store = self.attachment_store.clone();

        tokio::spawn(async move {
            if let Err(e) = store.send_download_payload(transfer_id, sender).await {
                tracing::error!("Failed to send download payload: {}", e);
            }
        });

        Ok(())
    }
}

