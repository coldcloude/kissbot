use std::array::TryFromSliceError;
use std::sync::Arc;

use dashmap::DashMap;
use kai_ws::{LEN_STATUS_CODE, OFFSET_STATUS_CODE};
use serde::{Deserialize, Serialize};

pub const TYPE_MESSENGER_INFO_REQUEST: u32 = 0x00010001;
pub const TYPE_BIND_AGENT_USER: u32 = 0x00020002;
pub const TYPE_UNBIND_AGENT_USER: u32 = 0x00020003;
pub const TYPE_OUTGOING_MESSAGE: u32 = 0x00030004;
pub const TYPE_ATTACHMENT_DOWNLOAD_REQUEST: u32 = 0x00030005;
pub const TYPE_JOIN_GROUP: u32 = 0x10010001;
pub const TYPE_LEAVE_GROUP: u32 = 0x10010002;
pub const TYPE_INCOMING_MESSAGE: u32 = 0x10020003;
pub const TYPE_ATTACHMENT_DOWNLOAD_PAYLOAD: u32 = 0x10020004;

// ========== 消息类型常量 ==========

pub const MSG_TYPE_TEXT: &str = "text";
pub const MSG_TYPE_IMAGE: &str = "image";
pub const MSG_TYPE_FILE: &str = "file";
pub const MSG_TYPE_SYSTEM_JOIN: &str = "system_join";
pub const MSG_TYPE_SYSTEM_LEAVE: &str = "system_leave";

// ========== Messenger -> User -> Group-> Channel ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub messenger_id: Arc<String>,
    pub group_id: Arc<String>,
    pub user_id: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfo {
    pub group_id: Arc<String>,
    pub group_name: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: Arc<String>,
    pub user_name: Arc<String>,
    pub group_map: Arc<DashMap<String, Arc<GroupInfo>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerInfo {
    pub messenger_id: Arc<String>,
    pub messenger_name: Arc<String>,
    pub user_map: Arc<DashMap<String, Arc<UserInfo>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerInfoRequest {
    pub messenger_id: Arc<String>,
}

// ========================= Message & Attachment ==========================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub att_id: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub attachment_map: Arc<DashMap<String, Arc<AttachmentInfo>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub attachment_upload_id_map: Arc<DashMap<String, u32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadRequest {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub key: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadResponseHeader {
    pub download_id: u32,
    pub metadata: Arc<AttachmentInfo>,
}

// ========================= Attachment Binary ==========================

pub const TYPE_ATTACHMENT_PAYLOAD: u32 = 0x20010001;

const OFFSET_ATT_ID: usize = OFFSET_STATUS_CODE + LEN_STATUS_CODE;
const LEN_ATT_ID: usize = 4;
const OFFSET_ATT_SIZE: usize = OFFSET_ATT_ID + LEN_ATT_ID;
const LEN_ATT_SIZE: usize = 4;
const OFFSET_ATT_POS: usize = OFFSET_ATT_SIZE + LEN_ATT_SIZE;
const LEN_ATT_POS: usize = 8;

pub const OFFSET_ATT_DATA: usize = OFFSET_ATT_POS + LEN_ATT_POS;

pub struct AttachmentPayloadHeader {
    pub id: u32,
    pub size: u32,
    pub pos: u64,
}

pub fn parse_attachment_payload_header(data: &[u8]) -> std::result::Result<AttachmentPayloadHeader, TryFromSliceError> {
    let id_bin: [u8; 4] = data[OFFSET_ATT_ID..OFFSET_ATT_ID + LEN_ATT_ID].try_into()?;
    let id = u32::from_be_bytes(id_bin);
    let size_bin: [u8; 4] = data[OFFSET_ATT_SIZE..OFFSET_ATT_SIZE + LEN_ATT_SIZE].try_into()?;
    let size = u32::from_be_bytes(size_bin);
    let pos_bin: [u8; 8] = data[OFFSET_ATT_POS..OFFSET_ATT_POS + LEN_ATT_POS].try_into()?;
    let pos = u64::from_be_bytes(pos_bin);
    Ok(AttachmentPayloadHeader { id, size, pos })
}

// ================= Receiving Message ==========================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub msg_id: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub time: Arc<String>,
}

// ========== Query & Bind ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
}
