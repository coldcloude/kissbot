use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::{channel::*, message::*};

use crate::error::Result;
use crate::channel_server::ChannelServer;
use std::sync::{Arc, Weak};

// Messenger trait
#[async_trait]
pub trait Messenger: Send + Sync + 'static {
    async fn get_info(&self) -> Result<Arc<MessengerInfo>>;

    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>>;

    async fn send_attachment_payload(&self, transfer_id: u32, size: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse>;

    async fn download_attachment_header(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>>;

    async fn start_send_download_attachment_payload(&self, transfer_id: u32) -> Result<()>;
}

/// Messenger 创建器。M 为具体 Messenger 类型，create 返回 Arc<M> 供调用方直接使用。
#[async_trait]
pub trait MessengerCreator<M: Messenger> {
    async fn create(&self, manager: Weak<ChannelServer>) -> Result<Arc<M>>;
}
