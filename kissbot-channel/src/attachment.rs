use std::sync::Arc;

use kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content, MessageItem};

use crate::error::Result;

/// 附件注册器。将 AttachmentInfo 注册为全局唯一的 key，并管理 key 与 info 的关系。
pub trait AttachmentRegistry: Send + Sync {
    /// 注册附件，返回生成的 key。
    fn register(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String;

    /// 取出所有待处理的附件列表（key → info）。
    fn drain_pending(&self) -> Vec<(Arc<String>, Arc<AttachmentInfo>)>;
}

/// 处理 OutgoingMessage 中的附件类型消息。
///
/// 递归遍历 content，将所有 AttachmentInfo 替换为 AttachmentInfoResponse（嵌入 key）。
/// 注册过程由 AttachmentRegistry 完成，调用方通过 drain_pending 获取待处理附件列表。
pub fn process_attachment_message(
    outgoing: &OutgoingMessage,
    msg_id: &str,
    registry: &dyn AttachmentRegistry,
) -> Result<OutgoingMessageResponse> {
    let new_content = process_content(
        &outgoing.content,
        outgoing.group_id.as_str(),
        msg_id,
        registry,
    )?;

    let response = OutgoingMessageResponse {
        msg_id: Arc::new(msg_id.to_string()),
        time: Arc::new(String::new()),  // 调用方会覆写 time
        msg_type: outgoing.msg_type.clone(),
        content: new_content.clone(),
    };

    Ok(response)
}

/// 递归处理 Content，将 AttachmentInfo 替换为 AttachmentInfoResponse。
fn process_content(
    content: &Content,
    group_id: &str,
    msg_id: &str,
    registry: &dyn AttachmentRegistry,
) -> Result<Content> {
    match content {
        Content::AttachmentInfo(info) => {
            let key = registry.register(group_id, msg_id, info);
            Ok(Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
                key: Arc::new(key),
                info: info.clone(),
            })))
        }
        Content::Multi(items) => {
            let new_items: Result<Vec<Arc<MessageItem>>> = items.iter().map(|item| {
                let new_content = process_content(
                    &item.content,
                    group_id,
                    msg_id,
                    registry,
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
