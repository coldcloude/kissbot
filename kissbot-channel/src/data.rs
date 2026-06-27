use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::DataWriter;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::error::Result;

// ========== Type aliases (kissbot_api 已直接使用 Arc<String> / Arc<DashMap<>>) ==========

pub type IncomingMessages = Vec<Arc<IncomingMessage>>;

// ========== Events ==========

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

// ========== Attachment ==========

#[async_trait]
pub trait AttachmentDownloadPayloadSender: Send + Sync {
    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, writer: Arc<dyn DataWriter<Error>>) -> Result<()>;
}

// ========== Group Change ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChangeEvent {
    pub msg_id: Arc<String>,
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

// ========== User Remove ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRemoveEvent {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
}

#[async_trait]
pub trait UserRemoveHandler: Send + Sync {
    async fn handle_user_remove(&self, event: Arc<UserRemoveEvent>);
}

/// 统一的 GroupChange → IncomingMessageEvent 转换。
pub fn group_change_to_incoming_message(message: Arc<GroupChangeEvent>) -> Arc<IncomingMessageEvent> {
    let msg_type = match message.change_type {
        GroupChangeType::Joined => MSG_TYPE_SYSTEM_JOIN,
        GroupChangeType::Left => MSG_TYPE_SYSTEM_LEAVE,
    };
    let incoming = Arc::new(IncomingMessage {
        msg_id: message.msg_id.clone(),
        messenger_id: message.messenger_id.clone(),
        user_id: message.user_id.clone(),
        group_id: message.group_id.clone(),
        is_self: 1,
        msg_type: Arc::new(msg_type.to_string()),
        content: Arc::new(String::new()),
        time: message.time.clone(),
    });
    Arc::new(IncomingMessageEvent {
        messenger_id: message.messenger_id.clone(),
        user_id: message.user_id.clone(),
        group_id: message.group_id.clone(),
        messages: Arc::new(vec![incoming]),
    })
}
