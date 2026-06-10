use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::{SyncMap, SyncString, channel::*};
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ========== Messenger -> User -> Group -> Channel ==========

pub type ChannelInfo = ChannelInfoGeneric<SyncString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChannelInfo;

impl ChannelInfoKind for SyncChannelInfo {
    type Type = Arc<ChannelInfo>;
}

pub type GroupInfo = GroupInfoGeneric<SyncString>;

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
pub trait AttachmentDownloadPayloadSender: Send + Sync {
    async fn send_attachment_payload(&self, data: Bytes) -> Result<()>;
}

// ========== Receiving Message ==========
pub type IncomingMessage = IncomingMessageGeneric<SyncString>;

pub type IncomingMessages = Vec<Arc<IncomingMessage>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessageEvent {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub messages: Arc<IncomingMessages>,
}

#[async_trait]
pub trait IncomingMessageHandler: Send + Sync {
    async fn handle_incoming_message(&self, event: Arc<IncomingMessageEvent>);
}

// ========== Group Change ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChangeEvent {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub change_type: GroupChangeType,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GroupChangeType {
    Joined,
    Left,
}

#[async_trait]
pub trait GroupChangeHandler: Send + Sync {
    async fn handle_group_change(&self, event: Arc<GroupChangeEvent>);
}
