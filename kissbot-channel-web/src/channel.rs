use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use dashmap::DashMap;
use kissbot_api::channel::{
    AttachmentDownloadRequestDTO, OutgoingMessageDTO,
};
use kissbot_channel::{
    AttachmentDownloadPayloadSender, AttachmentDownloadResponseHeader,
    AttachmentInfo, Channel, ChannelInfo, GroupChangeEvent, IncomingMessage,
    IncomingMessageEvent, IncomingMessageHandler,
    OutgoingMessageResponse,
};
use kissbot_channel::error::Result as ChannelResult;

/// 每个 (user, group) 组合对应一个 WebChannel 实例
pub struct WebChannel {
    info: Arc<ChannelInfo>,
    on_message_received: tokio::sync::RwLock<Option<Weak<dyn IncomingMessageHandler>>>,
    /// SSE event sender: 向 admin 前端推送消息
    sse_sender: flume::Sender<Arc<IncomingMessage>>,
}

impl WebChannel {
    pub fn new(info: Arc<ChannelInfo>) -> (Self, flume::Receiver<Arc<IncomingMessage>>) {
        let (sender, receiver) = flume::unbounded();
        (
            Self {
                info,
                on_message_received: tokio::sync::RwLock::new(None),
                sse_sender: sender,
            },
            receiver,
        )
    }

    #[allow(dead_code)]
    pub fn sse_sender(&self) -> &flume::Sender<Arc<IncomingMessage>> {
        &self.sse_sender
    }

    /// 前端消息到达时的回调触发
    pub async fn trigger_incoming_message(&self, message: Arc<IncomingMessage>) {
        let handler = self.on_message_received.read().await;
        if let Some(weak) = handler.as_ref() {
            if let Some(handler) = weak.upgrade() {
                let event = Arc::new(IncomingMessageEvent {
                    messages: Arc::new(vec![message]),
                });
                handler.handle_incoming_message(event).await;
            }
        }
    }
}

#[async_trait]
impl Channel for WebChannel {
    fn get_info(&self) -> Arc<ChannelInfo> {
        self.info.clone()
    }

    fn group_change_to_incoming_message(&self, message: Arc<GroupChangeEvent>) -> Arc<IncomingMessageEvent> {
        let incoming = Arc::new(IncomingMessage {
            msg_id: Arc::new(String::new()),
            messenger_id: message.messenger_id.clone(),
            user_id: message.user_id.clone(),
            group_id: message.group_id.clone(),
            is_self: 0,
            msg_type: Arc::new(match message.change_type {
                kissbot_channel::GroupChangeType::Joined => "system_join".to_string(),
                kissbot_channel::GroupChangeType::Left => "system_leave".to_string(),
            }),
            content: Arc::new(String::new()),
            time: message.time.clone(),
        });
        Arc::new(IncomingMessageEvent {
            messages: Arc::new(vec![incoming]),
        })
    }

    async fn send_message(&self, message: OutgoingMessageDTO, _attachment_sn: Arc<AtomicU32>) -> ChannelResult<Arc<OutgoingMessageResponse>> {
        // 将下行消息通过 SSE 推送给 admin 前端
        let incoming = Arc::new(IncomingMessage {
            msg_id: Arc::new(uuid::Uuid::new_v4().to_string()),
            messenger_id: Arc::new(message.messenger_id.clone()),
            user_id: Arc::new(message.user_id.clone()),
            group_id: Arc::new(message.group_id.clone()),
            is_self: 0, // agent 发送的消息
            msg_type: Arc::new(message.msg_type.clone()),
            content: Arc::new(message.content.clone()),
            time: Arc::new(message.time.clone()),
        });

        self.sse_sender.send(incoming)
            .map_err(|e| kissbot_channel::Error::InternalError(format!("SSE send error: {}", e)))?;

        let upload_id_map: Arc<DashMap<String, u32>> = Arc::new(DashMap::new());
        Ok(Arc::new(OutgoingMessageResponse {
            msg_id: Arc::new(uuid::Uuid::new_v4().to_string()),
            attachment_upload_id_map: upload_id_map,
        }))
    }

    async fn send_attachment_payload(&self, _id: u32, _size: u32, _pos: u64, _data: &[u8]) -> ChannelResult<()> {
        // WebChannel 不需要通过二进制发送附件，附件通过 HTTP API 传输
        Ok(())
    }

    async fn download_attachment_header(&self, request: AttachmentDownloadRequestDTO, _attachment_sn: Arc<AtomicU32>) -> ChannelResult<Arc<AttachmentDownloadResponseHeader>> {
        // 返回附件元数据
        let meta = crate::attachment::AttachmentStore::new("attachments")
            .get_meta_by_key(&request.key)
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

    fn register_on_download_attachment_payload(&self, _sender: Arc<dyn AttachmentDownloadPayloadSender>) {
        // WebChannel 不需要处理附件 payload 发送
    }

    fn register_on_incoming_messages(&self, callback: Weak<dyn IncomingMessageHandler>) {
        *self.on_message_received.blocking_write() = Some(callback);
    }
}
