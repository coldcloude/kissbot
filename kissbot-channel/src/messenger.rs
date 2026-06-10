use kissbot_api::{AttachmentDownloadRequestDTO, OutgoingMessageDTO};

use crate::error::Result;
use crate::data::*;
use std::sync::{Arc, Weak, atomic::AtomicU32};

// Messenger trait
#[async_trait::async_trait]
pub trait Messenger: Send + Sync {
    fn messenger_id(&self) -> &str;

    async fn get_info(&self) -> Result<Arc<MessengerInfo>>;

    async fn send_message(&self, message: OutgoingMessageDTO, attachment_sn: Arc<AtomicU32>) -> Result<Arc<OutgoingMessageResponse>>;

    async fn send_attachment_payload(&self, id: u32, size: u32, pos: u64, data: &[u8]) -> Result<()>;

    async fn download_attachment_header(&self, request: AttachmentDownloadRequestDTO, attachment_sn: Arc<AtomicU32>) -> Result<Arc<AttachmentDownloadResponseHeader>>;
    
    fn register_on_group_change(&self, callback: Weak<dyn GroupChangeHandler>);

    fn register_on_download_attachment_payload(&self, sender: Weak<dyn AttachmentDownloadPayloadSender>);

    fn register_on_incoming_messages(&self, callback: Weak<dyn IncomingMessageHandler>);
}
