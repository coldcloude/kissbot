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
    pub time: String,
    pub msg_type: String,
    pub content: String,
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
pub struct ToolCallRequest {
    pub agent_id: String,
    pub role_name: String,
    pub tool_name: String,
    pub tool_params: serde_json::Value,
    pub key: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRequest {
    pub agent_id: String,
    pub role_name: String,
    pub tool_result: serde_json::Value,
    pub key: String,
    pub time: String,
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
    pub time: S::Type,
    pub msg_type: S::Type,
    pub content: S::Type,
    pub sn: u64,
}

pub trait ChannelRecordKind<S>
where
    S: StringKind,
{
    type Type: Clone;
}

pub type ChannelRecordEntity = ChannelRecordGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalChannelRecord;

impl ChannelRecordKind<LocalString> for LocalChannelRecord {
    type Type = ChannelRecordEntity;
}

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

pub trait ThinkRecordKind<S>
where
    S: StringKind,
{
    type Type: Clone;
}

pub type ThinkRecordEntity = ThinkRecordGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalThinkRecord;

impl ThinkRecordKind<LocalString> for LocalThinkRecord {
    type Type = ThinkRecordEntity;
}

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

pub trait ToolCallRecordKind<S, V>
where
    S: StringKind,
    V: ValueKind,
{
    type Type: Clone;
}

pub type ToolCallRecordEntity = ToolCallRecordGeneric<LocalString, LocalValue>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalToolCallRecord;

impl ToolCallRecordKind<LocalString, LocalValue> for LocalToolCallRecord {
    type Type = ToolCallRecordEntity;
}

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

pub trait ToolResultRecordKind<S, V>
where
    S: StringKind,
    V: ValueKind,
{
    type Type: Clone;
}

pub type ToolResultRecordEntity = ToolResultRecordGeneric<LocalString, LocalValue>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalToolResultRecord;

impl ToolResultRecordKind<LocalString, LocalValue> for LocalToolResultRecord {
    type Type = ToolResultRecordEntity;
}
