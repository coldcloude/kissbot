use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ========== 消息类型常量 ==========

pub const MSG_TYPE_TEXT: &str = "text";
pub const MSG_TYPE_IMAGE: &str = "image";
pub const MSG_TYPE_FILE: &str = "file";
pub const MSG_TYPE_SYSTEM_JOIN: &str = "system_join";
pub const MSG_TYPE_SYSTEM_LEAVE: &str = "system_leave";
pub const MSG_TYPE_MULTI: &str = "multi";

// ========== Multi 消息 ==========

/// multi 消息的 content 为 JSON 列表，每个元素包含 msg_type 和 content 两个字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiMessageItem {
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
}
