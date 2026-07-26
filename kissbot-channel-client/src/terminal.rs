use std::sync::{Arc, Weak};
use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::channel::*;
use kissbot_api::message::*;

use crate::error::Result;

/// 终端接口：ChannelClient 直接调用的事件函数（ws 收到的服务端推送都转接到这里）。
#[async_trait]
pub trait Terminal: Send + Sync + 'static {
    /// 收到上行消息
    async fn incoming_message(&self, message: Arc<IncomingMessage>);
    /// 用户加入群组
    async fn join_group(&self, notification: Arc<GroupChangeNotification>);
    /// 用户离开群组
    async fn leave_group(&self, notification: Arc<GroupChangeNotification>);
    /// 用户被删除
    async fn user_removed(&self, notification: Arc<UserRemoveNotification>);
    /// 下载分块到达（请求下载后由服务端推送，Ok/Err 即该块的确认结果）
    async fn download_chunk(&self, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()>;
    /// 连接关闭（不做自动重连）
    async fn closed(&self);
}

/// 绑定/解绑（由 ChannelClient 实现，注入 Terminal）
#[async_trait]
pub trait BindHandler: Send + Sync {
    async fn bind(&self, request: BindRequest) -> Result<()>;
    async fn unbind(&self, request: BindRequest) -> Result<()>;
}

/// 查询 messenger 信息（由 ChannelClient 实现，注入 Terminal）
#[async_trait]
pub trait MessengerInfoHandler: Send + Sync {
    async fn get_info(&self, messenger_id: Arc<String>) -> Result<Arc<MessengerInfo>>;
}

/// 发送下行消息（由 ChannelClient 实现，注入 Terminal）
#[async_trait]
pub trait OutgoingMessageHandler: Send + Sync {
    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>>;
}

/// 上传附件分块（由 ChannelClient 实现，注入 Terminal）
#[async_trait]
pub trait AttachmentUploadHandler: Send + Sync {
    async fn send_upload_chunk(&self, transfer_id: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse>;
}

/// 请求附件下载（由 ChannelClient 实现，注入 Terminal）。
/// 返回下载头 AttachmentInfoResponse，之后分块经 Terminal::download_chunk 推送。
#[async_trait]
pub trait AttachmentDownloadHandler: Send + Sync {
    async fn request_download(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>>;
}

/// Terminal 创建器。T 为具体 Terminal 类型，create 返回 Arc<T> 供调用方直接使用。
/// ChannelClient 的各 handler 以 Weak 静态注入，避免循环引用。
#[async_trait]
pub trait TerminalCreator<T: Terminal> {
    async fn create(
        &self,
        bind_handler: Weak<dyn BindHandler>,
        messenger_info_handler: Weak<dyn MessengerInfoHandler>,
        outgoing_message_handler: Weak<dyn OutgoingMessageHandler>,
        attachment_upload_handler: Weak<dyn AttachmentUploadHandler>,
        attachment_download_handler: Weak<dyn AttachmentDownloadHandler>,
    ) -> Result<Arc<T>>;
}
