use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{MapKind, StringKind, error::Result, ws::{LEN_TYPE, OFFSET_TYPE}};

//========================= Attachment ==========================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentGeneric<S>
where
    S: StringKind,
{
    pub mime_type: S::Type,
    pub size_bytes: u64,
}

pub trait AttachmentKind {
    type Type: Clone + Serialize + DeserializeOwned;
}

pub struct OutgoingMessageGeneric<S, M, A>
where
    S: StringKind,
    M: MapKind,
    A: AttachmentKind,
{
    pub channel_id: S::Type,
    pub user_id: S::Type,
    pub is_self: usize,
    pub msg_type: S::Type,
    pub content: S::Type,
    pub time: S::Type,
    pub attachment_map: M::Map<String, A::Type>,
}

pub struct OutgoingMessageResponse<S, M>
where
    S: StringKind,
    M: MapKind,
{
    pub msg_id: S::Type,
    pub attachment_upload_id_map: M::Map<String, u32>,
}

//========================= Attachment Binary ==========================

pub const BIN_TYPE_ATTACHMENT: u16 = 0x0001;

const OFFSET_ATT_ID: usize = OFFSET_TYPE + LEN_TYPE;
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
