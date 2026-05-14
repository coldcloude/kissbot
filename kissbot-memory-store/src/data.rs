use std::{cmp::Ordering, sync::Arc};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

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

pub trait Record: Serialize + DeserializeOwned {
    fn sn(&self) -> u64;
    fn set_sn(&mut self, sn: u64);
    fn time(&self) -> Arc<String>;
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = self.time().as_str().cmp(other.time().as_str());
        if sign == Ordering::Equal {
            self.sn().cmp(&other.sn())
        } else {
            sign
        }
    }
}

macro_rules! impl_record {
    ($($t:ty),*) => {
        $(impl Record for $t {
            fn sn(&self) -> u64 {
                self.sn
            }
            fn set_sn(&mut self, sn: u64) {
                self.sn = sn;
            }
            fn time(&self) -> Arc<String> {
                self.time.clone()
            }
        })*
    };
}

impl_record!(
    ChannelRecord,
    ThinkRecord,
    ToolCallRecord,
    ToolResultRecord
);

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
