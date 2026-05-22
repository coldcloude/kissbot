use std::sync::Arc;

use kissbot_api::channel::{AttachmentDownloadRequestDTO, OutgoingMessageDTO};

use crate::error::Result;
use crate::data::*;

// Channel trait
#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    fn channel_id(&self) -> &str;
    fn messenger_id(&self) -> &str;
    fn user_id(&self) -> &str;
    fn group_id(&self) -> &str;
    fn agent_id(&self) -> &str;
    
    async fn send_message(&self, message: OutgoingMessageDTO) -> Result<Arc<OutgoingMessageResponse>>;

    async fn send_attachment_payload(&self, id: u16, size: u32, pos: u64, data: &[u8]) -> Result<()>;

    async fn download_attachment_header(&self, request: AttachmentDownloadRequestDTO) -> Result<Arc<AttachmentDownloadResponseHeader>>;

    async fn download_attachment_payload(&self, sender: Arc<dyn AttachmentDownloadResponsePayloadSender>) -> Result<()>;
}
