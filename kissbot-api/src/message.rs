use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ========== 通知类型 ==========

/// 群组变更通知
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupChangeNotification {
    pub messenger_id: Arc<String>,
    pub group_id: Arc<String>,
    pub user_id: Arc<String>,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
    pub group_name: Arc<String>,
}

/// 用户移除通知
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRemoveNotification {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
}

// ========== 内容类型枚举 ==========

/// content 的类型化表示。
/// 各变体自身即携带类型信息，无需外部 msg_type 字段。
/// 序列化格式：{"msg_type": "Text", "data": "hello"}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "msg_type", content = "data")]
pub enum Content {
    Text(Arc<String>),
    Multi(Vec<Content>),
    AttachmentInfo(Arc<AttachmentInfo>),
    AttachmentInfoResponse(Arc<AttachmentInfoResponse>),
    GroupJoin(Arc<GroupChangeNotification>),
    GroupLeave(Arc<GroupChangeNotification>),
    UserRemove(Arc<UserRemoveNotification>),
    // 新增三变体：内容为 key（UUID），关联对应详情记录（agent 直接生成 ChannelRecord，不用于 IncomingMessage/OutgoingMessage）
    Think(Arc<String>),
    ToolCall(Arc<String>),
    ToolResult(Arc<String>),
}

// ========== 附件消息相关类型 ==========

/// 附件信息。含 key 时表示已由 channel 处理后嵌入 key。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
}

/// 附件响应。channel 在处理后为 AttachmentInfo 附加生成的 key。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfoResponse {
    pub key: Arc<String>,
    pub info: Arc<AttachmentInfo>,
    pub transfer_id: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_content_text() {
        let content = Content::Text(Arc::new("hello".to_string()));
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, serde_json::json!({"msg_type": "Text", "data": "hello"}));
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_attachment_info() {
        let info = Arc::new(AttachmentInfo {
            file_name: Arc::new("photo.png".to_string()),
            mime_type: Arc::new("image/png".to_string()),
            size_bytes: 1024,
        });
        let content = Content::AttachmentInfo(info);
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_attachment_info_response() {
        let info = Arc::new(AttachmentInfo {
            file_name: Arc::new("photo.png".to_string()),
            mime_type: Arc::new("image/png".to_string()),
            size_bytes: 1024,
        });
        let content = Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
            key: Arc::new("g1/msg1/photo.png".to_string()),
            info,
            transfer_id: 42,
        }));
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_group_join() {
        let content = Content::GroupJoin(Arc::new(GroupChangeNotification {
            messenger_id: Arc::new("m1".to_string()),
            group_id: Arc::new("g1".to_string()),
            user_id: Arc::new("u1".to_string()),
            messenger_name: Arc::new("M1Name".to_string()),
            user_name: Arc::new("U1Name".to_string()),
            group_name: Arc::new("G1Name".to_string()),
        }));
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_group_leave() {
        let content = Content::GroupLeave(Arc::new(GroupChangeNotification {
            messenger_id: Arc::new("m1".to_string()),
            group_id: Arc::new("g1".to_string()),
            user_id: Arc::new("u1".to_string()),
            messenger_name: Arc::new("M1Name".to_string()),
            user_name: Arc::new("U1Name".to_string()),
            group_name: Arc::new("G1Name".to_string()),
        }));
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_user_remove() {
        let content = Content::UserRemove(Arc::new(UserRemoveNotification {
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            messenger_name: Arc::new("M1Name".to_string()),
            user_name: Arc::new("U1Name".to_string()),
        }));
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_multi() {
        let content = Content::Multi(vec![
            Content::Text(Arc::new("hello".to_string())),
        ]);
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_think_tool_variants() {
        let think = Content::Think(Arc::new("uuid-1".to_string()));
        let json = serde_json::to_value(&think).unwrap();
        assert_eq!(json, serde_json::json!({"msg_type":"Think","data":"uuid-1"}));
        assert_eq!(serde_json::from_value::<Content>(json).unwrap(), think);

        let call = Content::ToolCall(Arc::new("uuid-2".to_string()));
        let j = serde_json::to_value(&call).unwrap();
        assert_eq!(j["msg_type"], "ToolCall");
        assert_eq!(j["data"], "uuid-2");

        let result = Content::ToolResult(Arc::new("uuid-3".to_string()));
        let j = serde_json::to_value(&result).unwrap();
        assert_eq!(j["msg_type"], "ToolResult");
        assert_eq!(j["data"], "uuid-3");
    }

    #[test]
    fn test_serde_message_item_roundtrip() {
        let variants = vec![
            Content::Text(Arc::new("hello".to_string())),
            Content::AttachmentInfo(Arc::new(AttachmentInfo {
                file_name: Arc::new("f.png".to_string()),
                mime_type: Arc::new("image/png".to_string()),
                size_bytes: 100,
            })),
            Content::GroupJoin(Arc::new(GroupChangeNotification {
                messenger_id: Arc::new("m1".to_string()),
                group_id: Arc::new("g1".to_string()),
                user_id: Arc::new("u1".to_string()),
                messenger_name: Arc::new("M1Name".to_string()),
                user_name: Arc::new("U1Name".to_string()),
                group_name: Arc::new("G1Name".to_string()),
            })),
            Content::GroupLeave(Arc::new(GroupChangeNotification {
                messenger_id: Arc::new("m1".to_string()),
                group_id: Arc::new("g1".to_string()),
                user_id: Arc::new("u1".to_string()),
                messenger_name: Arc::new("M1Name".to_string()),
                user_name: Arc::new("U1Name".to_string()),
                group_name: Arc::new("G1Name".to_string()),
            })),
            Content::UserRemove(Arc::new(UserRemoveNotification {
                messenger_id: Arc::new("m1".to_string()),
                user_id: Arc::new("u1".to_string()),
                messenger_name: Arc::new("M1Name".to_string()),
                user_name: Arc::new("U1Name".to_string()),
            })),
            Content::Multi(vec![
                Content::Text(Arc::new("nested".to_string())),
            ]),
        ];
        for content in variants {
            let json = serde_json::to_value(&content).unwrap();
            let deserialized: Content = serde_json::from_value(json).unwrap();
            assert_eq!(deserialized, content);
        }
    }
}
