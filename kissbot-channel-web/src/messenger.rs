use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Weak};

use async_trait::async_trait;
use chrono::Utc;
use dashmap::{DashMap, DashSet};
use kissbot_api::channel::{
    AttachmentDownloadRequest, AttachmentDownloadResponseHeader, AttachmentInfo,
    GroupInfo, IncomingMessage, MessengerInfo, OutgoingMessage, OutgoingMessageResponse, UserInfo,
};
use kissbot_channel::{
    AttachmentDownloadPayloadSender, GroupChangeEvent, GroupChangeHandler, GroupChangeType,
    IncomingMessageEvent, IncomingMessageHandler,
    Messenger, MessengerCreator,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::attachment::AttachmentStore;
use crate::error::{Error, Result};

// =========== SSE 分发器（给 admin 前端推送） ===========

pub struct SseDispatcher {
    senders: DashMap<String, flume::Sender<String>>,
}

impl SseDispatcher {
    pub fn new() -> Self {
        Self { senders: DashMap::new() }
    }

    pub fn register(&self, group_id: &str) -> flume::Receiver<String> {
        let (tx, rx) = flume::unbounded();
        self.senders.insert(group_id.to_string(), tx);
        rx
    }

    pub fn push(&self, group_id: &str, data: &str) {
        if let Some(tx) = self.senders.get(group_id) {
            let _ = tx.send(data.to_string());
        }
    }
}

const ADMIN_USER_GROUP_PREFIX: &str = "a_";
pub static ADMIN_USER_ID: LazyLock<Arc<String>> = LazyLock::new(|| Arc::new("admin".to_string()));
const USER_ID_PREFIX: &str = "u";
const GROUP_ID_PREFIX: &str = "g";

// ========== JSON 配置 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerConfig {
    pub messenger_id: Arc<String>,
    pub admin_key: Arc<String>,
    pub user_key: Arc<String>,
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

// ========== SSE 消息结构（编译检查的 JSON 序列化） ==========

#[derive(Debug, Serialize)]
struct SsePayload<'a> {
    r#type: &'a str,
    data: SseMessage,
}

#[derive(Debug, Serialize)]
struct SseMessage {
    msg_id: Arc<String>,
    messenger_id: Arc<String>,
    user_id: Arc<String>,
    group_id: Arc<String>,
    is_self: usize,
    msg_type: Arc<String>,
    content: Arc<String>,
    time: Arc<String>,
}

// ========== WebMessenger ==========

pub struct WebMessenger {
    pub messenger_id: Arc<String>,
    config_path: PathBuf,
    config: Arc<RwLock<MessengerConfig>>,
    msg_id_seq: AtomicU32,
    pub(crate) on_group_change: Weak<dyn GroupChangeHandler>,
    pub(crate) on_incoming_messages: Weak<dyn IncomingMessageHandler>,
    pub(crate) on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
    pub sse: Arc<SseDispatcher>,
    pub attachment_store: Arc<AttachmentStore>,
}

impl WebMessenger {
    pub fn new(
        messenger_id: Arc<String>,
        config_path: PathBuf,
        config: Arc<RwLock<MessengerConfig>>,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        attachment_dir: &str,
    ) -> Self {
        Self {
            messenger_id,
            config_path,
            config,
            msg_id_seq: AtomicU32::new(0),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            sse: Arc::new(SseDispatcher::new()),
            attachment_store: Arc::new(AttachmentStore::new(attachment_dir)),
        }
    }

    fn next_msg_id(&self) -> Arc<String> {
        let now = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let seq = self.msg_id_seq.fetch_add(1, Ordering::SeqCst) % 1_000_000;
        Arc::new(format!("{}{:06}", now, seq))
    }

    async fn save(&self, cfg: &MessengerConfig) -> Result<()> {
        let json = serde_json::to_string_pretty(cfg)?;
        std::fs::write(&self.config_path, json)?;
        Ok(())
    }

    pub async fn admin_key(&self) -> Arc<String> {
        self.config.read().await.admin_key.clone()
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

    pub async fn get_group(&self, group_id: &str) -> Option<Arc<GroupConfig>> {
        self.config.read().await.groups.get(group_id).map(|g| g.clone())
    }

    /// group_id 是 admin-user 单聊组时返回对应的 user_id，否则 None。
    /// 验证前缀匹配、user 存在于 config。
    pub async fn parse_admin_user_group(&self, group_id: &str) -> Option<String> {
        let uid = group_id.strip_prefix(ADMIN_USER_GROUP_PREFIX)?;
        let cfg = self.config.read().await;
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
        Ok(())
    }

    pub async fn add_group(&self, group_name: &str, member_ids: Vec<String>) -> Result<String> {
        let mut cfg = self.config.write().await;
        let n = cfg.next_group_seq;
        cfg.next_group_seq += 1;
        let group_id = format!("{}{}", GROUP_ID_PREFIX, n);
        let members = Arc::new(member_ids.into_iter().collect::<DashSet<String>>());
        cfg.groups.insert(group_id.clone(), Arc::new(GroupConfig {
            group_id: Arc::new(group_id.clone()),
            group_name: Arc::new(group_name.to_string()),
            members,
        }));
        self.save(&cfg).await?;
        Ok(group_id)
    }

    pub async fn rename_group(&self, group_id: &str, new_name: &str) -> Result<()> {
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

    fn get_on_group_change(&self) -> Result<Arc<dyn GroupChangeHandler>> {
        self.on_group_change.upgrade()
            .ok_or_else(|| Error::InternalError("group change handler is None".to_string()))
    }

    /// 统一 Outgoing → Incoming 转换并分发。
    /// - 确定群组成员列表（admin-user 单聊组从 user_id 推导）
    /// - 为每个成员生成 IncomingMessage（含 msg_id / is_self）
    /// - 调 on_incoming_messages 回调（传给 Agent）
    /// - 推 SSE（admin 可看所有群组消息）
    async fn outgoing_to_incoming(&self, outgoing: OutgoingMessage) -> Result<Arc<String>> {
        let msg_id = self.next_msg_id();
        let messenger_id = self.messenger_id.clone();
        let time = Arc::new(Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());

        let cfg = self.config.read().await;
        let members: Vec<Arc<String>> = if let Some(uid) = self.parse_admin_user_group(outgoing.group_id.as_str()).await {
            vec![Arc::new(uid)]
        } else {
            let group = cfg.groups.get(outgoing.group_id.as_str())
                .map(|g| g.clone())
                .ok_or_else(|| Error::GroupNotFound(outgoing.group_id.to_string()))?;
            group.members.iter().filter(|m| m.as_str() != ADMIN_USER_ID.as_str()).map(|m| Arc::new(m.clone())).collect()
        };
        drop(cfg);

        if members.is_empty() {
            return Err(Error::GroupNotFound(outgoing.group_id.to_string()));
        }

        for member_id in &members {
            let is_self = if member_id.as_str() == outgoing.user_id.as_str() { 1 } else { 0 };
            let incoming = Arc::new(IncomingMessage {
                msg_id: msg_id.clone(),
                messenger_id: messenger_id.clone(),
                user_id: outgoing.user_id.clone(),
                group_id: outgoing.group_id.clone(),
                is_self,
                msg_type: outgoing.msg_type.clone(),
                content: outgoing.content.clone(),
                time: time.clone(),
            });
            let event = Arc::new(IncomingMessageEvent {
                messenger_id: messenger_id.clone(),
                user_id: outgoing.user_id.clone(),
                group_id: outgoing.group_id.clone(),
                messages: Arc::new(vec![incoming]),
            });

            if let Some(handler) = self.on_incoming_messages.upgrade() {
                handler.handle_incoming_message(event).await;
            }
        }

        // 推 SSE
        let group_id = outgoing.group_id.clone();
        let sse_event = SseMessage {
            msg_id: msg_id.clone(),
            messenger_id,
            user_id: outgoing.user_id,
            group_id: outgoing.group_id,
            is_self: 1,
            msg_type: outgoing.msg_type,
            content: outgoing.content,
            time,
        };
        let sse_payload = SsePayload { r#type: "message", data: sse_event };
        if let Ok(json) = serde_json::to_string(&sse_payload) {
            self.sse.push(group_id.as_str(), &json);
        }

        Ok(msg_id)
    }

    /// admin 发消息。admin 不是群组成员则拒绝。
    /// admin-user 单聊组（a_{user_id}）不在 groups 配置中，直接允许。
    pub async fn send_from_admin(&self, group_id: &str, content: &str, msg_type: &str) -> Result<Arc<String>> {
        let cfg = self.config.read().await;
        if self.parse_admin_user_group(group_id).await.is_none() {
            let group = cfg.groups.get(group_id)
                .map(|g| g.clone())
                .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
            if !group.members.contains(ADMIN_USER_ID.as_str()) {
                return Err(Error::GroupNotFound(group_id.to_string()));
            }
        }
        drop(cfg);

        let outgoing = OutgoingMessage {
            messenger_id: self.messenger_id.clone(),
            user_id: ADMIN_USER_ID.clone(),
            group_id: Arc::new(group_id.to_string()),
            msg_type: Arc::new(msg_type.to_string()),
            content: Arc::new(content.to_string()),
            attachment_map: Arc::new(DashMap::new()),
        };
        self.outgoing_to_incoming(outgoing).await
    }

    /// admin 发消息，带附件。
    pub async fn send_from_admin_with_attachment(
        &self,
        group_id: &str,
        content: &str,
        msg_type: &str,
        attachment_map: Arc<DashMap<String, Arc<AttachmentInfo>>>,
    ) -> Result<Arc<String>> {
        let cfg = self.config.read().await;
        if self.parse_admin_user_group(group_id).await.is_none() {
            let group = cfg.groups.get(group_id)
                .map(|g| g.clone())
                .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
            if !group.members.contains(ADMIN_USER_ID.as_str()) {
                return Err(Error::GroupNotFound(group_id.to_string()));
            }
        }
        drop(cfg);

        let outgoing = OutgoingMessage {
            messenger_id: self.messenger_id.clone(),
            user_id: ADMIN_USER_ID.clone(),
            group_id: Arc::new(group_id.to_string()),
            msg_type: Arc::new(msg_type.to_string()),
            content: Arc::new(content.to_string()),
            attachment_map,
        };
        self.outgoing_to_incoming(outgoing).await
    }

    pub async fn notify_group_change(&self, user_id: &str, group_id: &str, change_type: GroupChangeType, time: &str) {
        let handler = match self.get_on_group_change() {
            Ok(h) => h,
            Err(_) => return,
        };
        let event = Arc::new(GroupChangeEvent {
            msg_id: self.next_msg_id(),
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
    config_path: PathBuf,
    config: Arc<RwLock<MessengerConfig>>,
    attachment_dir: String,
}

impl WebMessengerCreator {
    pub async fn new(config_path: &str, attachment_dir: &str) -> Result<Self> {
        let path = PathBuf::from(config_path);
        let content = std::fs::read_to_string(&path)?;
        let config: MessengerConfig = serde_json::from_str(&content)?;
        Ok(Self {
            config_path: path,
            config: Arc::new(RwLock::new(config)),
            attachment_dir: attachment_dir.to_string(),
        })
    }

    pub async fn user_key(&self) -> Arc<String> {
        let config = self.config.read().await;
        config.user_key.clone()
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
    ) -> std::result::Result<Arc<WebMessenger>, kissbot_channel::Error> {
        let mid = self.config.read().await.messenger_id.clone();
        let messenger = Arc::new(WebMessenger::new(
            mid,
            self.config_path.clone(),
            self.config.clone(),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            &self.attachment_dir,
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

    async fn send_message(&self, message: OutgoingMessage, _attachment_sn: Arc<AtomicU32>) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel::Error> {
        let msg_id = self.outgoing_to_incoming(message).await?;

        let upload_id_map: Arc<DashMap<String, u32>> = Arc::new(DashMap::new());
        Ok(Arc::new(OutgoingMessageResponse {
            msg_id: msg_id,
            attachment_upload_id_map: upload_id_map,
        }))
    }

    async fn send_attachment_payload(&self, _id: u32, _size: u32, _pos: u64, _data: &[u8]) -> std::result::Result<(), kissbot_channel::Error> {
        Ok(())
    }

    async fn download_attachment_header(&self, request: AttachmentDownloadRequest, _attachment_sn: Arc<AtomicU32>) -> std::result::Result<Arc<AttachmentDownloadResponseHeader>, kissbot_channel::Error> {
        let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;

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
