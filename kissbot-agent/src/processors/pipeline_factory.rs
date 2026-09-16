use std::sync::Arc;

use async_trait::async_trait;

use crate::{pipeline::{AgentInputProcessor, AgentPipeline}, processors::{input_processor::BatchAgentInputProcessor, message_sender::TokenLimitMessageSender, output_processor::ChannelOutputProcessor, tool_caller::StationToolCaller}, types::SessionKey};

#[async_trait]
pub trait AgentPipelineFactory {
    async fn create_input(&self, session_key: Arc<SessionKey>) -> Arc<dyn AgentInputProcessor>;
    async fn create(&self, session_key: Arc<SessionKey>) -> Arc<AgentPipeline>;
}

pub struct AgenticLoopCompressPipelineFactory;

#[async_trait]
impl AgentPipelineFactory for AgenticLoopCompressPipelineFactory {
    async fn create(&self, session_key: Arc<SessionKey>) -> Arc<AgentPipeline> {
        let message_sender = Arc::new(TokenLimitMessageSender::new_compress(session_key.clone()));
        let tool_caller = Arc::new(StationToolCaller);
        let output_processor = Arc::new(ChannelOutputProcessor::new(session_key.clone()));
        Arc::new(AgentPipeline {
            session_key,
            message_sender,
            tool_caller,
            output_processor,
        })
    }
    async fn create_input(&self, session_key: Arc<SessionKey>) -> Arc<dyn AgentInputProcessor> {
        let processor = BatchAgentInputProcessor::new(session_key).await;
        Arc::new(processor)
    }
}

pub struct AgenticLoopMemoryRecoverPipelineFactory;

#[async_trait]
impl AgentPipelineFactory for AgenticLoopMemoryRecoverPipelineFactory {
    async fn create(&self, session_key: Arc<SessionKey>) -> Arc<AgentPipeline> {
        let message_sender = Arc::new(TokenLimitMessageSender::new_memory_recover(session_key.clone()));
        let tool_caller = Arc::new(StationToolCaller);
        let output_processor = Arc::new(ChannelOutputProcessor::new(session_key.clone()));
        Arc::new(AgentPipeline {
            session_key,
            message_sender,
            tool_caller,
            output_processor,
        })
    }
    async fn create_input(&self, session_key: Arc<SessionKey>) -> Arc<dyn AgentInputProcessor> {
        let processor = BatchAgentInputProcessor::new(session_key).await;
        Arc::new(processor)
    }
}
