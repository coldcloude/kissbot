#![allow(dead_code)]

use std::sync::{Arc, RwLock, Weak, atomic::{AtomicU32, Ordering}};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel::{Messenger, MessengerCreator, Error as ChannelError, ChannelManager};
use kissbot_channel::{GroupChangeEvent, GroupChangeType, UserRemoveEvent};
use kissbot_channel_client::error::Result as ClientResult;
use kissbot_channel_client::Terminal;

pub const TEST_TIME: &str = "2026-07-27 00:00:00";
pub const DOWNLOAD_CHUNK_SIZE: usize = 4;

/// 测试配置：ChannelManager 内部会读取 config.json（memory-store 推送、api key 校验），
/// 测试用临时配置文件，memory_store_url 指向不可达地址（错误由 NoopErrorHandler 吞掉）。
pub fn test_config_setup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join("kissbot-channel-client-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{
            "security": { "api_key": "test-key", "admin_api_key": "admin-key" },
            "api": { "memory_store_url": "http://127.0.0.1:1", "memory_ego_url": "http://127.0.0.1:1" }
        }"#).unwrap();
        // edition 2024: set_var 是 unsafe
        unsafe { std::env::set_var("KISSBOT_CONFIG", &path); }
    });
}

pub fn make_messenger_info(messenger_id: &str, user_id: &str, group_id: &str) -> MessengerInfo {
    let group = Arc::new(GroupInfo {
        group_id: Arc::new(group_id.to_string()),
        group_name: Arc::new("test-group".to_string()),
    });
    let group_map = Arc::new(DashMap::new());
    group_map.insert(group_id.to_string(), group);
    let user = Arc::new(UserInfo {
        user_id: Arc::new(user_id.to_string()),
        user_name: Arc::new("test-user".to_string()),
        group_map,
    });
    let user_map = Arc::new(DashMap::new());
    user_map.insert(user_id.to_string(), user);
    MessengerInfo {
        messenger_id: Arc::new(messenger_id.to_string()),
        messenger_name: Arc::new("test-messenger".to_string()),
        user_map,
    }
}

// ========== Mock Messenger ==========

pub struct MockMessenger {
    pub info: Arc<MessengerInfo>,
    pub download_data: Bytes,
    next_transfer_id: AtomicU32,
    next_msg_id: AtomicU32,
    manager: RwLock<Option<Weak<ChannelManager>>>,
    pub sent_messages: flume::Sender<OutgoingMessage>,
    sent_messages_rx: flume::Receiver<OutgoingMessage>,
    pub upload_chunks: flume::Sender<(u32, u64, Bytes)>,
    upload_chunks_rx: flume::Receiver<(u32, u64, Bytes)>,
}

impl MockMessenger {
    pub fn new(info: MessengerInfo, download_data: &[u8]) -> Arc<Self> {
        let (sent_messages, sent_messages_rx) = flume::unbounded();
        let (upload_chunks, upload_chunks_rx) = flume::unbounded();
        Arc::new(Self {
            info: Arc::new(info),
            download_data: Bytes::copy_from_slice(download_data),
            next_transfer_id: AtomicU32::new(1),
            next_msg_id: AtomicU32::new(1),
            manager: RwLock::new(None),
            sent_messages,
            sent_messages_rx,
            upload_chunks,
            upload_chunks_rx,
        })
    }

    pub fn sent_messages_rx(&self) -> flume::Receiver<OutgoingMessage> {
        self.sent_messages_rx.clone()
    }

    pub fn upload_chunks_rx(&self) -> flume::Receiver<(u32, u64, Bytes)> {
        self.upload_chunks_rx.clone()
    }

    /// 模拟外部消息到达，经 ChannelManager 推送给终端
    pub fn push_incoming(&self, msg: IncomingMessage) {
        let handler = self.manager.read().unwrap().clone().and_then(|w| w.upgrade());
        if let Some(manager) = handler {
            tokio::spawn(async move {
                manager.handle_incoming_message(Arc::new(msg)).await;
            });
        }
    }

    /// 模拟群组变化
    pub fn push_group_change(&self, change_type: GroupChangeType, user_id: &str, group_id: &str) {
        let handler = self.manager.read().unwrap().clone().and_then(|w| w.upgrade());
        if let Some(manager) = handler {
            let messenger_id = self.info.messenger_id.clone();
            let event = Arc::new(GroupChangeEvent {
                msg_id: Arc::new("gc-1".to_string()),
                notification: Arc::new(GroupChangeNotification {
                    messenger_id,
                    group_id: Arc::new(group_id.to_string()),
                    user_id: Arc::new(user_id.to_string()),
                }),
                change_type,
                time: Arc::new(TEST_TIME.to_string()),
            });
            tokio::spawn(async move {
                manager.handle_group_change(event).await;
            });
        }
    }

    /// 模拟用户被删除
    pub fn push_user_remove(&self, user_id: &str) {
        let handler = self.manager.read().unwrap().clone().and_then(|w| w.upgrade());
        if let Some(manager) = handler {
            let messenger_id = self.info.messenger_id.clone();
            let event = Arc::new(UserRemoveEvent {
                msg_id: Arc::new("ur-1".to_string()),
                notification: Arc::new(UserRemoveNotification {
                    messenger_id,
                    user_id: Arc::new(user_id.to_string()),
                }),
                time: Arc::new(TEST_TIME.to_string()),
            });
            tokio::spawn(async move {
                manager.handle_user_remove(event).await;
            });
        }
    }
}

#[async_trait]
impl Messenger for MockMessenger {
    async fn get_info(&self) -> Result<Arc<MessengerInfo>, ChannelError> {
        Ok(self.info.clone())
    }

    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>, ChannelError> {
        // 附件消息转换为 AttachmentInfoResponse（嵌入 key 与 transfer_id），其他消息原样返回
        let content = match &message.content {
            Content::AttachmentInfo(info) => Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
                key: Arc::new(format!("key-{}", info.file_name)),
                info: info.clone(),
                transfer_id: self.next_transfer_id.fetch_add(1, Ordering::Relaxed),
            })),
            other => other.clone(),
        };
        let msg_type = message.msg_type.clone();
        let _ = self.sent_messages.send(message);
        Ok(Arc::new(OutgoingMessageResponse {
            msg_id: Arc::new(format!("msg-{}", self.next_msg_id.fetch_add(1, Ordering::Relaxed))),
            time: Arc::new(TEST_TIME.to_string()),
            msg_type,
            content,
        }))
    }

    async fn send_attachment_payload(&self, transfer_id: u32, size: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse, ChannelError> {
        let _ = self.upload_chunks.send((transfer_id, pos, data));
        Ok(AttachmentPayloadResponse {
            current_pos: pos + size as u64,
            error_code: PAYLOAD_ERRCODE_OK,
            error_msg: None,
        })
    }

    async fn download_attachment_header(&self, _request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>, ChannelError> {
        Ok(Arc::new(AttachmentInfoResponse {
            key: Arc::new("download-key".to_string()),
            info: Arc::new(AttachmentInfo {
                file_name: Arc::new("download.bin".to_string()),
                mime_type: Arc::new("application/octet-stream".to_string()),
                size_bytes: self.download_data.len() as u64,
            }),
            transfer_id: self.next_transfer_id.fetch_add(1, Ordering::Relaxed),
        }))
    }

    async fn start_send_download_attachment_payload(&self, transfer_id: u32) -> Result<(), ChannelError> {
        let manager = self.manager.read().unwrap().clone()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| ChannelError::InternalError("manager is None".to_string()))?;
        let data = self.download_data.clone();
        tokio::spawn(async move {
            let mut pos = 0u64;
            while pos < data.len() as u64 {
                let end = (pos as usize + DOWNLOAD_CHUNK_SIZE).min(data.len());
                let chunk = data.slice(pos as usize..end);
                let size = chunk.len() as u32;
                let (sn, mut buf) = match manager.prepare_download_payload(transfer_id, size, pos) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("prepare_download_payload error: {:?}", e);
                        break;
                    }
                };
                buf.extend_from_slice(&chunk);
                if let Err(e) = manager.send_download_payload(sn, transfer_id, size, pos, buf).await {
                    eprintln!("send_download_payload error: {:?}", e);
                    break;
                }
                pos = end as u64;
            }
        });
        Ok(())
    }
}

pub struct MockMessengerCreator {
    pub messenger: Arc<MockMessenger>,
}

#[async_trait]
impl MessengerCreator<MockMessenger> for MockMessengerCreator {
    async fn create(&self, manager: Weak<ChannelManager>) -> Result<Arc<MockMessenger>, ChannelError> {
        *self.messenger.manager.write().unwrap() = Some(manager);
        Ok(self.messenger.clone())
    }
}

// ========== Mock Terminal ==========

pub struct MockTerminal {
    pub incoming: flume::Sender<Arc<IncomingMessage>>,
    pub joins: flume::Sender<Arc<GroupChangeNotification>>,
    pub leaves: flume::Sender<Arc<GroupChangeNotification>>,
    pub removals: flume::Sender<Arc<UserRemoveNotification>>,
    pub chunks: flume::Sender<(Arc<AttachmentInfoResponse>, u64, Bytes)>,
    pub closed_tx: flume::Sender<()>,
    incoming_rx: flume::Receiver<Arc<IncomingMessage>>,
    joins_rx: flume::Receiver<Arc<GroupChangeNotification>>,
    leaves_rx: flume::Receiver<Arc<GroupChangeNotification>>,
    removals_rx: flume::Receiver<Arc<UserRemoveNotification>>,
    chunks_rx: flume::Receiver<(Arc<AttachmentInfoResponse>, u64, Bytes)>,
    closed_rx: flume::Receiver<()>,
}

impl MockTerminal {
    pub fn new() -> Arc<Self> {
        let (incoming, incoming_rx) = flume::unbounded();
        let (joins, joins_rx) = flume::unbounded();
        let (leaves, leaves_rx) = flume::unbounded();
        let (removals, removals_rx) = flume::unbounded();
        let (chunks, chunks_rx) = flume::unbounded();
        let (closed_tx, closed_rx) = flume::unbounded();
        Arc::new(Self {
            incoming,
            joins,
            leaves,
            removals,
            chunks,
            closed_tx,
            incoming_rx,
            joins_rx,
            leaves_rx,
            removals_rx,
            chunks_rx,
            closed_rx,
        })
    }

    pub fn incoming_rx(&self) -> flume::Receiver<Arc<IncomingMessage>> { self.incoming_rx.clone() }
    pub fn joins_rx(&self) -> flume::Receiver<Arc<GroupChangeNotification>> { self.joins_rx.clone() }
    pub fn leaves_rx(&self) -> flume::Receiver<Arc<GroupChangeNotification>> { self.leaves_rx.clone() }
    pub fn removals_rx(&self) -> flume::Receiver<Arc<UserRemoveNotification>> { self.removals_rx.clone() }
    pub fn chunks_rx(&self) -> flume::Receiver<(Arc<AttachmentInfoResponse>, u64, Bytes)> { self.chunks_rx.clone() }
    pub fn closed_rx(&self) -> flume::Receiver<()> { self.closed_rx.clone() }
}

#[async_trait]
impl Terminal for MockTerminal {
    async fn incoming_message(&self, _id: &str, message: Arc<IncomingMessage>) {
        let _ = self.incoming.send(message);
    }

    async fn join_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) {
        let _ = self.joins.send(notification);
    }

    async fn leave_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) {
        let _ = self.leaves.send(notification);
    }

    async fn user_removed(&self, _id: &str, notification: Arc<UserRemoveNotification>) {
        let _ = self.removals.send(notification);
    }

    async fn download_chunk(&self, _id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> ClientResult<()> {
        let _ = self.chunks.send((info, pos, data));
        Ok(())
    }

    async fn closed(&self, _id: &str) {
        let _ = self.closed_tx.send(());
    }
}

// ========== 测试辅助 ==========

/// 启动带一个 mock messenger 的 ChannelManager，监听指定端口
pub async fn start_test_server(port: u16, messenger: Arc<MockMessenger>) -> Arc<kissbot_channel::ChannelManager> {
    let manager = Arc::new(kissbot_channel::ChannelManager::new());
    let messenger_id = messenger.info.messenger_id.to_string();
    manager.register_messenger(&messenger_id, MockMessengerCreator { messenger }).await.unwrap();
    let m = manager.clone();
    tokio::spawn(async move {
        m.start(&format!("127.0.0.1:{}", port)).await.unwrap();
    });
    // 等待 listener 就绪
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    manager
}

pub fn make_bind_request(messenger_id: &str, user_id: &str) -> BindRequest {
    BindRequest {
        agent_id: Arc::new("test-agent".to_string()),
        role_name: Arc::new("test-role".to_string()),
        messenger_id: Arc::new(messenger_id.to_string()),
        user_id: Arc::new(user_id.to_string()),
    }
}

pub fn make_text_incoming(messenger_id: &str, user_id: &str, group_id: &str, text: &str) -> IncomingMessage {
    IncomingMessage {
        msg_id: Arc::new("in-1".to_string()),
        messenger_id: Arc::new(messenger_id.to_string()),
        user_id: Arc::new(user_id.to_string()),
        group_id: Arc::new(group_id.to_string()),
        is_self: 0,
        msg_type: Arc::new(MSG_TYPE_TEXT.to_string()),
        content: Content::Text(Arc::new(text.to_string())),
        time: Arc::new(TEST_TIME.to_string()),
    }
}
