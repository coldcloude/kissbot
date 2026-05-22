use std::sync::Arc;

use async_trait::async_trait;
use kissbot_api::{SyncMap, SyncString, channel::*};
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

use crate::Result;

// ========== Messenger -> User -> Group -> Channel ==========

pub type ChannelInfo = ChannelInfoGeneric<SyncString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChannelInfo;

impl ChannelInfoKind for SyncChannelInfo {
    type Type = Arc<ChannelInfo>;
}

pub type GroupInfo = GroupInfoGeneric<SyncString, SyncChannelInfo>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncGroupInfo;

impl GroupInfoKind for SyncGroupInfo {
    type Type = Arc<GroupInfo>;
}

pub type UserInfo = UserInfoGeneric<SyncString, SyncMap, SyncGroupInfo>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncUserInfo;

impl UserInfoKind for SyncUserInfo {
    type Type = Arc<UserInfo>;
}

pub type MessengerInfo = MessengerInfoGeneric<SyncString, SyncMap, SyncUserInfo>;

// ========== Message & Attachment ==========

pub type AttachmentInfo = AttachmentInfoGeneric<SyncString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncAttachmentInfo;

impl AttachmentInfoKind for SyncAttachmentInfo {
    type Type = Arc<AttachmentInfo>;
}

pub type OutgoingMessageResponse = OutgoingMessageResponseGeneric<SyncString, SyncMap>;

pub type AttachmentDownloadResponseHeader = AttachmentDownloadResponseHeaderGeneric<SyncAttachmentInfo>;

#[async_trait]
pub trait AttachmentDownloadResponsePayloadSender: Send + Sync {
    async fn send_attachment_payload(&self, data: &[u8]) -> Result<()>;
}

// ========== Receiving Message ==========
pub type IncomingMessage = IncomingMessageGeneric<SyncString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessageEvent {
    pub messages: Vec<IncomingMessage>,
}

#[async_trait]
pub trait IncomingMessageSender: Send + Sync {
    async fn send_incoming_messages(&self, event: Arc<IncomingMessageEvent>) -> Result<()>;
}

// ========== Group Change ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChangeEvent {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
    pub change_type: GroupChangeType,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GroupChangeType {
    Joined,
    Left,
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
