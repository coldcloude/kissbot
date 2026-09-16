use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future;
use kissbot_api::IncomingMessageEvent;

use crate::{configs::ToolConfig, nexus::Nexus, types::{Message, ModelResponse, Result, SessionKey, ToolCall}};

#[async_trait]
pub trait AgentToolCaller {
    async fn call_tool(&self, tool_call: ToolCall) -> ToolCall;
}

#[async_trait]
pub trait AgentMessageSender {
    async fn send_messages(&self, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse>;
}

#[async_trait]
pub trait AgentInputProcessor {
    async fn accept(&self, event: Arc<IncomingMessageEvent>);
}

#[async_trait]
pub trait AgentOutputProcessor {
    async fn accept(&self, turn: usize, response: Result<ModelResponse>) -> (bool, Vec<Message>);
}

pub struct AgentPipeline {
    session_key: Arc<SessionKey>,
    message_sender: Arc<dyn AgentMessageSender>,
    tool_caller: Arc<dyn AgentToolCaller>,
    output_processor: Arc<dyn AgentOutputProcessor>,
}

impl AgentPipeline {
    async fn call_tool(&self, tool_call: ToolCall) -> ToolCall {
        let coordinator = Nexus::get();
        // 1. 工具调用 key：UUID（ToolCall/ToolResult 详情与 channel 占位同 key 关联）
        let tool_key = Arc::new(uuid::Uuid::new_v4().to_string());
        // 2. 记忆写入：ToolCallRequest.key 与 ToolResultRequest.key 用同一 key
        coordinator.send_memory_tool_call(self.session_key.as_ref(), tool_key.clone(), &tool_call).await;
        // 3. 执行工具
        let result = self.tool_caller.call_tool(tool_call).await;
        // 4. 记忆写入：ToolCallRequest.key 与 ToolResultRequest.key 用同一 key
        coordinator.send_memory_tool_result(self.session_key.as_ref(), tool_key, &result).await;
        result
    }

    async fn run_once(&self, round: usize, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> (bool, Vec<Message>) {
        let response = self.message_sender.send_messages(messages, tools).await;
        match response {
            Ok(mut res) => {
                // 推送 think 到 memory-store（reasoning_content + thinking 双字段，key 关联 ChannelRecord(Think)）
                // 任一有值才写，都 None 跳过
                if res.reasoning_content.is_some() || res.thinking.is_some() {
                    let key = Arc::new(uuid::Uuid::new_v4().to_string());
                    Nexus::get().send_memory_think(self.session_key.as_ref(), key, res.reasoning_content.clone(), res.thinking.clone()).await;
                }
                // 遍历执行 tool call
                if let Some(mut tool_calls) = res.tool_calls.take() {
                    let mut futs = Vec::with_capacity(tool_calls.len());
                    for tool_call in tool_calls.drain(..) {
                        let f = self.call_tool(tool_call);
                        futs.push(f);
                    }
                    let tool_results = future::join_all(futs).await;
                    res.tool_calls.replace(tool_results);
                }
                self.output_processor.accept(round, Ok(res)).await
            }
            Err(e) => {
                self.output_processor.accept(round, Err(e), ).await
            }
        }
    }

    pub async fn run(&self, message: Message, tools: &Vec<Arc<ToolConfig>>) {
        let session = Nexus::get().ensure_session(self.session_key.as_ref()).await;
        let mut round = 1 as usize;
        let (mut next, mut messages) = self.run_once(round, vec![message], tools).await;
        loop {
            if next {
                round = round + 1;
                (next, messages) = self.run_once(round, messages, tools).await;
            } else {
                if !messages.is_empty() {
                    session.context_append(messages).await;
                }
                break;
            }
        }
    }
}