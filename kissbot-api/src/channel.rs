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

use crate::message::{AttachmentInfoResponse, Content};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub msg_type: Arc<String>,
    pub content: Content,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub msg_type: Arc<String>,
    pub content: Content,  // 转换后的 content（已嵌入 key）
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadRequest {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub key: Arc<String>,
}

/// ChannelManager 返回给 agent 的 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsOutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub msg_type: Arc<String>,
    pub content: Content,
    /// key → internal_upload_id 映射，agent 上传 attachment payload 时使用
    pub attachment_upload_id_map: Arc<DashMap<String, u32>>,
}

/// ChannelManager 返回给 agent 的下载 response，附加下载 id
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsAttachmentDownloadResponseHeader {
    pub download_id: u32,
    pub response: Arc<AttachmentInfoResponse>,
}

/// Agent 对 attachment payload chunk 的确认 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadAttachmentPayloadResponse {
    pub key: Arc<String>,
    pub error_code: u32,
    pub error_msg: Option<Arc<String>>,
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
    pub content: Content,
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
    use crate::message::{GroupChangeNotification, UserRemoveNotification};
    use crate::{AttachmentInfo, MSG_TYPE_ATTACHMENT};

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

    // === Roundtrip tests ===

    #[test]
    fn test_serde_group_change_notification() {
        let obj = GroupChangeNotification {
            messenger_id: Arc::new("messenger1".to_string()),
            group_id: Arc::new("group1".to_string()),
            user_id: Arc::new("user1".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GroupChangeNotification = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.messenger_id, "messenger1");
        assert_eq!(*deserialized.group_id, "group1");
        assert_eq!(*deserialized.user_id, "user1");
    }

    #[test]
    fn test_serde_user_remove_notification() {
        let obj = UserRemoveNotification {
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UserRemoveNotification = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.messenger_id, "m1");
        assert_eq!(*deserialized.user_id, "u1");
    }

    #[test]
    fn test_serde_messenger_info() {
        let group_info = Arc::new(GroupInfo {
            group_id: Arc::new("g1".to_string()),
            group_name: Arc::new("MyGroup".to_string()),
        });
        let user_info = Arc::new(UserInfo {
            user_id: Arc::new("u1".to_string()),
            user_name: Arc::new("Alice".to_string()),
            group_map: {
                let gm = Arc::new(DashMap::new());
                gm.insert("g1".to_string(), group_info);
                gm
            },
        });
        let user_map = Arc::new(DashMap::new());
        user_map.insert("u1".to_string(), user_info);

        let obj = MessengerInfo {
            messenger_id: Arc::new("m1".to_string()),
            messenger_name: Arc::new("Telegram".to_string()),
            user_map,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: MessengerInfo = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.messenger_id, "m1");
        assert_eq!(*deserialized.messenger_name, "Telegram");
        assert_eq!(deserialized.user_map.len(), 1);
    }

    #[test]
    fn test_serde_messenger_info_request() {
        let obj = MessengerInfoRequest {
            messenger_id: Arc::new("m1".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: MessengerInfoRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.messenger_id, "m1");
    }

    #[test]
    fn test_serde_outgoing_message() {
        let att_info = Arc::new(AttachmentInfo {
            file_name: Arc::new("photo.png".to_string()),
            mime_type: Arc::new("image/png".to_string()),
            size_bytes: 12345,
        });
        let content = Content::AttachmentInfo(att_info.clone());

        let obj = OutgoingMessage {
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
            content,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: OutgoingMessage = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.messenger_id, "m1");
        assert_eq!(*deserialized.msg_type, MSG_TYPE_ATTACHMENT);
    }

    #[test]
    fn test_serde_outgoing_message_response() {
        let content = Content::Text(Arc::new("response content".to_string()));

        let obj = OutgoingMessageResponse {
            msg_id: Arc::new("msg1".to_string()),
            msg_type: Arc::new("text".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
            content,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: OutgoingMessageResponse = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.msg_id, "msg1");
        assert_eq!(*deserialized.msg_type, "text");
        assert_eq!(deserialized.content, Content::Text(Arc::new("response content".to_string())));
    }

    #[test]
    fn test_serde_attachment_download_request() {
        let obj = AttachmentDownloadRequest {
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            key: Arc::new("key1".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: AttachmentDownloadRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.key, "key1");
    }

    #[test]
    fn test_serde_attachment_download_response_header() {
        let metadata = Arc::new(AttachmentInfo {
            file_name: Arc::new("doc.pdf".to_string()),
            mime_type: Arc::new("application/pdf".to_string()),
            size_bytes: 99999,
        });
        let response = Arc::new(AttachmentInfoResponse {
            key: Arc::new("g1/msg1/doc.pdf".to_string()),
            info: metadata,
        });
        let json = serde_json::to_value(&response).unwrap();
        let deserialized: Arc<AttachmentInfoResponse> = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.key, "g1/msg1/doc.pdf");
        assert_eq!(*deserialized.info.file_name, "doc.pdf");
    }

    #[test]
    fn test_serde_incoming_message() {
        let obj = IncomingMessage {
            msg_id: Arc::new("msg1".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Content::Text(Arc::new("Hello".to_string())),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: IncomingMessage = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.msg_id, "msg1");
        assert_eq!(deserialized.content, Content::Text(Arc::new("Hello".to_string())));
    }

    #[test]
    fn test_serde_bind_request() {
        let obj = BindRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("admin".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: BindRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.role_name, "admin");
    }
}
