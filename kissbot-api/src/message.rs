use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ========== 消息类型常量 ==========

pub const MSG_TYPE_TEXT: &str = "text";
// 以下类型已由 MSG_TYPE_ATTACHMENT + mime_type 替代：
// pub const MSG_TYPE_IMAGE: &str = "image";
// pub const MSG_TYPE_FILE: &str = "file";
pub const MSG_TYPE_ATTACHMENT: &str = "attachment";
pub const MSG_TYPE_SYSTEM_JOIN: &str = "system_join";
pub const MSG_TYPE_SYSTEM_LEAVE: &str = "system_leave";
pub const MSG_TYPE_MULTI: &str = "multi";

// ========== Multi 消息 ==========

/// multi 消息的 content 为 JSON 列表，每个元素包含 msg_type 和 content 两个字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageItem {
    pub msg_type: Arc<String>,
    pub content: Arc<Value>,
}

// ========== 附件消息相关类型 ==========

/// 附件信息。含 key 时表示已由 channel 处理后嵌入 key。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub att_id: Arc<String>,
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
}

/// 附件响应。channel 在处理后为 AttachmentInfo 附加生成的 key。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfoResponse {
    pub key: Arc<String>,
    pub info: Arc<AttachmentInfo>,
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
                content: Arc::new(serde_json::Value::String(format!("content for {}", msg_type))),
            };
            let json = serde_json::to_value(&item).unwrap();
            let deserialized: MessageItem = serde_json::from_value(json).unwrap();
            assert_eq!(*deserialized.msg_type, msg_type);
        }
    }
}
