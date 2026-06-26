use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ========== 消息类型常量 ==========

pub const MSG_TYPE_TEXT: &str = "text";
pub const MSG_TYPE_IMAGE: &str = "image";       // 已弃用，将在 Task 4 中移除
pub const MSG_TYPE_FILE: &str = "file";         // 已弃用，将在 Task 4 中移除
pub const MSG_TYPE_ATTACHMENT: &str = "attachment";
pub const MSG_TYPE_SYSTEM_JOIN: &str = "system_join";
pub const MSG_TYPE_SYSTEM_LEAVE: &str = "system_leave";
pub const MSG_TYPE_MULTI: &str = "multi";

// ========== Multi 消息 ==========

/// multi 消息的 content 为 JSON 列表，每个元素包含 msg_type 和 content 两个字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageItem {
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_message_item() {
        let types = [MSG_TYPE_TEXT, MSG_TYPE_ATTACHMENT, MSG_TYPE_SYSTEM_JOIN, MSG_TYPE_SYSTEM_LEAVE, MSG_TYPE_MULTI];
        for msg_type in types {
            let item = MessageItem {
                msg_type: Arc::new(msg_type.to_string()),
                content: Arc::new(format!("content for {}", msg_type)),
            };
            let json = serde_json::to_value(&item).unwrap();
            let deserialized: MessageItem = serde_json::from_value(json).unwrap();
            assert_eq!(*deserialized.msg_type, msg_type);
        }
    }
}
