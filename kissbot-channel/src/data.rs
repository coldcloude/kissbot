use std::sync::Arc;

use kissbot_api::channel::*;
use kissbot_api::message::*;
use serde::{Deserialize, Serialize};

// ========== IncomingMessageEvent ==========

/// 通道内统一的消息分发事件。recipient_user_id 为接收者（用于 bound_map）。
/// incoming_message.user_id 为**发送者**。两者不同时表示转发（如 admin → agent）。
#[derive(Debug, Clone)]
pub struct IncomingMessageEvent {
    pub recipient_user_id: Arc<String>,
    pub incoming_message: Arc<IncomingMessage>,
}

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
pub fn group_change_to_incoming_message_event(message: Arc<GroupChangeEvent>) -> Arc<IncomingMessageEvent> {
    let content = match message.change_type {
        GroupChangeType::Joined => Content::GroupJoin(message.notification.clone()),
        GroupChangeType::Left => Content::GroupLeave(message.notification.clone()),
    };
    let incoming = Arc::new(IncomingMessage {
        msg_id: message.msg_id.clone(),
        messenger_id: message.notification.messenger_id.clone(),
        user_id: message.notification.user_id.clone(),
        group_id: message.notification.group_id.clone(),
        is_self: 1,
        messenger_name: message.notification.messenger_name.clone(),
        user_name: message.notification.user_name.clone(),
        group_name: message.notification.group_name.clone(),
        content,
        time: message.time.clone(),
    });
    // 接收者即被通知的用户
    Arc::new(IncomingMessageEvent {
        recipient_user_id: message.notification.user_id.clone(),
        incoming_message: incoming,
    })
}
