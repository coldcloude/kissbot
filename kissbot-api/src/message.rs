use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ========== 消息类型常量 ==========

pub const MSG_TYPE_TEXT: &str = "text";
pub const MSG_TYPE_ATTACHMENT: &str = "attachment";
pub const MSG_TYPE_SYSTEM_GROUP_JOIN: &str = "system_group_join";
pub const MSG_TYPE_SYSTEM_GROUP_LEAVE: &str = "system_group_leave";
pub const MSG_TYPE_USER_REMOVE: &str = "user_remove";
pub const MSG_TYPE_MULTI: &str = "multi";

// ========== 通知类型 ==========

/// 群组变更通知
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupChangeNotification {
    pub messenger_id: Arc<String>,
    pub group_id: Arc<String>,
    pub user_id: Arc<String>,
}

/// 用户移除通知
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRemoveNotification {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
}

// ========== 内容类型枚举 ==========

/// content 的类型化表示，替代任意的 serde_json::Value。
/// 各变体对应不同的 msg_type。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Content {
    Text(String),
    Multi(Vec<Arc<MessageItem>>),
    AttachmentInfo(AttachmentInfo),
    AttachmentInfoResponse(AttachmentInfoResponse),
    GroupChange(GroupChangeNotification),
    UserRemove(UserRemoveNotification),
}

// ========== Multi 消息 ==========

/// multi 消息的 content 为 JSON 列表，每个元素包含 msg_type 和 content 两个字段
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageItem {
    pub msg_type: Arc<String>,
    pub content: Arc<Content>,
}

// ========== 附件消息相关类型 ==========

/// 附件信息。含 key 时表示已由 channel 处理后嵌入 key。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub att_id: Arc<String>,
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
}

/// 附件响应。channel 在处理后为 AttachmentInfo 附加生成的 key。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfoResponse {
    pub key: Arc<String>,
    pub info: Arc<AttachmentInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_content(msg_type: &str) -> Content {
        match msg_type {
            MSG_TYPE_TEXT => Content::Text("hello".to_string()),
            MSG_TYPE_ATTACHMENT => Content::AttachmentInfo(AttachmentInfo {
                att_id: Arc::new("att1".to_string()),
                file_name: Arc::new("photo.png".to_string()),
                mime_type: Arc::new("image/png".to_string()),
                size_bytes: 1024,
            }),
            MSG_TYPE_SYSTEM_GROUP_JOIN | MSG_TYPE_SYSTEM_GROUP_LEAVE => {
                Content::GroupChange(GroupChangeNotification {
                    messenger_id: Arc::new("m1".to_string()),
                    group_id: Arc::new("g1".to_string()),
                    user_id: Arc::new("u1".to_string()),
                })
            }
            MSG_TYPE_USER_REMOVE => Content::UserRemove(UserRemoveNotification {
                messenger_id: Arc::new("m1".to_string()),
                user_id: Arc::new("u1".to_string()),
            }),
            MSG_TYPE_MULTI => Content::Multi(vec![
                Arc::new(MessageItem {
                    msg_type: Arc::new(MSG_TYPE_TEXT.to_string()),
                    content: Arc::new(Content::Text("nested".to_string())),
                }),
            ]),
            _ => Content::Text(format!("unknown: {}", msg_type)),
        }
    }

    #[test]
    fn test_serde_content_text() {
        let content = Content::Text("hello".to_string());
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, serde_json::json!({"Text": "hello"}));
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_attachment_info() {
        let info = AttachmentInfo {
            att_id: Arc::new("att1".to_string()),
            file_name: Arc::new("photo.png".to_string()),
            mime_type: Arc::new("image/png".to_string()),
            size_bytes: 1024,
        };
        let content = Content::AttachmentInfo(info);
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_attachment_info_response() {
        let info = Arc::new(AttachmentInfo {
            att_id: Arc::new("att1".to_string()),
            file_name: Arc::new("photo.png".to_string()),
            mime_type: Arc::new("image/png".to_string()),
            size_bytes: 1024,
        });
        let content = Content::AttachmentInfoResponse(AttachmentInfoResponse {
            key: Arc::new("g1/msg1/photo.png".to_string()),
            info,
        });
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_group_change() {
        let content = Content::GroupChange(GroupChangeNotification {
            messenger_id: Arc::new("m1".to_string()),
            group_id: Arc::new("g1".to_string()),
            user_id: Arc::new("u1".to_string()),
        });
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_user_remove() {
        let content = Content::UserRemove(UserRemoveNotification {
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
        });
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_multi() {
        let content = Content::Multi(vec![
            Arc::new(MessageItem {
                msg_type: Arc::new(MSG_TYPE_TEXT.to_string()),
                content: Arc::new(Content::Text("hello".to_string())),
            }),
        ]);
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_message_item_roundtrip() {
        let types = [
            MSG_TYPE_TEXT,
            MSG_TYPE_ATTACHMENT,
            MSG_TYPE_SYSTEM_GROUP_JOIN,
            MSG_TYPE_SYSTEM_GROUP_LEAVE,
            MSG_TYPE_USER_REMOVE,
            MSG_TYPE_MULTI,
        ];
        for msg_type in types {
            let content = make_content(msg_type);
            let item = MessageItem {
                msg_type: Arc::new(msg_type.to_string()),
                content: Arc::new(content),
            };
            let json = serde_json::to_value(&item).unwrap();
            let deserialized: MessageItem = serde_json::from_value(json).unwrap();
            assert_eq!(*deserialized.msg_type, msg_type);
            assert_eq!(*deserialized.content, *item.content);
        }
    }
}
