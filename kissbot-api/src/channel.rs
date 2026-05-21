use kai_ws::{LEN_PAYLOAD_TYPE, OFFSET_PAYLOAD_TYPE};
use serde::{Deserialize, Serialize};

use crate::{MapKind, StringKind, error::Result};

//========================= Attachment ==========================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMetadata{
    pub mime_type: String,
    pub size_bytes: u64,
}

pub struct OutgoingMessageGeneric<S, M>
where
    S: StringKind,
    M: MapKind,
{
    pub channel_id: S::Type,
    pub user_id: S::Type,
    pub is_self: usize,
    pub msg_type: S::Type,
    pub content: S::Type,
    pub time: S::Type,
    pub attachment_map: M::Map<String, AttachmentMetadata>,
}

pub struct OutgoingMessageResponseGeneric<S, M>
where
    S: StringKind,
    M: MapKind,
{
    pub msg_id: S::Type,
    pub attachment_upload_id_map: M::Map<String, u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadRequestGeneric<S>
where
    S: StringKind,
{
    pub key: S::Type,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadResponseHeader
{
    pub download_id: u32,
    pub metadata: AttachmentMetadata,
}

//========================= Attachment Binary ==========================

pub const BIN_TYPE_ATTACHMENT: u16 = 0x0001;

const OFFSET_ATT_ID: usize = OFFSET_PAYLOAD_TYPE + LEN_PAYLOAD_TYPE;
const LEN_ATT_ID: usize = 4;
const OFFSET_ATT_SIZE: usize = OFFSET_ATT_ID + LEN_ATT_ID;
const LEN_ATT_SIZE: usize = 4;
const OFFSET_ATT_POS: usize = OFFSET_ATT_SIZE + LEN_ATT_SIZE;
const LEN_ATT_POS: usize = 8;
const OFFSET_ATT_DATA: usize = OFFSET_ATT_POS + LEN_ATT_POS;

pub trait AttachmentProcessor {
    fn process_attachment(&self, id: u32, size: u32, pos: u64, data: &[u8]) -> Result<()>;
}

pub fn process_attachment<P: AttachmentProcessor>(data: &[u8], processor: &mut P) -> Result<()> {
    let id_bin: [u8; 4] = data[OFFSET_ATT_ID..OFFSET_ATT_ID + LEN_ATT_ID].try_into()?;
    let id = u32::from_be_bytes(id_bin);
    let size_bin: [u8; 4] = data[OFFSET_ATT_SIZE..OFFSET_ATT_SIZE + LEN_ATT_SIZE].try_into()?;
    let size = u32::from_be_bytes(size_bin);
    let pos_bin: [u8; 8] = data[OFFSET_ATT_POS..OFFSET_ATT_POS + LEN_ATT_POS].try_into()?;
    let pos = u64::from_be_bytes(pos_bin);
    let data = &data[OFFSET_ATT_DATA..];
    processor.process_attachment(id, size, pos, data)?;
    Ok(())
}
