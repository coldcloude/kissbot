use std::sync::Arc;

use dashmap::DashMap;
use kissbot_api::channel::{
    AttachmentInfo, OutgoingMessage, OutgoingMessageResponse, ResponseAttachmentInfo,
};
use kissbot_api::message::{MessageItem, MSG_TYPE_ATTACHMENT, MSG_TYPE_MULTI, MSG_TYPE_TEXT};

use crate::error::Result;

/// 附件 key 生成器。将 AttachmentInfo 映射为全局唯一的 attachment key。
pub trait AttachmentKeyGenerator: Send + Sync {
    fn generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String;
}

/// 处理 OutgoingMessage 中的附件类型消息。
///
/// 根据 msg_type：
/// - "text"：原样返回，key_map 为空
/// - "attachment"：解析 content 中 AttachmentInfo → 生成 key → 返回 ResponseAttachmentInfo 内容
/// - "multi"：逐项处理，attachment 类型项同上处理
///
/// 返回 (新 content, OutgoingMessageResponse, 待处理附件列表)。
/// 待处理附件列表为 Vec<(Arc<AttachmentInfo>, Arc<String>)>，
/// 其中 Arc<String> 为生成的 key，供 Task 3 创建临时文件使用。
pub fn process_attachment_message(
    outgoing: &OutgoingMessage,
    msg_id: &str,
    key_generator: &dyn AttachmentKeyGenerator,
) -> Result<(String, OutgoingMessageResponse, Vec<(Arc<AttachmentInfo>, Arc<String>)>)> {
    let attachment_key_map = Arc::new(DashMap::new());
    let mut pending_attachments: Vec<(Arc<AttachmentInfo>, Arc<String>)> = Vec::new();

    let new_content = match outgoing.msg_type.as_str() {
        MSG_TYPE_TEXT => {
            // 纯文本，无附件处理
            outgoing.content.to_string()
        }
        MSG_TYPE_ATTACHMENT => {
            // 单条附件：content 是 AttachmentInfo JSON
            let info: AttachmentInfo = serde_json::from_str(outgoing.content.as_str())
                .map_err(|e| crate::Error::InternalError(format!("parse AttachmentInfo failed: {}", e)))?;
            let key = key_generator.generate_key(
                outgoing.group_id.as_str(), msg_id, &info
            );
            let info_arc = Arc::new(info);
            let key_arc = Arc::new(key.clone());
            attachment_key_map.insert(info_arc.att_id.to_string(), key_arc.clone());
            pending_attachments.push((info_arc.clone(), key_arc.clone()));
            let response = ResponseAttachmentInfo {
                key: key_arc,
                info: info_arc,
            };
            serde_json::to_string(&response)
                .map_err(|e| crate::Error::InternalError(format!("serialize ResponseAttachmentInfo failed: {}", e)))?
        }
        MSG_TYPE_MULTI => {
            // multi：逐项处理
            let items: Vec<MessageItem> = serde_json::from_str(outgoing.content.as_str())
                .map_err(|e| crate::Error::InternalError(format!("parse MessageItem[] failed: {}", e)))?;
            let new_items: crate::error::Result<Vec<MessageItem>> = items.into_iter().map(|item| {
                if item.msg_type.as_str() == MSG_TYPE_ATTACHMENT {
                    let info: AttachmentInfo = serde_json::from_str(item.content.as_str())
                        .map_err(|e| crate::Error::InternalError(format!("parse AttachmentInfo failed: {}", e)))?;
                    let key = key_generator.generate_key(
                        outgoing.group_id.as_str(), msg_id, &info
                    );
                    let info_arc = Arc::new(info);
                    let key_arc = Arc::new(key.clone());
                    attachment_key_map.insert(info_arc.att_id.to_string(), key_arc.clone());
                    pending_attachments.push((info_arc.clone(), key_arc.clone()));
                    let response = ResponseAttachmentInfo {
                        key: key_arc,
                        info: info_arc,
                    };
                    let new_content = serde_json::to_string(&response)
                        .map_err(|e| crate::Error::InternalError(format!("serialize ResponseAttachmentInfo failed: {}", e)))?;
                    Ok(MessageItem {
                        msg_type: item.msg_type,
                        content: Arc::new(new_content),
                    })
                } else {
                    // 非 attachment 类型（如 text），原样保留
                    Ok(item)
                }
            }).collect();
            let items = new_items?;
            serde_json::to_string(&items)
                .map_err(|e| crate::Error::InternalError(format!("serialize MessageItem[] failed: {}", e)))?
        }
        _other => {
            // 其他类型（如 system_join、system_leave），不做处理
            outgoing.content.to_string()
        }
    };

    let response = OutgoingMessageResponse {
        msg_id: Arc::new(msg_id.to_string()),
        time: Arc::new(String::new()),  // 调用方会覆写 time
        attachment_key_map,
    };

    Ok((new_content, response, pending_attachments))
}
