use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::kinds::*;

// ========== Request structures (simple, no generics) ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequest {
    pub agent_id: String,
    pub role_name: String,
    pub channel_id: String,
    pub user_id: String,
    pub is_self: usize,
    pub msg_type: String,
    pub content: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequests {
    pub requests: Vec<ChannelRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRequest {
    pub agent_id: String,
    pub role_name: String,
    pub content: String,
    pub key: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRequests {
    pub requests: Vec<ThinkRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub agent_id: String,
    pub role_name: String,
    pub tool_name: String,
    pub tool_params: serde_json::Value,
    pub key: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequests {
    pub requests: Vec<ToolCallRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRequest {
    pub agent_id: String,
    pub role_name: String,
    pub tool_result: serde_json::Value,
    pub key: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRequests {
    pub requests: Vec<ToolResultRequest>,
    pub force: usize,
}

// ========== Query requests ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryChannelRequest {
    pub agent_id: String,
    pub role_name: String,
    pub channel_id: String,
    pub start_time: String,
    pub end_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub agent_id: String,
    pub role_name: String,
    pub start_time: String,
    pub end_time: String,
}

// ========== ValueKind trait for serde_json::Value abstraction ==========

pub trait ValueKind: Clone {
    type Type: Clone;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SyncValue;

impl ValueKind for SyncValue {
    type Type = Arc<serde_json::Value>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LocalValue;

impl ValueKind for LocalValue {
    type Type = serde_json::Value;
}

// ========== ChannelRecord - Generic with trait bounds ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecordGeneric<S>
where
    S: StringKind,
{
    pub agent_id: S::Type,
    pub role_name: S::Type,
    pub channel_id: S::Type,
    pub user_id: S::Type,
    pub is_self: usize,
    pub msg_type: S::Type,
    pub content: S::Type,
    pub time: S::Type,
    pub sn: u64,
}

pub type ChannelRecordEntity = ChannelRecordGeneric<LocalString>;

// ========== ThinkRecord - Generic with trait bounds ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecordGeneric<S>
where
    S: StringKind,
{
    pub agent_id: S::Type,
    pub role_name: S::Type,
    pub content: S::Type,
    pub key: S::Type,
    pub time: S::Type,
    pub sn: u64,
}

pub type ThinkRecordEntity = ThinkRecordGeneric<LocalString>;

// ========== ToolCallRecord - Generic with trait bounds ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecordGeneric<S, V>
where
    S: StringKind,
    V: ValueKind,
{
    pub agent_id: S::Type,
    pub role_name: S::Type,
    pub tool_name: S::Type,
    pub tool_params: V::Type,
    pub key: S::Type,
    pub time: S::Type,
    pub sn: u64,
}

pub type ToolCallRecordEntity = ToolCallRecordGeneric<LocalString, LocalValue>;

// ========== ToolResultRecord - Generic with trait bounds ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecordGeneric<S, V>
where
    S: StringKind,
    V: ValueKind,
{
    pub agent_id: S::Type,
    pub role_name: S::Type,
    pub tool_result: V::Type,
    pub key: S::Type,
    pub time: S::Type,
    pub sn: u64,
}

pub type ToolResultRecordEntity = ToolResultRecordGeneric<LocalString, LocalValue>;
