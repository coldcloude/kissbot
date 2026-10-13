use std::sync::Arc;

use arcstr::ArcStr;
use async_trait::async_trait;
use futures_util::future;
use kissbot_api::IncomingMessageEvent;

use kissbot_llm::{Message, ModelResponse, Session, ToolCall, ToolConfig, session};

use crate::{nexus::Nexus, types::{MemoryKey, Result, SessionKey}};

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
