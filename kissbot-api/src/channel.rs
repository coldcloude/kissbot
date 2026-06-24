use std::sync::Arc;

use dashmap::DashMap;
use kai_ws::{LEN_STATUS_CODE, OFFSET_STATUS_CODE};
use serde::{Deserialize, Serialize};

pub const TYPE_MESSENGER_INFO_REQUEST: u32 = 0x00010001;
pub const TYPE_BIND_AGENT_USER: u32 = 0x00020002;
pub const TYPE_UNBIND_AGENT_USER: u32 = 0x00020003;
pub const TYPE_USER_REMOVED: u32 = 0x00020004;
pub const TYPE_OUTGOING_MESSAGE: u32 = 0x00030004;
pub const TYPE_ATTACHMENT_DOWNLOAD_REQUEST: u32 = 0x00030005;
pub const TYPE_JOIN_GROUP: u32 = 0x10010001;
pub const TYPE_LEAVE_GROUP: u32 = 0x10010002;
pub const TYPE_INCOMING_MESSAGE: u32 = 0x10020003;
pub const TYPE_ATTACHMENT_DOWNLOAD_PAYLOAD: u32 = 0x10020004;

// ========== Group Change Notification ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChangeNotification {
    pub messenger_id: Arc<String>,
    pub group_id: Arc<String>,
    pub user_id: Arc<String>,
}

// ========== User Remove Notification ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRemoveNotification {
    pub messenger_id: Arc<String>,
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
    pub time: Arc<String>,
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

pub fn parse_attachment_payload_header(data: &[u8]) -> std::result::Result<AttachmentPayloadHeader, kai_ws::Error> {
    let id_bytes: [u8; 4] = data.get(OFFSET_ATT_ID..OFFSET_ATT_ID + LEN_ATT_ID)
        .and_then(|s| s.try_into().ok())
        .ok_or(kai_ws::Error::BinParse)?;
    let id = u32::from_be_bytes(id_bytes);
    let size_bytes: [u8; 4] = data.get(OFFSET_ATT_SIZE..OFFSET_ATT_SIZE + LEN_ATT_SIZE)
        .and_then(|s| s.try_into().ok())
        .ok_or(kai_ws::Error::BinParse)?;
    let size = u32::from_be_bytes(size_bytes);
    let pos_bytes: [u8; 8] = data.get(OFFSET_ATT_POS..OFFSET_ATT_POS + LEN_ATT_POS)
        .and_then(|s| s.try_into().ok())
        .ok_or(kai_ws::Error::BinParse)?;
    let pos = u64::from_be_bytes(pos_bytes);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_att_header(id: u32, size: u32, pos: u64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(28);
        // kai-ws binary header: 12 bytes (sn 4 + payload_type 4 + status_code 4)
        buf.extend_from_slice(&0u32.to_be_bytes());  // sn
        buf.extend_from_slice(&0u32.to_be_bytes());  // payload_type
        buf.extend_from_slice(&200u32.to_be_bytes()); // status_code
        // attachment header: id 4 + size 4 + pos 8
        buf.extend_from_slice(&id.to_be_bytes());
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(&pos.to_be_bytes());
        buf
    }

    #[test]
    fn test_parse_attachment_header_ok() {
        let data = make_att_header(42, 1024, 65536);
        let header = parse_attachment_payload_header(&data).unwrap();
        assert_eq!(header.id, 42);
        assert_eq!(header.size, 1024);
        assert_eq!(header.pos, 65536);
    }

    #[test]
    fn test_parse_attachment_header_too_short() {
        let data = vec![0u8; 10];
        let result = parse_attachment_payload_header(&data);
        assert!(matches!(result, Err(kai_ws::Error::BinParse)));
    }
}
