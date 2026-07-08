use std::sync::Arc;

use async_trait::async_trait;
use kissbot_api::channel::OutgoingMessage;
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content, MessageItem};

use crate::error::Result;

/// 附件注册器。将 AttachmentInfo 注册为全局唯一的 key，并管理 key 与 info 的关系。
#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    /// 注册附件，返回包含 key、info、transfer_id 的响应。
    /// transfer_id 用于上传时的 write_chunk 路由。
    async fn register(
        &self,
        messenger_id: &str,
        user_id: &str,
        group_id: &str,
        info: Arc<AttachmentInfo>,
    ) -> Result<Arc<AttachmentInfoResponse>>;
}

/// 处理 OutgoingMessage 中的附件类型消息。
///
/// 递归遍历 content，将所有 AttachmentInfo 替换为 AttachmentInfoResponse（嵌入 key 和 transfer_id）。
/// 注册过程由 AttachmentRegistry 完成。
/// 返回顶层 MessageItem，调用方将其与其他消息类型组装。
pub async fn process_attachment_message(
    outgoing: Arc<OutgoingMessage>,
    registry: &dyn AttachmentRegistry,
) -> Result<Arc<MessageItem>> {
    let new_content = process_content(
        &outgoing.content,
        outgoing.messenger_id.as_str(),
        outgoing.user_id.as_str(),
        outgoing.group_id.as_str(),
        registry,
    ).await?;

    Ok(Arc::new(MessageItem {
        msg_type: outgoing.msg_type.clone(),
        content: new_content,
    }))
}

/// 递归处理 Content，将 AttachmentInfo 替换为 AttachmentInfoResponse。
async fn process_content(
    content: &Content,
    messenger_id: &str,
    user_id: &str,
    group_id: &str,
    registry: &dyn AttachmentRegistry,
) -> Result<Content> {
    match content {
        Content::AttachmentInfo(info) => {
            let resp = registry.register(messenger_id, user_id, group_id, info.clone()).await?;
            Ok(Content::AttachmentInfoResponse(resp))
        }
        Content::Multi(items) => {
            let mut new_items = Vec::with_capacity(items.len());
            for item in items.iter() {
                let new_content = Box::pin(process_content(
                    &item.content,
                    messenger_id,
                    user_id,
                    group_id,
                    registry,
                )).await?;
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
