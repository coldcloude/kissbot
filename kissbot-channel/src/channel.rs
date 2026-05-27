use std::sync::{Arc, Weak};

use kissbot_api::channel::{AttachmentDownloadRequestDTO, OutgoingMessageDTO};

use crate::error::Result;
use crate::data::*;

// Channel trait
#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    fn get_info(&self) -> Arc<ChannelInfo>;

    fn group_change_to_incoming_message(&self, message: Arc<GroupChangeEvent>) -> Arc<IncomingMessageEvent>;
    
    async fn send_message(&self, message: OutgoingMessageDTO) -> Result<Arc<OutgoingMessageResponse>>;

    async fn send_attachment_payload(&self, id: u16, size: u32, pos: u64, data: &[u8]) -> Result<()>;

    async fn download_attachment_header(&self, request: AttachmentDownloadRequestDTO) -> Result<Arc<AttachmentDownloadResponseHeader>>;

    async fn download_attachment_payload(&self, sender: Arc<dyn AttachmentDownloadResponsePayloadSender>) -> Result<()>;

    fn register_on_incoming_messages(&self, callback: Weak<dyn IncomingMessageHandler>);
}
