use std::sync::Arc;

use kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content, MessageItem, MSG_TYPE_ATTACHMENT, MSG_TYPE_MULTI};

use crate::error::Result;

/// 附件 key 生成器。将 AttachmentInfo 映射为全局唯一的 attachment key。
pub trait AttachmentKeyGenerator: Send + Sync {
    fn generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String;
}

/// 处理 OutgoingMessage 中的附件类型消息。
///
/// 递归遍历 content，将所有 AttachmentInfo 替换为 AttachmentInfoResponse（嵌入 key），
/// 同时收集待处理的附件列表供调用方创建临时文件。
///
/// 返回 (OutgoingMessageResponse, 待处理附件列表)。
pub fn process_attachment_message(
    outgoing: &OutgoingMessage,
    msg_id: &str,
    key_generator: &dyn AttachmentKeyGenerator,
) -> Result<(OutgoingMessageResponse, Vec<(Arc<AttachmentInfo>, Arc<String>)>)> {
    let mut pending_attachments: Vec<(Arc<AttachmentInfo>, Arc<String>)> = Vec::new();
    let new_content = process_content(
        &outgoing.content,
        outgoing.group_id.as_str(),
        msg_id,
        key_generator,
        &mut pending_attachments,
    )?;

    let response = OutgoingMessageResponse {
        msg_id: Arc::new(msg_id.to_string()),
        time: Arc::new(String::new()),  // 调用方会覆写 time
        msg_type: outgoing.msg_type.clone(),
        content: new_content.clone(),
    };

    Ok((response, pending_attachments))
}

/// 递归处理 Content，将 AttachmentInfo 替换为 AttachmentInfoResponse。
fn process_content(
    content: &Content,
    group_id: &str,
    msg_id: &str,
    key_generator: &dyn AttachmentKeyGenerator,
    pending_attachments: &mut Vec<(Arc<AttachmentInfo>, Arc<String>)>,
) -> Result<Content> {
    match content {
        Content::AttachmentInfo(info) => {
            let key = key_generator.generate_key(group_id, msg_id, info);
            let key_arc = Arc::new(key);
            pending_attachments.push((info.clone(), key_arc.clone()));
            Ok(Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
                key: key_arc,
                info: info.clone(),
            })))
        }
        Content::Multi(items) => {
            let new_items: Result<Vec<Arc<MessageItem>>> = items.iter().map(|item| {
                let new_content = process_content(
                    &item.content,
                    group_id,
                    msg_id,
                    key_generator,
                    pending_attachments,
                )?;
                Ok(Arc::new(MessageItem {
                    msg_type: item.msg_type.clone(),
                    content: new_content,
                }))
            }).collect();
            Ok(Content::Multi(new_items?))
        }
        // 其他类型（text、group_change、user_remove、AttachmentInfoResponse），不做处理
        _ => Ok(content.clone()),
    }
}
