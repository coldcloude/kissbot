use std::sync::Arc;

use kissbot_api::channel::{AttachmentDownloadRequestDTO, OutgoingMessageDTO};

use crate::error::Result;
use crate::data::*;

// Channel trait
#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    async fn get_info(&self) -> Result<Arc<ChannelInfo>>;

    async fn forward_group_message(&self, message: &GroupChangeEvent) -> Result<()>;
    
    async fn send_message(&self, message: OutgoingMessageDTO) -> Result<Arc<OutgoingMessageResponse>>;

    async fn send_attachment_payload(&self, id: u16, size: u32, pos: u64, data: &[u8]) -> Result<()>;

    async fn download_attachment_header(&self, request: AttachmentDownloadRequestDTO) -> Result<Arc<AttachmentDownloadResponseHeader>>;

    async fn download_attachment_payload(&self, sender: Arc<dyn AttachmentDownloadResponsePayloadSender>) -> Result<()>;

    fn register_on_incoming_messages(&self, callback: Arc<dyn IncomingMessageHandler>);
}
