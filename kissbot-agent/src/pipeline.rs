use std::sync::Arc;

use async_trait::async_trait;
use kissbot_api::IncomingMessageEvent;

use kissbot_llm::{Message, Session, ToolCall};
use crate::types::{Error, SessionKey};

#[async_trait]
pub trait SystemPromptGenerator {
    async fn reset_system_prompt(&self);
}

#[async_trait]
pub trait IncomingMessageProcessor {
    async fn incoming_message(&self, event: Arc<IncomingMessageEvent>);
}

#[async_trait]
pub trait AgentPipeline: IncomingMessageProcessor + SystemPromptGenerator {}

#[async_trait]
pub trait AgentMessageProcessor: Send + Sync {
    async fn process_messages(&self, messages: Vec<Message>);
}

#[async_trait]
pub trait TokenReducer: Send + Sync + 'static {
    async fn reduce(&self, session_key: SessionKey, session: &Session, messages: Vec<Message>) -> (u64, Vec<Message>);
}

#[async_trait]
pub trait ToolCaller: Send + Sync {
    async fn call_tool(&self, tool_call: ToolCall) -> (ToolCall, Option<Error>);
}
