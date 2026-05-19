use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

// ========== User and Group Info ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: String,
    pub user_name: String,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfo {
    pub group_id: String,
    pub group_name: String,
    pub group_type: String,
}

// ========== Group Change ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChangeEvent {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
    pub group_name: String,
    pub change_type: GroupChangeType,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GroupChangeType {
    Joined,
    Left,
}

// ========== Attachment ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub key: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
}

// ========== Message Record ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub channel_id: String,
    pub user_id: String,
    pub is_self: usize,
    pub msg_type: String,
    pub content: String,
    pub attachments: Vec<Attachment>,
    pub timestamp: DateTime<Utc>,
}

// ========== Outgoing Message ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub channel_id: String,
    pub target_user_id: Option<String>,
    pub msg_type: String,
    pub content: String,
    pub attachments: Vec<Attachment>,
}

// ========== Channel Status ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelStatus {
    pub channel_id: String,
    pub messenger_id: String,
    pub group_id: String,
    pub user_id: String,
    pub agent_id: String,
    pub is_running: bool,
}

// ========== Channel Info ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub channel_id: String,
    pub messenger_id: String,
    pub group_id: String,
    pub group_name: String,
    pub user_id: String,
}

// ========== WSS Messages ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WssMessage {
    pub r#type: String,
    pub data: serde_json::Value,
}

// Agent -> Channel messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageData {
    pub channel_id: String,
    pub msg_type: String,
    pub content: String,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindData {
    pub agent_id: String,
    pub messenger_id: String,
    pub user_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadData {
    pub key: String,
}

// Channel -> Agent messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessageData {
    pub channel_id: String,
    pub user_id: String,
    pub is_self: usize,
    pub msg_type: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindAckData {
    pub agent_id: String,
    pub channels: Vec<ChannelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelsData {
    pub channels: Vec<ChannelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChangeData {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
    pub group_name: String,
    pub change_type: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentData {
    pub key: String,
    pub filename: String,
    pub mime_type: String,
    pub data: String, // base64 encoded
}

// Helper functions
pub fn format_channel_id(messenger_id: &str, group_id: &str, user_id: &str) -> String {
    format!("{}:{}:{}", messenger_id, group_id, user_id)
}
