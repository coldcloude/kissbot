use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;
use flume::{Receiver, unbounded};
use kai_file::{FileObjectAppender, NoopErrorHandler};
use kissbot_api::ArcSwapHashMap;
use uuid::Uuid;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use kissbot_api::channel::{
    AttachmentDownloadRequest, AttachmentPayloadResponse, GroupInfo, IncomingMessage, MessengerInfo, OutgoingMessage, OutgoingMessageResponse,
    UserInfo,
};
use kissbot_api::message::{AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel::{
    ChannelManager, GroupChangeEvent, GroupChangeType, UserRemoveEvent,
    IncomingMessageEvent,
    Messenger, MessengerCreator,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use arc_swap::ArcSwap;
use crate::attachment::AttachmentStore;
use crate::error::{Error, Result};
use crate::message_store::{MessageFileWriterContext, MessageStore, MsgKey};
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
#[derive(Debug, Serialize, Deserialize)]
pub struct WebMessengerRepo {
    pub messenger_id: Arc<String>,
    pub admin_name: Arc<String>,
    pub users: Arc<ArcSwapHashMap<String, UserConfig>>,
    pub groups: Arc<ArcSwapHashMap<String, GroupConfig>>,
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
    pub members: Arc<HashSet<String>>,
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
    manager: Weak<ChannelManager>,
    pub sse: Arc<SseDispatcher>,
    pub attachment_store: Arc<AttachmentStore>,
    pub message_store: Arc<MessageStore>,
    appender: FileObjectAppender<MsgKey, IncomingMessage, MessageStore, MessageFileWriterContext>,
    stored_receiver: Receiver<Vec<IncomingMessage>>,
}
impl WebMessenger {
    pub fn new(
        messenger_id: Arc<String>,
        repo_path: PathBuf,
        config: Arc<RwLock<WebMessengerRepo>>,
        manager: Weak<ChannelManager>,
        attachment_dir: &str,
        message_base_dir: &str,
    ) -> Arc<Self> {
        let sse = Arc::new(SseDispatcher::new());
        let attachment_store = Arc::new(AttachmentStore::new(attachment_dir));
        let (tx, rx) = unbounded();
        let message_store = Arc::new(MessageStore::new(message_base_dir, tx));
        let appender = FileObjectAppender::new(
            message_store.clone(),
            Arc::new(NoopErrorHandler),
            Duration::from_secs(3),
            100,
        );
        let messenger = Arc::new(Self {
            messenger_id,
            repo_path,
            config,
            msg_id_seq: AtomicU32::new(0),
            manager,
            sse,
            attachment_store,
            message_store,
            appender,
            stored_receiver: rx
        });
        let msgr = messenger.clone();
        tokio::spawn(async move {
            while let Ok(msgs) = msgr.stored_receiver.recv_async().await {
                msgr.send_stored(msgs).await;
            }
        });
        messenger
    }
    pub fn next_msg_id(&self) -> Arc<String> {
        let now = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let seq = self.msg_id_seq.fetch_add(1, Ordering::SeqCst) % 1_000_000;
        Arc::new(format!("{}{:06}", now, seq))
    }
    /// 写配置：获取写锁 → to_mut() 克隆获得 &mut WebMessengerRepo → op 直接修改 → 序列化写文件
    async fn write_config<F, R>(&self, op: F) -> Result<R>
    where
        F: FnOnce(&mut WebMessengerRepo) -> Result<R>,
    {
        let mut guard = self.config.write().await;
        let rst = op(&mut *guard)?;
        let json = serde_json::to_string_pretty(&*guard)?;
        tokio::fs::write(&self.repo_path, json.as_bytes()).await?;
        Ok(rst)
    }
    pub async fn admin_name(&self) -> Arc<String> {
        self.config.read().await.admin_name.clone()
    }
    pub async fn config_users(&self) -> HashMap<String, Arc<UserConfig>> {
        self.config.read().await.users.as_ref().iter().map(|(k, v)| {
            (k.clone(), v.load().clone())
        }).collect()
    }
    pub async fn config_groups(&self) -> HashMap<String, Arc<GroupConfig>> {
        self.config.read().await.groups.as_ref().iter().map(|(k, v)| {
            (k.clone(), v.load().clone())
        }).collect()
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
        self.write_config(|repo| {
            repo.admin_name = Arc::new(new_name.to_string());
            Ok(())
        }).await
    }
    pub async fn rename_user(&self, user_id: &str, new_name: &str) -> Result<()> {
        self.write_config(|repo| {
            let user_swap = repo.users.get(user_id)
                .ok_or_else(|| Error::UserNotFound(user_id.to_string()))?;
            let mut user_arc = user_swap.load().clone();
            Arc::make_mut(&mut user_arc).user_name = Arc::new(new_name.to_string());
            user_swap.store(user_arc);
            Ok(())
        }).await
    }
    pub async fn add_user(&self, user_name: &str) -> Result<Arc<String>> {
        let user_id = self.write_config(|repo| {
            let n = repo.next_user_seq;
            repo.next_user_seq += 1;
            let user_id = Arc::new(format!("{}{}", USER_ID_PREFIX, n));
            let users = Arc::make_mut(&mut repo.users);
            users.insert(user_id.as_str().to_string(), ArcSwap::new(Arc::new(UserConfig {
                user_id: user_id.clone(),
                user_name: Arc::new(user_name.to_string()),
            })));
            Ok(user_id)
        }).await?;
        Ok(user_id)
    }
    pub async fn remove_user(&self, user_id: &str) -> Result<()> {
        self.write_config(|repo| {
            if repo.users.contains_key(user_id) {
                let users = Arc::make_mut(&mut repo.users);
                users.remove(user_id);
                // 从所有群组中移除该成员
                for group_swap in repo.groups.values() {
                    let group = group_swap.load();
                    if group.members.contains(user_id) {
                        let mut group_new_arc = group.clone();
                        let group_new = Arc::make_mut(&mut group_new_arc);
                        let members = Arc::make_mut(&mut group_new.members);
                        members.remove(user_id);
                        group_swap.store(group_new_arc);
                    }
                }
                Ok(())
            } else {
                Err(Error::UserNotFound(user_id.to_string()))
            }
        }).await?;
        // 通知 agent 用户已删除
        if let Some(manager) = self.manager.upgrade() {
            let event = Arc::new(UserRemoveEvent {
                msg_id: self.next_msg_id(),
                notification: Arc::new(UserRemoveNotification {
                    messenger_id: self.messenger_id.clone(),
                    user_id: Arc::new(user_id.to_string()),
                }),
                time: Arc::new(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()),
            });
            manager.handle_user_remove(event).await;
        }
        Ok(())
    }
    pub async fn add_group(&self, group_name: &str, member_ids: Vec<String>) -> Result<Arc<String>> {
        let group_id = self.write_config(|repo| {
            let n = repo.next_group_seq;
            repo.next_group_seq += 1;
            let group_id = Arc::new(format!("{}{}", GROUP_ID_PREFIX, n));
            let groups = Arc::make_mut(&mut repo.groups);
            groups.insert(group_id.as_str().to_string(), ArcSwap::new(Arc::new(GroupConfig {
                group_id: group_id.clone(),
                group_name: Arc::new(group_name.to_string()),
                members: Arc::new(member_ids.iter().cloned().collect()),
            })));
            Ok(group_id)
        }).await?;
        // 通知新成员
        let time = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
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
        self.write_config(|repo| {
            let group_swap = repo.groups.get(group_id)
                .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
            let mut g_arc = group_swap.load().clone();
            Arc::make_mut(&mut g_arc).group_name = Arc::new(new_name.to_string());
            group_swap.store(g_arc);
            Ok(())
        }).await
    }
    pub async fn manage_members(&self, group_id: &str, add_ids: &[String], remove_ids: &[String]) -> Result<()> {
        if group_id.starts_with(ADMIN_USER_GROUP_PREFIX) {
            return Err(Error::GroupNotFound(group_id.to_string()));
        }
        self.write_config(|repo| {
            let group_swap = repo.groups.get(group_id)
                .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
            let mut group_new_arc = group_swap.load_full();
            let group_new = Arc::make_mut(&mut group_new_arc);
            let members = Arc::make_mut(&mut group_new.members);
            for id in add_ids {
                members.insert(id.clone());
            }
            for id in remove_ids {
                members.remove(id);
            }
            group_swap.store(group_new_arc);
            Ok(())
        }).await?;
        // 通知成员变更
        let time = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
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
        let members = self.write_config(|repo| {
            if repo.groups.contains_key(group_id) {
                let groups = Arc::make_mut(&mut repo.groups);
                let group = groups.remove(group_id).ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
                Ok(group.load().members.clone())
            } else {
                Err(Error::GroupNotFound(group_id.to_string()))
            }
        }).await?;
        // 通知成员退出
        let time = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        for m in members.iter() {
            if m.as_str() != ADMIN_USER_ID.as_str() {
                self.notify_group_change(m, group_id, GroupChangeType::Left, &time).await;
            }
        }
        Ok(())
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
        let cfg = self.config.read().await;
        // 判断用户和群组信息是否正确
        if outgoing.group_id.starts_with(ADMIN_USER_GROUP_PREFIX) {
            // 处理 admin-user 单聊组
            if outgoing.user_id.as_str() == ADMIN_USER_ID.as_str() {
                if Self::parse_admin_user_group_ref(&cfg, outgoing.group_id.as_str()).is_none() {
                    return Err(Error::GroupNotFound(outgoing.group_id.to_string()));
                }
            } else {
                // 普通用户对 admin-user 单聊组发消息：验证 group_id == a_{user_id} 且用户存在
                if !Self::is_admin_user_group_for(outgoing.user_id.as_str(), outgoing.group_id.as_str())
                    || !cfg.users.contains_key(outgoing.user_id.as_str())
                {
                    return Err(Error::GroupNotFound(outgoing.group_id.to_string()));
                }
            }
        } else {
            // 普通群组：admin 和普通用户逻辑相同——检查群组存在且发送者在成员中
            let group_swap = cfg.groups.get(outgoing.group_id.as_str())
                .ok_or_else(|| Error::GroupNotFound(outgoing.group_id.to_string()))?;
            let group = group_swap.load();
            if !group.members.contains(outgoing.user_id.as_str()) {
                return Err(Error::GroupNotFound(outgoing.group_id.to_string()));
            }
        }
        drop(cfg);
        // 处理附件消息：解析 content、生成 key（在成员分发之前执行）
        let new_content = kissbot_channel::process_attachment_message(
            outgoing.clone(),
            self.attachment_store.as_ref(),
        ).await.map_err(|e| Error::InternalError(e.to_string()))?;
        // 生成消息ID和时间戳
        let msg_id = self.next_msg_id();
        let time = Arc::new(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
        // 写入存储
        let is_admin = if outgoing.user_id.as_str() == ADMIN_USER_ID.as_str() { 1 } else { 0 };
        let admin_msg = IncomingMessage {
            msg_id: msg_id.clone(),
            messenger_id: self.messenger_id.clone(),
            user_id: outgoing.user_id.clone(),
            group_id: outgoing.group_id.clone(),
            is_self: is_admin,
            content: new_content.clone(),
            time: time.clone(),
        };
        let date = kai_date::as_date(&time).to_string();
        let key = MsgKey {
            group_id: admin_msg.group_id.to_string(),
            date,
        };
        self.appender.append(key, vec![admin_msg]).await;
        // 发送完成即返回，写入后再推送
        Ok(Arc::new(OutgoingMessageResponse {
            msg_id,
            time,
            content: new_content,
        }))
    }
    pub async fn send_stored(&self, msgs: Vec<IncomingMessage>) {
        // 推 SSE + 
        for admin_msg in msgs.iter() {
            if let Ok(json) = serde_json::to_string(&admin_msg) {
                self.sse.push(&json);
            }
        }
        // 根据群组成员计算要推送的消息
        let mut messages = Vec::new();
        let cfg = self.config.read().await;
        for admin_msg in msgs.iter() {
            let members: Vec<Arc<String>> = if admin_msg.group_id.starts_with(ADMIN_USER_GROUP_PREFIX) {
                if admin_msg.user_id.as_str() == ADMIN_USER_ID.as_str() {
                    // admin 对 admin-user 单聊组发消息：用 parse 验证用户存在并提取 uid
                    match Self::parse_admin_user_group_ref(&cfg, admin_msg.group_id.as_str()) {
                        Some(uid) => vec![Arc::new(uid)],
                        None => vec![],
                    }
                } else {
                    // 普通用户对 admin-user 单聊组发消息：验证 group_id == a_{user_id} 且用户存在
                    if Self::is_admin_user_group_for(admin_msg.user_id.as_str(), admin_msg.group_id.as_str())
                        && cfg.users.contains_key(admin_msg.user_id.as_str())
                    {
                        // 发送者发给自己（agent 会识别 is_self）
                        vec![admin_msg.user_id.clone()]
                    } else {
                        vec![]
                    }
                }
            } else {
                // 普通群组：admin 和普通用户逻辑相同——检查群组存在且发送者在成员中
                if let Some(group_swap) = cfg.groups.get(admin_msg.group_id.as_str()) {
                    let group = group_swap.load();
                    if group.members.contains(admin_msg.user_id.as_str()) {
                        // admin不推送
                        group.members.iter()
                        .filter(|m| m.as_str() != ADMIN_USER_ID.as_str())
                        .map(|m| Arc::new(m.clone()))
                        .collect()
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                }
            };
            if members.is_empty() {
                continue;
            }
            for member_id in &members {
                let is_self = if member_id.as_str() == admin_msg.user_id.as_str() { 1 } else { 0 };
                let incoming = Arc::new(IncomingMessage {
                    msg_id: admin_msg.msg_id.clone(),
                    messenger_id: admin_msg.messenger_id.clone(),
                    user_id: admin_msg.user_id.clone(),
                    group_id: admin_msg.group_id.clone(),
                    is_self,
                    content: admin_msg.content.clone(),
                    time: admin_msg.time.clone(),
                });
                messages.push(Arc::new(IncomingMessageEvent {
                    recipient_user_id: member_id.clone(),
                    incoming_message: incoming,
                }));
            }
        }
        drop(cfg);
        if let Some(manager) = self.manager.upgrade() {
            for message in messages {
                manager.handle_incoming_message(message).await;
            }
        }
    }
    async fn notify_group_change(&self, user_id: &str, group_id: &str, change_type: GroupChangeType, time: &str) {
        let Some(manager) = self.manager.upgrade() else { return; };
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
        manager.handle_group_change(event).await;
    }
    pub async fn build_messenger_info(&self) -> MessengerInfo {
        let cfg = self.config.read().await;
        let full_user_map = dashmap::DashMap::new();
        for (uid, user_swap) in cfg.users.iter() {
            let user = user_swap.load();
            let ug_map = dashmap::DashMap::new();
            for (gid, group_swap) in cfg.groups.iter() {
                let group = group_swap.load();
                if group.members.contains(uid) {
                    ug_map.insert(gid.clone(), Arc::new(GroupInfo {
                        group_id: Arc::new(gid.clone()),
                        group_name: group.group_name.clone(),
                    }));
                }
            }
            let gid = admin_user_group_id(uid);
            if !cfg.groups.contains_key(&gid) {
                ug_map.insert(gid.clone(), Arc::new(GroupInfo {
                    group_id: Arc::new(gid),
                    group_name: user.user_name.clone(),
                }));
            }
            full_user_map.insert(uid.clone(), Arc::new(UserInfo {
                user_id: user.user_id.clone(),
                user_name: user.user_name.clone(),
                group_map: Arc::new(ug_map),
            }));
        }
        MessengerInfo {
            messenger_id: self.messenger_id.clone(),
            messenger_name: Arc::new("Web Chat".to_string()),
            user_map: Arc::new(full_user_map),
        }
    }
}
// ========== Creator 与 Messenger trait 实现 ==========
/// 持有完整配置和路径，create() 时用预读的配置构造 WebMessenger。
pub struct WebMessengerCreator {
    repo_path: PathBuf,
    config: Arc<RwLock<WebMessengerRepo>>,
    attachment_dir: String,
    message_dir: String,
}
impl WebMessengerCreator {
    pub async fn new(repo_path: &str, attachment_dir: &str, message_dir: &str,
                     messenger_id: &str, admin_name: &str) -> Result<Self> {
        let path = PathBuf::from(repo_path);
        let config = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            serde_json::from_str(&content)?
        } else {
            // repo 文件不存在时根据 config 创建初始结构
            WebMessengerRepo {
                messenger_id: Arc::new(messenger_id.to_string()),
                admin_name: Arc::new(admin_name.to_string()),
                users: Arc::new(ArcSwapHashMap::new()),
                groups: Arc::new(ArcSwapHashMap::new()),
                next_user_seq: 0,
                next_group_seq: 0,
            }
        };
        Ok(Self {
            repo_path: path,
            config: Arc::new(RwLock::new(config)),
            attachment_dir: attachment_dir.to_string(),
            message_dir: message_dir.to_string(),
        })
    }
    pub async fn messenger_id(&self) -> Arc<String> {
        self.config.read().await.messenger_id.clone()
    }
}
#[async_trait]
impl MessengerCreator<WebMessenger> for WebMessengerCreator {
    async fn create(
        &self,
        manager: Weak<ChannelManager>,
    ) -> std::result::Result<Arc<WebMessenger>, kissbot_channel::Error> {
        let mid = self.config.read().await.messenger_id.clone();

        let messenger = WebMessenger::new(
            mid,
            self.repo_path.clone(),
            self.config.clone(),
            manager,
            &self.attachment_dir,
            &self.message_dir,
        );
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
        use kissbot_api::channel::OFFSET_ATT_DATA;
        const CHUNK_SIZE: u64 = 65536;
        let manager = self.manager.upgrade()
            .ok_or_else(|| kissbot_channel::Error::InternalError("manager unavailable".to_string()))?;
        let store = self.attachment_store.clone();
        // 获取附件 key
        let key = store.get_transfer_key(transfer_id)
            .ok_or_else(|| kissbot_channel::Error::AttachmentNotFound(transfer_id.to_string()))?;
        let meta = store.get_meta(key.as_str())
            .map_err(|e| kissbot_channel::Error::AttachmentNotFound(e.to_string()))?;
        let file_len = meta.info.size_bytes;
        tokio::spawn(async move {
            let mut pos = 0u64;
            let mut ok = true;
            while pos < file_len && ok {
                let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
                let chunk_size = (end - pos) as u32;
                let (sn, mut buf) = match manager.prepare_download_payload(transfer_id, chunk_size, pos) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("Failed to prepare download payload: {}", e);
                        break;
                    }
                };
                // 读取文件数据
                let data = match store.read_attachment_range(key.as_str(), pos, chunk_size as u64) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("Failed to read attachment range: {}", e);
                        break;
                    }
                };
                // 扩展 buffer 并填充 payload 数据
                buf.resize(OFFSET_ATT_DATA + chunk_size as usize, 0);
                buf[OFFSET_ATT_DATA..OFFSET_ATT_DATA + chunk_size as usize].copy_from_slice(&data);
                let response = manager.send_download_payload(sn, transfer_id, chunk_size, pos, buf).await;
                match response {
                    Ok(resp) => {
                        ok = resp.error_code == 0;
                    }
                    Err(e) => {
                        tracing::error!("Failed to send download payload: {}", e);
                        break;
                    }
                }
                pos = end;
            }
            // 下载完成，清理 transfer_key_map
            store.remove_transfer_key(transfer_id);
        });
        Ok(())
    }
}
