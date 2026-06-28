use std::sync::Arc;

use dashmap::DashMap;
use kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content, MessageItem, MSG_TYPE_ATTACHMENT, MSG_TYPE_MULTI, MSG_TYPE_TEXT};

use crate::error::Result;

/// 附件 key 生成器。将 AttachmentInfo 映射为全局唯一的 attachment key。
pub trait AttachmentKeyGenerator: Send + Sync {
    fn generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String;
}

/// 处理 OutgoingMessage 中的附件类型消息。
///
/// 根据 msg_type：
/// - "text"：原样返回，key_map 为空
/// - "attachment"：解析 content 中 AttachmentInfo → 生成 key → 返回 AttachmentInfoResponse 内容
/// - "multi"：逐项处理，attachment 类型项同上处理
///
/// 返回 (新 content, OutgoingMessageResponse, 待处理附件列表)。
/// 待处理附件列表为 Vec<(Arc<AttachmentInfo>, Arc<String>)>，
/// 其中 Arc<String> 为生成的 key，供 Task 3 创建临时文件使用。
pub fn process_attachment_message(
    outgoing: &OutgoingMessage,
    msg_id: &str,
    key_generator: &dyn AttachmentKeyGenerator,
) -> Result<(Content, OutgoingMessageResponse, Vec<(Arc<AttachmentInfo>, Arc<String>)>)> {
    let attachment_key_map = Arc::new(DashMap::new());
    let mut pending_attachments: Vec<(Arc<AttachmentInfo>, Arc<String>)> = Vec::new();

    let new_content = match outgoing.msg_type.as_str() {
        MSG_TYPE_TEXT => {
            // 纯文本，无附件处理
            match &outgoing.content {
                Content::Text(s) => Content::Text(s.clone()),
                _ => return Err(crate::Error::InternalError("expected Text content".to_string())),
            }
        }
        MSG_TYPE_ATTACHMENT => {
            // 单条附件：content 是 AttachmentInfo
            let info = match &outgoing.content {
                Content::AttachmentInfo(info) => info.clone(),
                _ => return Err(crate::Error::InternalError("expected AttachmentInfo content".to_string())),
            };
            let key = key_generator.generate_key(
                outgoing.group_id.as_str(), msg_id, &info
            );
            let key_arc = Arc::new(key.clone());
            attachment_key_map.insert(info.att_id.to_string(), key_arc.clone());
            pending_attachments.push((info.clone(), key_arc.clone()));
            let response = Arc::new(AttachmentInfoResponse {
                key: key_arc,
                info: info,
            });
            Content::AttachmentInfoResponse(response)
        }
        MSG_TYPE_MULTI => {
            // multi：逐项处理
            let items = match &outgoing.content {
                Content::Multi(items) => items.clone(),
                _ => return Err(crate::Error::InternalError("expected Multi content".to_string())),
            };
            let new_items: crate::error::Result<Vec<Arc<MessageItem>>> = items.into_iter().map(|item| {
                if item.msg_type.as_str() == MSG_TYPE_ATTACHMENT {
                    let info = match &item.content {
                        Content::AttachmentInfo(info) => info.clone(),
                        _ => return Err(crate::Error::InternalError("expected AttachmentInfo content in multi item".to_string())),
                    };
                    let key = key_generator.generate_key(
                        outgoing.group_id.as_str(), msg_id, &info
                    );
                    let key_arc = Arc::new(key.clone());
                    attachment_key_map.insert(info.att_id.to_string(), key_arc.clone());
                    pending_attachments.push((info.clone(), key_arc.clone()));
                    let response = Arc::new(AttachmentInfoResponse {
                        key: key_arc,
                        info: info,
                    });
                    Ok(Arc::new(MessageItem {
                        msg_type: item.msg_type.clone(),
                        content: Content::AttachmentInfoResponse(response),
                    }))
                } else {
                    // 非 attachment 类型（如 text），原样保留
                    Ok(item)
                }
            }).collect();
            let items = new_items?;
            Content::Multi(items)
        }
        _other => {
            // 其他类型（如 system_join、system_leave），不做处理
            outgoing.content.clone()
        }
    };

    let response = OutgoingMessageResponse {
        msg_id: Arc::new(msg_id.to_string()),
        time: Arc::new(String::new()),  // 调用方会覆写 time
        content: new_content.clone(),
    };

    Ok((new_content, response, pending_attachments))
}
