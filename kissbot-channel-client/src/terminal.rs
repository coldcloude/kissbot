use std::sync::Arc;
use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::channel::*;
use kissbot_api::message::*;

use crate::error::Result;

/// 终端接口：ChannelClient 收到服务端推送后调用的回调函数。
/// id 是触发事件的 ChannelClient 的标识（由 ChannelClient::new 时传入）。
/// receiver 用 self: Arc<Self>（by-value Arc）：调用方（ChannelClient）持 Weak<dyn Terminal>，
/// upgrade 得 Arc<dyn Terminal> 后直接调用——与 &self 不同，方法内可直接持有/降级 Arc 自身
/// （&Arc<Self> receiver 不可对象化 E0038，trait 里无法声明）。
#[async_trait]
pub trait Terminal: Send + Sync + 'static {
    /// 收到上行消息（含接收方 recipient_user_id）
    async fn incoming_message(self: Arc<Self>, id: &str, message: Arc<IncomingMessageEvent>);
    /// 用户加入群组
    async fn join_group(self: Arc<Self>, id: &str, notification: Arc<GroupChangeNotification>);
    /// 用户离开群组
    async fn leave_group(self: Arc<Self>, id: &str, notification: Arc<GroupChangeNotification>);
    /// 用户被删除
    async fn user_removed(self: Arc<Self>, id: &str, notification: Arc<UserRemoveNotification>);
    /// 下载分块到达（Ok/Err 即该块的确认结果）
    async fn download_chunk(self: Arc<Self>, id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()>;
    /// 连接关闭（不做自动重连）
    async fn closed(self: Arc<Self>, id: &str);
}
