use std::sync::Arc;

use arcstr::ArcStr;
use serde::{Deserialize, Serialize};

// ========== 通知类型 ==========

/// 群组变更通知
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupChangeNotification {
    pub messenger_id: ArcStr,
    pub group_id: ArcStr,
    pub user_id: ArcStr,
    pub messenger_name: ArcStr,
    pub user_name: ArcStr,
    pub group_name: ArcStr,
}

/// 用户移除通知
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRemoveNotification {
    pub messenger_id: ArcStr,
    pub user_id: ArcStr,
    pub messenger_name: ArcStr,
    pub user_name: ArcStr,
}

// ========== 内容类型枚举 ==========

/// content 的类型化表示。
/// 各变体自身即携带类型信息，无需外部 msg_type 字段。
/// 序列化格式：{"msg_type": "Text", "data": "hello"}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "msg_type", content = "data")]
pub enum Content {
    Text(ArcStr),
    Multi(Vec<Content>),
    AttachmentInfo(Arc<AttachmentInfo>),
    AttachmentInfoResponse(Arc<AttachmentInfoResponse>),
    GroupJoin(Arc<GroupChangeNotification>),
    GroupLeave(Arc<GroupChangeNotification>),
    UserRemove(Arc<UserRemoveNotification>),
    // 新增三变体：内容为 key（UUID），关联对应详情记录（agent 直接生成 ChannelRecord，不用于 IncomingMessage/OutgoingMessage）
    Think(ArcStr),
    ToolCall(ArcStr),
    ToolResult(ArcStr),
}

// ========== 附件消息相关类型 ==========

/// 附件信息。含 key 时表示已由 channel 处理后嵌入 key。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub file_name: ArcStr,
    pub mime_type: ArcStr,
    pub size_bytes: u64,
}

/// 附件响应。channel 在处理后为 AttachmentInfo 附加生成的 key。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfoResponse {
    pub key: ArcStr,
    pub info: Arc<AttachmentInfo>,
    pub transfer_id: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_content_text() {
        let content = Content::Text(ArcStr::from("hello"));
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, serde_json::json!({"msg_type": "Text", "data": "hello"}));
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_attachment_info() {
        let info = Arc::new(AttachmentInfo {
            file_name: ArcStr::from("photo.png"),
            mime_type: ArcStr::from("image/png"),
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
            file_name: ArcStr::from("photo.png"),
            mime_type: ArcStr::from("image/png"),
            size_bytes: 1024,
        });
        let content = Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
            key: ArcStr::from("g1/msg1/photo.png"),
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
            messenger_id: ArcStr::from("m1"),
            group_id: ArcStr::from("g1"),
            user_id: ArcStr::from("u1"),
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
            group_name: ArcStr::from("G1Name"),
        }));
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_group_leave() {
        let content = Content::GroupLeave(Arc::new(GroupChangeNotification {
            messenger_id: ArcStr::from("m1"),
            group_id: ArcStr::from("g1"),
            user_id: ArcStr::from("u1"),
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
            group_name: ArcStr::from("G1Name"),
        }));
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_user_remove() {
        let content = Content::UserRemove(Arc::new(UserRemoveNotification {
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
        }));
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_multi() {
        let content = Content::Multi(vec![
            Content::Text(ArcStr::from("hello")),
        ]);
        let json = serde_json::to_value(&content).unwrap();
        let deserialized: Content = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_serde_content_think_tool_variants() {
        let think = Content::Think(ArcStr::from("uuid-1"));
        let json = serde_json::to_value(&think).unwrap();
        assert_eq!(json, serde_json::json!({"msg_type":"Think","data":"uuid-1"}));
        assert_eq!(serde_json::from_value::<Content>(json).unwrap(), think);

        let call = Content::ToolCall(ArcStr::from("uuid-2"));
        let j = serde_json::to_value(&call).unwrap();
        assert_eq!(j["msg_type"], "ToolCall");
        assert_eq!(j["data"], "uuid-2");
        assert_eq!(serde_json::from_value::<Content>(j).unwrap(), call);

        let result = Content::ToolResult(ArcStr::from("uuid-3"));
        let j = serde_json::to_value(&result).unwrap();
        assert_eq!(j["msg_type"], "ToolResult");
        assert_eq!(j["data"], "uuid-3");
        assert_eq!(serde_json::from_value::<Content>(j).unwrap(), result);
    }

    #[test]
    fn test_serde_message_item_roundtrip() {
        let variants = vec![
            Content::Text(ArcStr::from("hello")),
            Content::AttachmentInfo(Arc::new(AttachmentInfo {
                file_name: ArcStr::from("f.png"),
                mime_type: ArcStr::from("image/png"),
                size_bytes: 100,
            })),
            Content::GroupJoin(Arc::new(GroupChangeNotification {
                messenger_id: ArcStr::from("m1"),
                group_id: ArcStr::from("g1"),
                user_id: ArcStr::from("u1"),
                messenger_name: ArcStr::from("M1Name"),
                user_name: ArcStr::from("U1Name"),
                group_name: ArcStr::from("G1Name"),
            })),
            Content::GroupLeave(Arc::new(GroupChangeNotification {
                messenger_id: ArcStr::from("m1"),
                group_id: ArcStr::from("g1"),
                user_id: ArcStr::from("u1"),
                messenger_name: ArcStr::from("M1Name"),
                user_name: ArcStr::from("U1Name"),
                group_name: ArcStr::from("G1Name"),
            })),
            Content::UserRemove(Arc::new(UserRemoveNotification {
                messenger_id: ArcStr::from("m1"),
                user_id: ArcStr::from("u1"),
                messenger_name: ArcStr::from("M1Name"),
                user_name: ArcStr::from("U1Name"),
            })),
            Content::Multi(vec![
                Content::Text(ArcStr::from("nested")),
            ]),
        ];
        for content in variants {
            let json = serde_json::to_value(&content).unwrap();
            let deserialized: Content = serde_json::from_value(json).unwrap();
            assert_eq!(deserialized, content);
        }
    }
}
