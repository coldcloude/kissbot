use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ========== Request structures ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequests {
    pub requests: Vec<ChannelRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub content: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRequests {
    pub requests: Vec<ThinkRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub tool_name: Arc<String>,
    pub tool_params: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequests {
    pub requests: Vec<ToolCallRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub tool_result: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRequests {
    pub requests: Vec<ToolResultRequest>,
    pub force: usize,
}

// ========== Query requests ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryChannelRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
}

// ========== Records ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub content: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub tool_name: Arc<String>,
    pub tool_params: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub tool_result: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}
