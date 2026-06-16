use async_trait::async_trait;
use kissbot_api::{AttachmentDownloadRequestDTO, OutgoingMessageDTO};

use crate::error::Result;
use crate::data::*;
use std::sync::{Arc, Weak, atomic::AtomicU32};

// Messenger trait
#[async_trait]
pub trait Messenger: Send + Sync + 'static {
    async fn get_info(&self) -> Result<Arc<MessengerInfo>>;

    async fn send_message(&self, message: OutgoingMessageDTO, attachment_sn: Arc<AtomicU32>) -> Result<Arc<OutgoingMessageResponse>>;

    async fn send_attachment_payload(&self, id: u32, size: u32, pos: u64, data: &[u8]) -> Result<()>;

    async fn download_attachment_header(&self, request: AttachmentDownloadRequestDTO, attachment_sn: Arc<AtomicU32>) -> Result<Arc<AttachmentDownloadResponseHeader>>;
}

/// Messenger 创建器。M 为具体 Messenger 类型，create 返回 Arc<M> 供调用方直接使用。
#[async_trait]
pub trait MessengerCreator<M: Messenger> {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>
    ) -> Result<Arc<M>>;
}
