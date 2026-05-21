use std::sync::Arc;

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use tokio::io::AsyncReadExt;

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

// ========== Message Record ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub msg_id: Arc<String>,
    pub channel_id: Arc<String>,
    pub user_id: Arc<String>,
    pub is_self: usize,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub time: Arc<String>,
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

// Helper functions
pub fn format_channel_id(messenger_id: &str, group_id: &str, user_id: &str) -> String {
    format!("{}:{}:{}", messenger_id, group_id, user_id)
}
