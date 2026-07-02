use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Weak};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use dashmap::{DashMap, DashSet};
use tokio::sync::oneshot;
use kissbot_api::channel::{
    AttachmentDownloadRequest, AttachmentPayloadResponse, GroupInfo, IncomingMessage, MessengerInfo, OFFSET_ATT_DATA, OutgoingMessage, OutgoingMessageResponse,
    UserInfo,
};
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel::{
    AttachmentDownloadPayloadSender, AttachmentKeyGenerator, GroupChangeEvent, GroupChangeHandler, GroupChangeType,
    IncomingMessageEvent, IncomingMessageHandler, UserRemoveEvent, UserRemoveHandler,
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

// ========== 上传队列命令 ==========

/// 上传队列命令，通过 flume 队列串行处理
enum UploadCommand {
    Write {
        key: String,
        pos: u64,
        size: u32,
        data: Bytes,
        size_bytes: u64,
        temp_path: PathBuf,
        target_path: PathBuf,
        res: oneshot::Sender<std::result::Result<u64, String>>,
    },
}

// ========== 待完成附件上传 ==========

/// 待完成的附件上传信息
pub struct PendingAttachment {
    pub group_id: Arc<String>,
    pub msg_id: Arc<String>,
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
    pub temp_path: PathBuf,
    pub target_path: PathBuf,
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
    pub pending_uploads: DashMap<String, PendingAttachment>,  // key → pending
    pub upload_channels: Arc<DashMap<String, flume::Sender<UploadCommand>>>,
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
            pending_uploads: DashMap::new(),
            upload_channels: Arc::new(DashMap::new()),
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
        let mut cfg = self.config.write().await;
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
            let mut cfg = self.config.write().await;
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
            let mut cfg = self.config.write().await;
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
    pub async fn send(&self, outgoing: OutgoingMessage) -> Result<OutgoingMessageResponse> {
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
        let (new_content, response, pending_attachments) = kissbot_channel::process_attachment_message(
            &outgoing,
            msg_id.as_str(),
            self,
        ).map_err(|e| Error::InternalError(e.to_string()))?;

        // 为每个 key 创建临时文件
        for (info, ref key) in pending_attachments {
            let (temp_path, target_path) = match self.attachment_store.create_temp_file(
                outgoing.group_id.as_str(), msg_id.as_str(), info.file_name.as_str()
            ) {
                Ok(paths) => paths,
                Err(e) => return Err(Error::from(e)),
            };
            self.pending_uploads.insert(key.to_string(), PendingAttachment {
                group_id: outgoing.group_id.clone(),
                msg_id: msg_id.clone(),
                file_name: info.file_name.clone(),
                mime_type: info.mime_type.clone(),
                size_bytes: info.size_bytes,
                temp_path,
                target_path,
            });
        }

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
        let response_content = new_content;
        let sse_event = SseMessage {
            msg_id: msg_id.clone(),
            messenger_id,
            user_id: outgoing.user_id,
            group_id: outgoing.group_id,
            is_self: 1,
            msg_type: outgoing.msg_type.clone(),
            content: Arc::new(serde_json::to_string(&response_content).unwrap_or_default()),
            time: time.clone(),
        };
        let sse_payload = SsePayload { r#type: "message", data: sse_event };
        if let Ok(json) = serde_json::to_string(&sse_payload) {
            self.sse.push(group_id.as_str(), &json);
        }

        Ok(OutgoingMessageResponse {
            msg_id,
            time,
            msg_type: outgoing.msg_type.clone(),
            content: response_content,
        })
    }

    // ========== 上传引擎 ==========

    // ========== 上传引擎 ==========

    /// 写入附件数据。通过 flume 队列串行处理，避免竞争。
    /// 返回当前已写入位置。
    pub fn write_attachment_chunk(
        &self,
        key: &str,
        pos: u64,
        size: u32,
        data: Bytes,
    ) -> Result<u64> {
        // 从 pending_uploads 获取路径信息（在发送到队列前读取）
        let (temp_path, target_path, size_bytes) = {
            let pending = self.pending_uploads.get(key)
                .ok_or_else(|| Error::AttachmentNotFound(key.to_string()))?;
            (pending.temp_path.clone(), pending.target_path.clone(), pending.size_bytes)
        };

        let tx = self.get_or_create_upload_channel(key);
        let (res_tx, res_rx) = oneshot::channel();

        tx.send(UploadCommand::Write {
            key: key.to_string(),
            pos,
            size,
            data,
            size_bytes,
            temp_path,
            target_path,
            res: res_tx,
        }).map_err(|_| Error::InternalError("upload channel closed".to_string()))?;

        // 等待后台任务处理完成
        res_rx.blocking_recv().map_err(|_| Error::InternalError("upload channel recv error".to_string()))?
            .map_err(|e| Error::InternalError(e))
    }

    fn get_or_create_upload_channel(&self, key: &str) -> flume::Sender<UploadCommand> {
        if let Some(entry) = self.upload_channels.get(key) {
            return entry.value().clone();
        }

        let (tx, rx) = flume::unbounded::<UploadCommand>();
        let store = self.attachment_store.clone();
        let key_owned = key.to_string();
        let channels = self.upload_channels.clone();

        tokio::spawn(async move {
            let mut current_pos = 0u64;
            while let Ok(cmd) = rx.recv_async().await {
                match cmd {
                    UploadCommand::Write { key, pos, size, data, size_bytes, temp_path, target_path, res } => {
                        let result = Self::process_upload_write(
                            &store, &mut current_pos, pos, data, size_bytes, &temp_path, &target_path
                        );

                        // 如果是最后一块，清理 channel
                        if let Ok(p) = &result {
                            if *p >= size_bytes {
                                channels.remove(&key);
                            }
                        }
                        let _ = res.send(result);
                    }
                }
            }
        });

        self.upload_channels.insert(key_owned, tx.clone());
        tx
    }

    fn process_upload_write(
        store: &AttachmentStore,
        current_pos: &mut u64,
        pos: u64,
        data: Bytes,
        size_bytes: u64,
        temp_path: &PathBuf,
        target_path: &PathBuf,
    ) -> std::result::Result<u64, String> {
        if pos < *current_pos {
            return Ok(*current_pos);  // 已写入，幂等
        }
        if pos > *current_pos {
            return Err(format!("out of order: expected pos={}, got pos={}", *current_pos, pos));
        }

        store.append_to_temp(temp_path, &data)
            .map_err(|e| e.to_string())?;
        *current_pos = pos + data.len() as u64;

        if *current_pos >= size_bytes {
            AttachmentStore::finalize_upload(temp_path, target_path)
                .map_err(|e| e.to_string())?;
        }

        Ok(*current_pos)
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
}

impl WebMessengerCreator {
    pub async fn new(repo_path: &str, attachment_dir: &str) -> Result<Self> {
        let path = PathBuf::from(repo_path);
        let content = std::fs::read_to_string(&path)?;
        let config: WebMessengerRepo = serde_json::from_str(&content)?;
        Ok(Self {
            repo_path: path,
            config: Arc::new(RwLock::new(config)),
            attachment_dir: attachment_dir.to_string(),
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
        _global_attachment_sn: Arc<AtomicU32>,
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
        let resp = self.send(message).await?;
        Ok(Arc::new(resp))
    }

    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> std::result::Result<AttachmentPayloadResponse, kissbot_channel::Error> {
        self.write_attachment_chunk(key, pos, size, data)
            .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;
        Ok(AttachmentPayloadResponse {
            key: Arc::new(key.to_string()),
            pos,
            size,
            error_code: 0,
            error_msg: None,
        })
    }

    async fn download_attachment_header(&self, request: AttachmentDownloadRequest, _attachment_sn: Arc<AtomicU32>) -> std::result::Result<Arc<AttachmentInfoResponse>, kissbot_channel::Error> {
        let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;
        let info = AttachmentInfo {
            file_name: meta.file_name.clone(),
            mime_type: meta.mime_type.clone(),
            size_bytes: meta.size_bytes,
        };

        Ok(Arc::new(AttachmentInfoResponse {
            key: Arc::clone(&request.key),
            info: Arc::new(info),
        }))
    }

    async fn start_send_download_attachment_payload(&self, key: &str) -> std::result::Result<(), kissbot_channel::Error> {
        let sender = self.on_download_attachment_payload.upgrade()
            .ok_or_else(|| kissbot_channel::Error::InternalError("download payload sender unavailable".to_string()))?;
        let store = self.attachment_store.clone();
        let key_owned = key.to_string();

        tokio::spawn(async move {
            const CHUNK_SIZE: u64 = 65536;

            let file_result = store.open_file(&key_owned);
            let (mut file, file_len) = match file_result {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!("Failed to open attachment for download: key={}, error={}", key_owned, e);
                    return;
                }
            };

            let mut pos = 0u64;
            let mut ok = true;
            while pos < file_len && ok {
                let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
                let chunk_size = (end - pos) as usize;
                let (sn, mut buf) = match sender.prepare_send(&key_owned, chunk_size as u32, pos) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!("prepare_send error: {}", e);
                        break;
                    }
                };
                // 读取到 payload 偏移处（prepare_send 已分配足够 capacity）
                use std::io::Read;
                if let Err(e) = (&mut file).read_exact(&mut buf[OFFSET_ATT_DATA..OFFSET_ATT_DATA + chunk_size]) {
                    tracing::error!("Failed to read file chunk: {}", e);
                    break;
                }
                ok = sender.send(sn, &key_owned, chunk_size as u32, pos, buf).await.is_ok();
                pos = end;
            }
            // 发送 size=0 的结束标记
            if let Ok((sn, buf)) = sender.prepare_send(&key_owned, 0, pos) {
                let _ = sender.send(sn, &key_owned, 0, pos, buf).await;
            }
        });

        Ok(())
    }
}

impl AttachmentKeyGenerator for WebMessenger {
    fn generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String {
        format!("{}/{}/{}", group_id, msg_id, info.file_name)
    }
}
