use std::collections::HashMap;
use std::sync::Arc;

use arcstr::ArcStr;
use kai_file::index::Record;
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
    pub group_id: ArcStr,
    pub group_name: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: ArcStr,
    pub user_name: ArcStr,
    pub group_map: Arc<HashMap<ArcStr, Arc<GroupInfo>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerInfo {
    pub messenger_id: ArcStr,
    pub messenger_name: ArcStr,
    pub user_map: Arc<HashMap<ArcStr, Arc<UserInfo>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerInfoRequest {
    pub messenger_id: ArcStr,
}

/// 通道用户标识（消息方身份：messenger_id + user_id）
///
/// 内部字段用 `ArcStr`，并实现 `PartialEq` / `Hash`，因此 `Arc<ChannelUser>`
/// 可直接作为 Set / Map 的键。
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct ChannelUser {
    pub messenger_id: ArcStr,
    pub user_id: ArcStr,
}

// ========================= Message & Attachment ==========================

use crate::message::Content;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub messenger_id: ArcStr,
    pub user_id: ArcStr,
    pub group_id: ArcStr,
    pub content: Content,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageResponse {
    pub msg_id: ArcStr,
    pub time: ArcStr,
    pub content: Content,  // 转换后的 content（已嵌入 key）
    pub messenger_name: ArcStr,
    pub user_name: ArcStr,
    pub group_name: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadRequest {
    pub messenger_id: ArcStr,
    pub user_id: ArcStr,
    pub group_id: ArcStr,
    pub key: ArcStr,
}

/// Agent 对 attachment payload chunk 的确认 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentPayloadResponse {
    pub current_pos: u64,
    pub error_code: u32,
    pub error_msg: Option<ArcStr>,
}

pub const PAYLOAD_ERRCODE_OK: u32 = 0;
pub const PAYLOAD_ERRCODE_POSITION_OUT_OF_ORDER: u32 = 1;

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
    pub msg_id: ArcStr,
    pub messenger_id: ArcStr,
    pub user_id: ArcStr,
    pub group_id: ArcStr,
    pub messenger_name: ArcStr,
    pub user_name: ArcStr,
    pub group_name: ArcStr,
    pub content: Content,
    pub time: ArcStr,
}

/// 通道内统一的消息分发事件。recipient_user_id 为接收者（用于 bound_map）。
/// incoming_message.user_id 为**发送者**。两者不同时表示转发（如 admin → agent）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessageEvent {
    pub recipient_user_id: ArcStr,
    pub incoming_message: Arc<IncomingMessage>,
}

impl Record for IncomingMessage {
    fn time(&self) -> &str {
        self.time.as_str()
    }
}

// ========== Query & Bind ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindRequest {
    pub messenger_id: ArcStr,
    pub user_id: ArcStr,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{AttachmentInfo, AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};

    #[test]
    fn arc_channel_user_works_as_set_key() {
        // 内部字段用 ArcStr，结构体实现 PartialEq + Hash，Arc<ChannelUser> 可直接作 Set / Map 键
        use std::collections::HashSet;
        let a = Arc::new(ChannelUser { messenger_id: ArcStr::from("web"), user_id: ArcStr::from("u1") });
        let b = Arc::new(ChannelUser { messenger_id: ArcStr::from("web"), user_id: ArcStr::from("u1") });
        let c = Arc::new(ChannelUser { messenger_id: ArcStr::from("web"), user_id: ArcStr::from("u2") });
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b), "等值 Arc<ChannelUser> 应命中 HashSet");
        assert!(!set.contains(&c), "不同用户不应命中");
    }

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
            messenger_id: ArcStr::from("messenger1"),
            group_id: ArcStr::from("group1"),
            user_id: ArcStr::from("user1"),
            messenger_name: ArcStr::from("Messenger1Name"),
            user_name: ArcStr::from("User1Name"),
            group_name: ArcStr::from("Group1Name"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GroupChangeNotification = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.messenger_id, "messenger1");
        assert_eq!(deserialized.group_id, "group1");
        assert_eq!(deserialized.user_id, "user1");
        assert_eq!(deserialized.messenger_name, "Messenger1Name");
        assert_eq!(deserialized.user_name, "User1Name");
        assert_eq!(deserialized.group_name, "Group1Name");
    }

    #[test]
    fn test_serde_user_remove_notification() {
        let obj = UserRemoveNotification {
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UserRemoveNotification = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.messenger_id, "m1");
        assert_eq!(deserialized.user_id, "u1");
        assert_eq!(deserialized.messenger_name, "M1Name");
        assert_eq!(deserialized.user_name, "U1Name");
    }

    #[test]
    fn test_serde_messenger_info() {
        let group_info = Arc::new(GroupInfo {
            group_id: ArcStr::from("g1"),
            group_name: ArcStr::from("MyGroup"),
        });
        let user_info = Arc::new(UserInfo {
            user_id: ArcStr::from("u1"),
            user_name: ArcStr::from("Alice"),
            group_map: {
                let mut gm = HashMap::new();
                gm.insert(ArcStr::from("g1"), group_info);
                Arc::new(gm)
            },
        });
        let mut user_map = HashMap::new();
        user_map.insert(ArcStr::from("u1"), user_info);
        let user_map = Arc::new(user_map);

        let obj = MessengerInfo {
            messenger_id: ArcStr::from("m1"),
            messenger_name: ArcStr::from("Telegram"),
            user_map,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: MessengerInfo = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.messenger_id, "m1");
        assert_eq!(deserialized.messenger_name, "Telegram");
        assert_eq!(deserialized.user_map.len(), 1);
    }

    #[test]
    fn test_serde_messenger_info_request() {
        let obj = MessengerInfoRequest {
            messenger_id: ArcStr::from("m1"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: MessengerInfoRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.messenger_id, "m1");
    }

    #[test]
    fn test_serde_outgoing_message() {
        let att_info = Arc::new(AttachmentInfo {
            file_name: ArcStr::from("photo.png"),
            mime_type: ArcStr::from("image/png"),
            size_bytes: 12345,
        });
        let content = Content::AttachmentInfo(att_info.clone());

        let obj = OutgoingMessage {
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
            group_id: ArcStr::from("g1"),
            content,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: OutgoingMessage = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.messenger_id, "m1");
    }

    #[test]
    fn test_serde_outgoing_message_response() {
        let content = Content::Text(ArcStr::from("response content"));

        let obj = OutgoingMessageResponse {
            msg_id: ArcStr::from("msg1"),
            time: ArcStr::from("2026-01-01 00:00:00"),
            content,
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
            group_name: ArcStr::from("G1Name"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: OutgoingMessageResponse = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.msg_id, "msg1");
        assert_eq!(deserialized.messenger_name, "M1Name");
        assert_eq!(deserialized.user_name, "U1Name");
        assert_eq!(deserialized.group_name, "G1Name");
        assert_eq!(deserialized.content, Content::Text(ArcStr::from("response content")));
    }

    #[test]
    fn test_serde_attachment_download_request() {
        let obj = AttachmentDownloadRequest {
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
            group_id: ArcStr::from("g1"),
            key: ArcStr::from("key1"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: AttachmentDownloadRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.key, "key1");
    }

    #[test]
    fn test_serde_attachment_download_response_header() {
        let metadata = Arc::new(AttachmentInfo {
            file_name: ArcStr::from("doc.pdf"),
            mime_type: ArcStr::from("application/pdf"),
            size_bytes: 99999,
        });
        let response = Arc::new(AttachmentInfoResponse {
            key: ArcStr::from("g1/msg1/doc.pdf"),
            info: metadata,
            transfer_id: 42,
        });
        let json = serde_json::to_value(&response).unwrap();
        let deserialized: Arc<AttachmentInfoResponse> = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.key, "g1/msg1/doc.pdf");
        assert_eq!(deserialized.info.file_name, "doc.pdf");
        assert_eq!(deserialized.transfer_id, 42);
    }

    #[test]
    fn test_serde_incoming_message() {
        let obj = IncomingMessage {
            msg_id: ArcStr::from("msg1"),
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
            group_id: ArcStr::from("g1"),
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
            group_name: ArcStr::from("G1Name"),
            content: Content::Text(ArcStr::from("Hello")),
            time: ArcStr::from("2026-01-01 00:00:00"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: IncomingMessage = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.msg_id, "msg1");
        assert_eq!(deserialized.messenger_name, "M1Name");
        assert_eq!(deserialized.user_name, "U1Name");
        assert_eq!(deserialized.group_name, "G1Name");
        assert_eq!(deserialized.content, Content::Text(ArcStr::from("Hello")));
    }

    #[test]
    fn test_serde_incoming_message_event() {
        let incoming = Arc::new(IncomingMessage {
            msg_id: ArcStr::from("msg1"),
            messenger_id: ArcStr::from("web"),
            user_id: ArcStr::from("u2"),
            group_id: ArcStr::from("g1"),
            messenger_name: ArcStr::from("Web"),
            user_name: ArcStr::from("User2"),
            group_name: ArcStr::from("Group1"),
            content: Content::Text(ArcStr::from("hello")),
            time: ArcStr::from("2026-01-01 00:00:00"),
        });
        let obj = IncomingMessageEvent {
            recipient_user_id: ArcStr::from("u1"),
            incoming_message: incoming,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: IncomingMessageEvent = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.recipient_user_id, "u1");
        assert_eq!(deserialized.incoming_message.user_id, "u2");
    }

    #[test]
    fn test_serde_bind_request() {
        let obj = BindRequest {
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: BindRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.messenger_id, "m1");
        assert_eq!(deserialized.user_id, "u1");
    }
}
