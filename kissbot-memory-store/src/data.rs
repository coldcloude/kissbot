use std::sync::Arc;

use serde::{Deserialize, Serialize};

use kissbot_api::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub user_id: Arc<String>,
    pub time: Arc<String>,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecord {
    pub content: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: Arc<String>,
    pub tool_params: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub tool_result: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

pub type ChannelRecordResult = ChannelRecordGeneric<SyncString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChannelRecord;

impl ChannelRecordKind<SyncString> for SyncChannelRecord {
    type Type = Arc<ChannelRecordResult>;
}

pub type ThinkRecordResult = ThinkRecordGeneric<SyncString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncThinkRecord;

impl ThinkRecordKind<SyncString> for SyncThinkRecord {
    type Type = Arc<ThinkRecordResult>;
}

pub type ToolCallRecordResult = ToolCallRecordGeneric<SyncString, SyncValue>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncToolCallRecord;

impl ToolCallRecordKind<SyncString, SyncValue> for SyncToolCallRecord {
    type Type = Arc<ToolCallRecordResult>;
}

pub type ToolResultRecordResult = ToolResultRecordGeneric<SyncString, SyncValue>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncToolResultRecord;

impl ToolResultRecordKind<SyncString, SyncValue> for SyncToolResultRecord {
    type Type = Arc<ToolResultRecordResult>;
}
