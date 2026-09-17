use std::sync::Arc;

use crate::{config_manager::ConfigManager, configs::OutChannelConfig, nexus::Nexus, pipeline::AgentOutputProcessor, types::{Message, ModelResponse, Result, SessionKey, role_mode}};
use async_trait::async_trait;

/// Agentic Loop 工具调用轮次上限（防死循环）
const MAX_TOOL_ROUNDS: usize = 10;

pub struct ChannelOutputProcessor {
    session_key: Arc<SessionKey>,
}

impl ChannelOutputProcessor {
    pub fn new(session_key: Arc<SessionKey>) -> Self {
        Self { session_key }
    }
}

#[async_trait]
impl AgentOutputProcessor for ChannelOutputProcessor {
    async fn accept(&self, round: usize, response: Result<ModelResponse>) -> (bool, Vec<Message>) {
        if let Ok(mut resp) = response {
            let mut next = false;
            let mut new_messages = Vec::new();
            new_messages.push(Message::Assistant {
                content: resp.raw_content.take(),
                reasoning_content: resp.raw_reasoning_content.take(),
                tool_calls: resp.raw_tool_calls.take(),
            });
            if let Some(mut tool_calls) = resp.tool_calls {
                for tool_call in tool_calls.drain(..) {
                    let content = if tool_call.data.result.is_null() {
                        tool_call.data.error.to_string()
                    } else {
                        tool_call.data.result.to_string()
                    };
                    new_messages.push(Message::Tool {
                        tool_call_id: tool_call.id,
                        content: Arc::new(content),
                    });
                    next = true;
                }
            }
            if let Some(content) = resp.content.as_ref() {
                let oc_cfg = ConfigManager::get().session_config::<OutChannelConfig,OutChannelConfig>(self.session_key.as_ref()).await;
                if let Some(out_channel) = oc_cfg.out_channel.as_ref() {
                    let agent_id = self.session_key.agent_id.as_str();
                    let role_name = self.session_key.role_name.as_str();
                    let role_event = role_mode(role_name, &self.session_key.mode);
                    Nexus::get().send_outgoing(agent_id, role_name, &role_event, out_channel.as_ref(), content.clone()).await;
                }
            }
            if round >= MAX_TOOL_ROUNDS {
                next = false;
            }
            (next, new_messages)
        } else {
            (false, Vec::new())
        }
    }
}