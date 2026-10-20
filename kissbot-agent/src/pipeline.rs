use std::sync::Arc;

use async_trait::async_trait;
use arcstr::ArcStr;
use kissbot_api::IncomingMessageEvent;

use kissbot_llm::{Message, Session, ToolCall};
use crate::types::{Error, SessionKey};

#[async_trait]
pub trait SystemPromptManager {
    async fn reset_system_prompt(&self);
}

#[async_trait]
pub trait IncomingMessageProcessor: Send + Sync {
    /// 输入入口。以 `Arc<Self>` 接收：按 channel 攒批的 pipeline 需要把 `Weak<Self>` 交给批次消费者，
    /// 避免「pipeline → producer → 消费者 → pipeline」引用环。
    async fn incoming_message(self: Arc<Self>, event: Arc<IncomingMessageEvent>);
}

#[async_trait]
pub trait IncomingMessageBatchProcessor: Send + Sync {
    /// 批次入口。以 `Arc<Self>` 接收：批次消费者只持弱引用，处理时升级后调用本方法。
    async fn incoming_message_batch(self: Arc<Self>, events: Vec<Arc<IncomingMessageEvent>>);
}

#[async_trait]
pub trait AgentPipeline: IncomingMessageProcessor + SystemPromptManager + Send + Sync {
    /// channel 不再绑定本会话：清理该 channel（messenger + 绑定用户）在本会话中的资源。
    ///
    /// 按 channel 攒批的 pipeline 覆写本方法丢弃对应 channel 的批次生产者与待处理消息；
    /// 其余 pipeline 无 channel 级状态，使用默认空实现。
    async fn channel_unbound(&self, _messenger_id: ArcStr, _self_user_id: ArcStr) {}
}

#[async_trait]
pub trait TokenReducer: Send + Sync + 'static {
    async fn reduce(&self, session_key: SessionKey, session: &Session, messages: Vec<Message>) -> (u64, Vec<Message>);
}

#[async_trait]
pub trait ToolCaller: Send + Sync {
    /// 依次执行工具调用（`&mut self` 便于调用方在单次 run 内直接累积工具内容，无需共享 Mutex）
    async fn call_tool(&mut self, tool_call: ToolCall) -> (ToolCall, Option<Error>);
}
