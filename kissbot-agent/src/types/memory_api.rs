use std::sync::Arc;

use arcstr::ArcStr;
use chrono::Local;
use kissbot_api::{ThinkRequest, ToolCallRequest, ToolResultRequest};
use kissbot_llm::ToolCall;

use crate::types::MemoryKey;

fn now() -> ArcStr {
    ArcStr::from(Local::now().format("%Y-%m-%d %H:%M:%S").to_string())
}

pub fn build_memory_think(memory_key: &MemoryKey, key: ArcStr, reasoning_content: Option<ArcStr>, thinking: Option<ArcStr>) -> ThinkRequest {
    let time = now();
    ThinkRequest {
        agent_id: memory_key.agent_id.clone(),
        role_event: memory_key.role_event.clone(),
        reasoning_content,
        thinking,
        key,
        time,
    }
}

pub fn build_memory_tool_call(memory_key: &MemoryKey, key: ArcStr, tool_call: &ToolCall) -> ToolCallRequest {
    let time = now();
    let tool_params = match tool_call.result_arguments.as_ref() {
        Some(params) => params.clone(),
        None => Arc::new(tool_call.arguments.clone()),
    };
    ToolCallRequest {
        agent_id: memory_key.agent_id.clone(),
        role_event: memory_key.role_event.clone(),
        tool_name: tool_call.name.clone(),
        tool_params,
        key,
        time,
    }
}

pub fn build_memory_tool_result(memory_key: &MemoryKey, key: ArcStr, tool_call: &ToolCall) -> ToolResultRequest {
    let time = now();
    ToolResultRequest {
        agent_id: memory_key.agent_id.clone(),
        role_event: memory_key.role_event.clone(),
        tool_result: tool_call.result.clone(),
        key,
        time,
    }
}