use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use serde::{Deserialize, Serialize};

// ========== IncomingMessageEvent ==========

/// IncomingMessageEvent 已移至 kissbot-api（kissbot_api::channel::IncomingMessageEvent，含 serde derive），
/// 见文件头部 use kissbot_api::channel::*;

// ========== Group Change ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChangeEvent {
    pub msg_id: ArcStr,
    pub notification: Arc<GroupChangeNotification>,
    pub change_type: GroupChangeType,
    pub time: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GroupChangeType {
    Joined,
    Left,
}

// ========== User Remove ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRemoveEvent {
    pub msg_id: ArcStr,
    pub notification: Arc<UserRemoveNotification>,
    pub time: ArcStr,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_change_to_incoming_passes_names() {
        let notification = Arc::new(GroupChangeNotification {
            messenger_id: ArcStr::from("web"),
            group_id: ArcStr::from("g1"),
            user_id: ArcStr::from("u1"),
            messenger_name: ArcStr::from("Web Chat"),
            user_name: ArcStr::from("用户1"),
            group_name: ArcStr::from("群组1"),
        });
        let event = Arc::new(GroupChangeEvent {
            msg_id: ArcStr::from("msg1"),
            notification: notification.clone(),
            change_type: GroupChangeType::Joined,
            time: ArcStr::from("2026-08-02 10:00:00"),
        });

        let event_msg = group_change_to_incoming_message_event(event);
        let incoming = &event_msg.incoming_message;

        // name 与通知一致
        assert_eq!(incoming.messenger_name, notification.messenger_name);
        assert_eq!(incoming.user_name, notification.user_name);
        assert_eq!(incoming.group_name, notification.group_name);
        // 消息主体字段不变
        assert_eq!(incoming.messenger_id, notification.messenger_id);
        assert_eq!(incoming.user_id, notification.user_id);
        assert_eq!(incoming.group_id, notification.group_id);
        // 内容为 GroupJoin 通知
        assert!(matches!(incoming.content.clone(), Content::GroupJoin(n) if n.as_ref() == notification.as_ref()));
        // 接收者即被通知用户
        assert_eq!(event_msg.recipient_user_id, notification.user_id);
    }
}
