use std::sync::Arc;

use kissbot_api::channel::*;
use kissbot_api::message::*;
use serde::{Deserialize, Serialize};

// ========== Group Change ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChangeEvent {
    pub msg_id: Arc<String>,
    pub notification: Arc<GroupChangeNotification>,
    pub change_type: GroupChangeType,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GroupChangeType {
    Joined,
    Left,
}

// ========== User Remove ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRemoveEvent {
    pub msg_id: Arc<String>,
    pub notification: Arc<UserRemoveNotification>,
    pub time: Arc<String>,
}

/// 统一的 GroupChange → IncomingMessageEvent 转换。
pub fn group_change_to_incoming_message(message: Arc<GroupChangeEvent>) -> Arc<IncomingMessage> {
    let msg_type = match message.change_type {
        GroupChangeType::Joined => MSG_TYPE_SYSTEM_GROUP_JOIN,
        GroupChangeType::Left => MSG_TYPE_SYSTEM_GROUP_LEAVE,
    };
    Arc::new(IncomingMessage {
        msg_id: message.msg_id.clone(),
        messenger_id: message.notification.messenger_id.clone(),
        user_id: message.notification.user_id.clone(),
        group_id: message.notification.group_id.clone(),
        is_self: 1,
        msg_type: Arc::new(msg_type.to_string()),
        content: Content::GroupChange(message.notification.clone()),
        time: message.time.clone(),
    })
}
