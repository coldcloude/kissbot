use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;
use arcstr::ArcStr;
use flume::{Receiver, unbounded};
use kai_file::{FileObjectAppender, NoopErrorHandler};
use kissbot_api::ArcSwapHashMap;
use uuid::Uuid;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use kissbot_api::channel::{
    AttachmentDownloadRequest, AttachmentPayloadResponse, GroupInfo, IncomingMessage, IncomingMessageEvent, MessengerInfo, OutgoingMessage, OutgoingMessageResponse,
    UserInfo,
};
use kissbot_api::message::{AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel::{
    ChannelServer, GroupChangeEvent, GroupChangeType, UserRemoveEvent,
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
    senders: Arc<Mutex<HashMap<Uuid, flume::Sender<ArcStr>>>>,
}
impl SseDispatcher {
    pub fn new() -> Self {
        Self { senders: Arc::new(Mutex::new(HashMap::new())) }
    }
    pub fn register(&self) -> flume::Receiver<ArcStr> {
        let (tx, rx) = flume::unbounded();
        let id = Uuid::new_v4();
        self.senders.lock().unwrap().insert(id, tx);
        rx
    }
    pub fn push(&self, data: &str) {
        let mut senders = self.senders.lock().unwrap();
        senders.retain(|_, tx| tx.try_send(ArcStr::from(data)).is_ok());
    }
}
const ADMIN_USER_GROUP_PREFIX: ArcStr = arcstr::literal!("a_");
const WEB_MESSENGER_NAME: ArcStr = arcstr::literal!("Web Chat");
pub static ADMIN_USER_ID: LazyLock<ArcStr> = LazyLock::new(|| ArcStr::from("admin"));
const USER_ID_PREFIX: ArcStr = arcstr::literal!("u");
const GROUP_ID_PREFIX: ArcStr = arcstr::literal!("g");
// ========== JSON 配置 ==========
#[derive(Debug, Serialize, Deserialize)]
pub struct WebMessengerRepo {
    pub messenger_id: ArcStr,
    pub admin_name: ArcStr,
    pub users: Arc<ArcSwapHashMap<ArcStr, UserConfig>>,
    pub groups: Arc<ArcSwapHashMap<ArcStr, GroupConfig>>,
    pub next_user_seq: u32,
    pub next_group_seq: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    pub user_id: ArcStr,
    pub user_name: ArcStr,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    pub group_id: ArcStr,
    pub group_name: ArcStr,
    pub members: Arc<HashSet<ArcStr>>,
}
pub fn admin_user_group_id(user_id: &str) -> ArcStr {
    ArcStr::from(format!("{}{}", ADMIN_USER_GROUP_PREFIX, user_id))
}
// ========== WebMessenger ==========
pub struct WebMessenger {
    pub messenger_id: ArcStr,
    repo_path: PathBuf,
    config: Arc<RwLock<WebMessengerRepo>>,
    msg_id_seq: AtomicU32,
    manager: Weak<ChannelServer>,
    pub sse: Arc<SseDispatcher>,
    pub attachment_store: Arc<AttachmentStore>,
    pub message_store: Arc<MessageStore>,
    appender: FileObjectAppender<MsgKey, IncomingMessage, MessageStore, MessageFileWriterContext>,
    stored_receiver: Receiver<Vec<IncomingMessage>>,
}
impl WebMessenger {
    pub fn new(
        messenger_id: ArcStr,
        repo_path: PathBuf,
        config: Arc<RwLock<WebMessengerRepo>>,
        manager: Weak<ChannelServer>,
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
            Duration::from_secs(1),
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
    pub fn next_msg_id(&self) -> ArcStr {
        let now = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let seq = self.msg_id_seq.fetch_add(1, Ordering::SeqCst) % 1_000_000;
        ArcStr::from(format!("{}{:06}", now, seq))
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
    pub async fn admin_name(&self) -> ArcStr {
        self.config.read().await.admin_name.clone()
    }
    pub async fn config_users(&self) -> HashMap<ArcStr, Arc<UserConfig>> {
        self.config.read().await.users.as_ref().iter().map(|(k, v)| {
            (k.clone(), v.load().clone())
        }).collect()
    }
    pub async fn config_groups(&self) -> HashMap<ArcStr, Arc<GroupConfig>> {
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
    fn parse_admin_user_group_ref(cfg: &WebMessengerRepo, group_id: &str) -> Option<ArcStr> {
        let uid = group_id.strip_prefix(ADMIN_USER_GROUP_PREFIX.as_str())?;
        if cfg.users.contains_key(uid) {
            Some(ArcStr::from(uid))
        } else {
            None
        }
    }

    /// 按 user_id/group_id 解析 (messenger_name, user_name, group_name)
    fn resolve_names(&self, cfg: &WebMessengerRepo, user_id: &str, group_id: &str) -> (ArcStr, ArcStr, ArcStr) {
        let messenger_name = WEB_MESSENGER_NAME.clone();
        let user_name = if user_id == ADMIN_USER_ID.as_str() {
            // admin（agent 绑定身份）的展示名取 repo.admin_name
            cfg.admin_name.clone()
        } else {
            cfg.users.get(user_id)
                .map(|s| s.load().user_name.clone())
                .unwrap_or_else(|| ArcStr::new())
        };
        let group_name = if group_id.starts_with(ADMIN_USER_GROUP_PREFIX.as_str()) {
            // admin-user 单聊组：group_name = 对端 user 的 user_name
            match Self::parse_admin_user_group_ref(cfg, group_id) {
                Some(uid) => cfg.users.get(uid.as_str())
                    .map(|s| s.load().user_name.clone())
                    .unwrap_or_else(|| ArcStr::new()),
                None => ArcStr::new(),
            }
        } else {
            cfg.groups.get(group_id)
                .map(|s| s.load().group_name.clone())
                .unwrap_or_else(|| ArcStr::new())
        };
        (messenger_name, user_name, group_name)
    }
    pub async fn update_admin_name(&self, new_name: &str) -> Result<()> {
        self.write_config(|repo| {
            repo.admin_name = ArcStr::from(new_name);
            Ok(())
        }).await
    }
    pub async fn rename_user(&self, user_id: &str, new_name: &str) -> Result<()> {
        self.write_config(|repo| {
            let user_swap = repo.users.get(user_id)
                .ok_or_else(|| Error::UserNotFound(ArcStr::from(user_id)))?;
            let mut user_arc = user_swap.load().clone();
            Arc::make_mut(&mut user_arc).user_name = ArcStr::from(new_name);
            user_swap.store(user_arc);
            Ok(())
        }).await
    }
    pub async fn add_user(&self, user_name: &str) -> Result<ArcStr> {
        let user_id = self.write_config(|repo| {
            let n = repo.next_user_seq;
            repo.next_user_seq += 1;
            let user_id = ArcStr::from(format!("{}{}", USER_ID_PREFIX, n));
            let users = Arc::make_mut(&mut repo.users);
            users.insert(user_id.clone(), ArcSwap::new(Arc::new(UserConfig {
                user_id: user_id.clone(),
                user_name: ArcStr::from(user_name),
            })));
            Ok(user_id)
        }).await?;
        Ok(user_id)
    }
    pub async fn remove_user(&self, user_id: &str) -> Result<()> {
        let user_name = self.write_config(|repo| {
            if repo.users.contains_key(user_id) {
                // 移除前先取 user_name（移除后即丢失）
                let user_name = repo.users.get(user_id)
                    .map(|s| s.load().user_name.clone())
                    .unwrap_or_else(|| ArcStr::new());
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
                Ok(user_name)
            } else {
                Err(Error::UserNotFound(ArcStr::from(user_id)))
            }
        }).await?;
        // 通知 agent 用户已删除
        if let Some(manager) = self.manager.upgrade() {
            let event = Arc::new(UserRemoveEvent {
                msg_id: self.next_msg_id(),
                notification: Arc::new(UserRemoveNotification {
                    messenger_id: self.messenger_id.clone(),
                    user_id: ArcStr::from(user_id),
                    messenger_name: WEB_MESSENGER_NAME.clone(),
                    user_name,
                }),
                time: ArcStr::from(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()),
            });
            manager.handle_user_remove(event).await;
        }
        Ok(())
    }
    pub async fn add_group(&self, group_name: &str, member_ids: Vec<ArcStr>) -> Result<ArcStr> {
        let group_id = self.write_config(|repo| {
            let n = repo.next_group_seq;
            repo.next_group_seq += 1;
            let group_id = ArcStr::from(format!("{}{}", GROUP_ID_PREFIX, n));
            let groups = Arc::make_mut(&mut repo.groups);
            groups.insert(group_id.clone(), ArcSwap::new(Arc::new(GroupConfig {
                group_id: group_id.clone(),
                group_name: ArcStr::from(group_name),
                members: Arc::new(member_ids.iter().cloned().collect()),
            })));
            Ok(group_id)
        }).await?;
        // 通知新成员
        let time = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        for m in &member_ids {
            if m.as_str() != ADMIN_USER_ID.as_str() {
                self.notify_group_change(m, &group_id, GroupChangeType::Joined, &time).await;
            }
        }
        Ok(group_id)
    }
    pub async fn rename_group(&self, group_id: &str, new_name: &str) -> Result<()> {
        if group_id.starts_with(ADMIN_USER_GROUP_PREFIX.as_str()) {
            return Err(Error::GroupNotFound(ArcStr::from(group_id)));
        }
        self.write_config(|repo| {
            let group_swap = repo.groups.get(group_id)
                .ok_or_else(|| Error::GroupNotFound(ArcStr::from(group_id)))?;
            let mut g_arc = group_swap.load().clone();
            Arc::make_mut(&mut g_arc).group_name = ArcStr::from(new_name);
            group_swap.store(g_arc);
            Ok(())
        }).await
    }
    pub async fn manage_members(&self, group_id: &str, add_ids: &[ArcStr], remove_ids: &[ArcStr]) -> Result<()> {
        if group_id.starts_with(ADMIN_USER_GROUP_PREFIX.as_str()) {
            return Err(Error::GroupNotFound(ArcStr::from(group_id)));
        }
        self.write_config(|repo| {
            let group_swap = repo.groups.get(group_id)
                .ok_or_else(|| Error::GroupNotFound(ArcStr::from(group_id)))?;
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
        if group_id.starts_with(ADMIN_USER_GROUP_PREFIX.as_str()) {
            return Err(Error::GroupNotFound(ArcStr::from(group_id)));
        }
        let members = self.write_config(|repo| {
            if repo.groups.contains_key(group_id) {
                let groups = Arc::make_mut(&mut repo.groups);
                let group = groups.remove(group_id).ok_or_else(|| Error::GroupNotFound(ArcStr::from(group_id)))?;
                Ok(group.load().members.clone())
            } else {
                Err(Error::GroupNotFound(ArcStr::from(group_id)))
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
            return Err(Error::InvalidMessage(ArcStr::from("messenger_id mismatch")));
        }
        let cfg = self.config.read().await;
        // 判断用户和群组信息是否正确
        if outgoing.group_id.starts_with(ADMIN_USER_GROUP_PREFIX.as_str()) {
            // 处理 admin-user 单聊组
            if outgoing.user_id.as_str() == ADMIN_USER_ID.as_str() {
                if Self::parse_admin_user_group_ref(&cfg, outgoing.group_id.as_str()).is_none() {
                    return Err(Error::GroupNotFound(outgoing.group_id.clone()));
                }
            } else {
                // 普通用户对 admin-user 单聊组发消息：验证 group_id == a_{user_id} 且用户存在
                if !Self::is_admin_user_group_for(outgoing.user_id.as_str(), outgoing.group_id.as_str())
                    || !cfg.users.contains_key(outgoing.user_id.as_str())
                {
                    return Err(Error::GroupNotFound(outgoing.group_id.clone()));
                }
            }
        } else {
            // 普通群组：admin 和普通用户逻辑相同——检查群组存在且发送者在成员中
            let group_swap = cfg.groups.get(outgoing.group_id.as_str())
                .ok_or_else(|| Error::GroupNotFound(outgoing.group_id.clone()))?;
            let group = group_swap.load();
            if !group.members.contains(outgoing.user_id.as_str()) {
                return Err(Error::GroupNotFound(outgoing.group_id.clone()));
            }
        }
        // 在 drop(cfg) 前解析 name（cfg 持有读锁）
        let (messenger_name, user_name, group_name) = self.resolve_names(&cfg, outgoing.user_id.as_str(), outgoing.group_id.as_str());
        drop(cfg);
        // 处理附件消息：解析 content、生成 key（在成员分发之前执行）
        let new_content = kissbot_channel::process_attachment_message(
            outgoing.clone(),
            self.attachment_store.as_ref(),
        ).await.map_err(|e| Error::InternalError(ArcStr::from(e.to_string())))?;
        // 生成消息ID和时间戳
        let msg_id = self.next_msg_id();
        let time = ArcStr::from(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
        // 写入存储
        let admin_msg = IncomingMessage {
            msg_id: msg_id.clone(),
            messenger_id: self.messenger_id.clone(),
            user_id: outgoing.user_id.clone(),
            group_id: outgoing.group_id.clone(),
            messenger_name: messenger_name.clone(),
            user_name: user_name.clone(),
            group_name: group_name.clone(),
            content: new_content.clone(),
            time: time.clone(),
        };
        let date = ArcStr::from(kai_date::as_date(&time));
        let key = MsgKey {
            group_id: admin_msg.group_id.clone(),
            date,
        };
        self.appender.append(key, vec![admin_msg]).await;
        // 发送完成即返回，写入后再推送
        Ok(Arc::new(OutgoingMessageResponse {
            msg_id,
            time,
            content: new_content,
            messenger_name,
            user_name,
            group_name,
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
            let members: Vec<ArcStr> = if admin_msg.group_id.starts_with(ADMIN_USER_GROUP_PREFIX.as_str()) {
                if admin_msg.user_id.as_str() == ADMIN_USER_ID.as_str() {
                    // admin 对 admin-user 单聊组发消息：用 parse 验证用户存在并提取 uid
                    match Self::parse_admin_user_group_ref(&cfg, admin_msg.group_id.as_str()) {
                        Some(uid) => vec![uid],
                        None => vec![],
                    }
                } else {
                    // 普通用户对 admin-user 单聊组发消息：验证 group_id == a_{user_id} 且用户存在
                    if Self::is_admin_user_group_for(admin_msg.user_id.as_str(), admin_msg.group_id.as_str())
                        && cfg.users.contains_key(admin_msg.user_id.as_str())
                    {
                        // 发送者发给自己（agent 按 msg_id 识别自身回显）
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
                        .map(|m| m.clone())
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
                let incoming = Arc::new(IncomingMessage {
                    msg_id: admin_msg.msg_id.clone(),
                    messenger_id: admin_msg.messenger_id.clone(),
                    user_id: admin_msg.user_id.clone(),
                    group_id: admin_msg.group_id.clone(),
                    messenger_name: admin_msg.messenger_name.clone(),
                    user_name: admin_msg.user_name.clone(),
                    group_name: admin_msg.group_name.clone(),
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
        let cfg = self.config.read().await;
        let (messenger_name, user_name, group_name) = self.resolve_names(&cfg, user_id, group_id);
        drop(cfg);
        let event = Arc::new(GroupChangeEvent {
            msg_id: self.next_msg_id(),
            notification: Arc::new(GroupChangeNotification {
                messenger_id: self.messenger_id.clone(),
                user_id: ArcStr::from(user_id),
                group_id: ArcStr::from(group_id),
                messenger_name,
                user_name,
                group_name,
            }),
            change_type,
            time: ArcStr::from(time),
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
                        group_id: gid.clone(),
                        group_name: group.group_name.clone(),
                    }));
                }
            }
            let gid = admin_user_group_id(uid);
            if !cfg.groups.contains_key(&gid) {
                ug_map.insert(gid.clone(), Arc::new(GroupInfo {
                    group_id: gid,
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
            messenger_name: ArcStr::from("Web Chat"),
            user_map: Arc::new(full_user_map),
        }
    }
}
// ========== Creator 与 Messenger trait 实现 ==========
/// 持有完整配置和路径，create() 时用预读的配置构造 WebMessenger。
pub struct WebMessengerCreator {
    repo_path: PathBuf,
    config: Arc<RwLock<WebMessengerRepo>>,
    attachment_dir: ArcStr,
    message_dir: ArcStr,
}
impl WebMessengerCreator {
    pub async fn new(repo_path: &str, attachment_dir: impl Into<ArcStr>, message_dir: impl Into<ArcStr>,
                     messenger_id: impl Into<ArcStr>, admin_name: impl Into<ArcStr>) -> Result<Self> {
        let path = PathBuf::from(repo_path);
        let config = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            serde_json::from_str(&content)?
        } else {
            // repo 文件不存在时根据 config 创建初始结构
            WebMessengerRepo {
                messenger_id: messenger_id.into(),
                admin_name: admin_name.into(),
                users: Arc::new(ArcSwapHashMap::new()),
                groups: Arc::new(ArcSwapHashMap::new()),
                next_user_seq: 0,
                next_group_seq: 0,
            }
        };
        Ok(Self {
            repo_path: path,
            config: Arc::new(RwLock::new(config)),
            attachment_dir: attachment_dir.into(),
            message_dir: message_dir.into(),
        })
    }
    pub async fn messenger_id(&self) -> ArcStr {
        self.config.read().await.messenger_id.clone()
    }
}
#[async_trait]
impl MessengerCreator<WebMessenger> for WebMessengerCreator {
    async fn create(
        &self,
        manager: Weak<ChannelServer>,
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
            .map_err(|e| kissbot_channel::Error::AttachmentNotFound(ArcStr::from(e.to_string())))?;
        let info = meta.info.clone();
        // 生成 transfer_id 并存储 key 映射（下载时由 transfer_id 反查 key）
        let transfer_id = self.attachment_store.next_transfer_id_for(request.key.clone());
        Ok(Arc::new(AttachmentInfoResponse {
            key: request.key.clone(),
            info,
            transfer_id,
        }))
    }
    async fn start_send_download_attachment_payload(&self, transfer_id: u32) -> std::result::Result<(), kissbot_channel::Error> {
        use kissbot_api::channel::OFFSET_ATT_DATA;
        const CHUNK_SIZE: u64 = 65536;
        let manager = self.manager.upgrade()
            .ok_or_else(|| kissbot_channel::Error::InternalError(ArcStr::from("manager unavailable")))?;
        let store = self.attachment_store.clone();
        // 获取附件 key
        let key = store.get_transfer_key(transfer_id)
            .ok_or_else(|| kissbot_channel::Error::AttachmentNotFound(ArcStr::from(transfer_id.to_string())))?;
        let meta = store.get_meta(key.as_str())
            .map_err(|e| kissbot_channel::Error::AttachmentNotFound(ArcStr::from(e.to_string())))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_api::Content;

    fn test_repo(admin_name: &str, users: &[(&str, &str)]) -> Arc<RwLock<WebMessengerRepo>> {
        let mut user_map = ArcSwapHashMap::new();
        for (uid, uname) in users {
            user_map.insert(ArcStr::from(*uid), ArcSwap::new(Arc::new(UserConfig {
                user_id: ArcStr::from(*uid),
                user_name: ArcStr::from(*uname),
            })));
        }
        Arc::new(RwLock::new(WebMessengerRepo {
            messenger_id: ArcStr::from("web"),
            admin_name: ArcStr::from(admin_name),
            users: Arc::new(user_map),
            groups: Arc::new(ArcSwapHashMap::new()),
            next_user_seq: 0,
            next_group_seq: 0,
        }))
    }

    fn make_messenger(config: Arc<RwLock<WebMessengerRepo>>) -> (Arc<WebMessenger>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let messenger = WebMessenger::new(
            ArcStr::from("web"),
            dir.path().join("repo.json"),
            config,
            Weak::new(),
            dir.path().join("attachments").to_str().unwrap(),
            dir.path().join("messages").to_str().unwrap(),
        );
        (messenger, dir)
    }

    #[tokio::test]
    async fn test_send_response_names_admin_single_chat() {
        // admin 发送到 admin-user 单聊组 a_u1：user_name=admin_name，group_name=对端 u1 的 user_name
        let config = test_repo("管理员", &[("u1", "用户1")]);
        let (messenger, _dir) = make_messenger(config);
        let outgoing = Arc::new(OutgoingMessage {
            messenger_id: ArcStr::from("web"),
            user_id: ADMIN_USER_ID.clone(),
            group_id: admin_user_group_id("u1"),
            content: Content::Text(ArcStr::from("hello")),
        });
        let response = messenger.send(outgoing).await.unwrap();
        assert_eq!(response.messenger_name.as_str(), "Web Chat");
        assert_eq!(response.user_name.as_str(), "管理员");
        assert_eq!(response.group_name.as_str(), "用户1");
    }

    #[tokio::test]
    async fn test_resolve_names() {
        let config = test_repo("管理员", &[("u1", "用户1")]);
        // 注入一个普通群组 g1
        let mut groups = ArcSwapHashMap::new();
        groups.insert(ArcStr::from("g1"), ArcSwap::new(Arc::new(GroupConfig {
            group_id: ArcStr::from("g1"),
            group_name: ArcStr::from("研发群"),
            members: Arc::new(HashSet::new()),
        })));
        config.write().await.groups = Arc::new(groups);

        let (messenger, _dir) = make_messenger(config.clone());
        let cfg = config.read().await;

        // admin 发送到 admin-user 单聊组 a_u1：user_name=admin_name，group_name=对端 u1 名
        let (m, u, g) = messenger.resolve_names(&cfg, "admin", &admin_user_group_id("u1"));
        assert_eq!(m.as_str(), "Web Chat");
        assert_eq!(u.as_str(), "管理员");
        assert_eq!(g.as_str(), "用户1");

        // admin 发送到普通群组 g1：group_name=群组名
        let (_, u2, g2) = messenger.resolve_names(&cfg, "admin", "g1");
        assert_eq!(u2.as_str(), "管理员");
        assert_eq!(g2.as_str(), "研发群");

        // 普通用户 u1 发送到单聊组 a_u1：user_name=u1 名，group_name=u1 名
        let (_, u3, g3) = messenger.resolve_names(&cfg, "u1", &admin_user_group_id("u1"));
        assert_eq!(u3.as_str(), "用户1");
        assert_eq!(g3.as_str(), "用户1");

        // 未知 user/group：空串兜底
        let (_, u4, g4) = messenger.resolve_names(&cfg, "nobody", "nope");
        assert_eq!(u4.as_str(), "");
        assert_eq!(g4.as_str(), "");
    }
}
