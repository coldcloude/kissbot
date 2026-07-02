use std::sync::Arc;

use kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content, MessageItem};

use crate::error::Result;

/// 附件注册器。将 AttachmentInfo 注册为全局唯一的 key，并管理 key 与 info 的关系。
pub trait AttachmentRegistry: Send + Sync {
    /// 注册附件，返回生成的 key。
    fn register(&self, messenger_id: &str, user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Arc<String>;
}

/// 处理 OutgoingMessage 中的附件类型消息。
///
/// 递归遍历 content，将所有 AttachmentInfo 替换为 AttachmentInfoResponse（嵌入 key）。
/// 注册过程由 AttachmentRegistry 完成。
pub fn process_attachment_message(
    outgoing: Arc<OutgoingMessage>,
    registry: Arc<dyn AttachmentRegistry>,
) -> Result<Arc<OutgoingMessageResponse>> {
    let new_content = process_content(
        &outgoing.content,
        outgoing.messenger_id.as_str(),
        outgoing.user_id.as_str(),
        outgoing.group_id.as_str(),
        registry.as_ref(),
    )?;

    Ok(Arc::new(OutgoingMessageResponse {
        msg_id: Arc::new(String::new()),  // 调用方会覆写 msg_id 和 time
        time: Arc::new(String::new()),
        msg_type: outgoing.msg_type.clone(),
        content: new_content.clone(),
    }))
}

/// 递归处理 Content，将 AttachmentInfo 替换为 AttachmentInfoResponse。
fn process_content(
    content: &Content,
    messenger_id: &str,
    user_id: &str,
    group_id: &str,
    registry: &dyn AttachmentRegistry,
) -> Result<Content> {
    match content {
        Content::AttachmentInfo(info) => {
            let key = registry.register(messenger_id, user_id, group_id, info.clone());
            Ok(Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
                key,
                info: info.clone(),
            })))
        }
        Content::Multi(items) => {
            let mut new_items = Vec::with_capacity(items.len());
            for item in items.iter() {
                let new_content = process_content(
                    &item.content,
                    messenger_id,
                    user_id,
                    group_id,
                    registry,
                )?;
                new_items.push(Arc::new(MessageItem {
                    msg_type: item.msg_type.clone(),
                    content: new_content,
                }));
            }
            Ok(Content::Multi(new_items))
        }
        // 其他类型（text、group_change、user_remove、AttachmentInfoResponse），不做处理
        _ => Ok(content.clone()),
    }
}
