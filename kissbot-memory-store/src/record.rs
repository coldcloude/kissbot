use dashmap::DashMap;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub agent_id: String,
    pub role_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub time: String,
    pub msg_type: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecordWithSn {
    pub user_id: String,
    pub time: String,
    pub msg_type: String,
    pub content: String,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingRecord {
    pub agent_id: String,
    pub role_id: String,
    pub content: String,
    pub key: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingRecordWithSn {
    pub content: String,
    pub key: String,
    pub time: String,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRecord {
    pub agent_id: String,
    pub role_id: String,
    pub tool_name: String,
    pub tool_params: serde_json::Value,
    pub tool_result: serde_json::Value,
    pub key: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRecordWithSn {
    pub tool_name: String,
    pub tool_params: serde_json::Value,
    pub tool_result: serde_json::Value,
    pub key: String,
    pub time: String,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ChannelRecordKey {
    pub agent_id: String,
    pub role_id: String,
    pub channel_id: String,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub agent_id: String,
    pub role_id: String,
    pub date: String,
}

pub struct RecordManager {
    channel_sn_counters: DashMap<ChannelRecordKey, AtomicU64>,
    thinking_sn_counters: DashMap<RecordKey, AtomicU64>,
    tool_sn_counters: DashMap<RecordKey, AtomicU64>,
}

static RECORD_MANAGER: OnceLock<RecordManager> = OnceLock::new();

impl RecordManager {
    pub fn new() -> Self {
        Self {
            channel_sn_counters: DashMap::new(),
            thinking_sn_counters: DashMap::new(),
            tool_sn_counters: DashMap::new(),
        }
    }

    pub fn get() -> &'static Self {
        RECORD_MANAGER.get_or_init(|| RecordManager::new())
    }

    async fn ensure_year_role_dir(&self, agent_id: &str, role_id: &str, time: &str) -> Result<PathBuf> {
        let year = &time[0..4];
        let store_dir = DirectoryManager::get().ensure_agent_store_dir(agent_id).await?;
        let year_role_dir = store_dir.join(format!("{}-{}", year, role_id));
        
        if !year_role_dir.exists() {
            tokio::fs::create_dir_all(&year_role_dir).await?;
        }
        
        Ok(year_role_dir)
    }

    fn parse_date_from_time(time: &str) -> String {
        time[0..10].to_string()
    }

    async fn load_existing_sn(&self, file_path: &PathBuf) -> Result<AtomicU64> {
        if !file_path.exists() {
            return Ok(AtomicU64::new(0));
        }
        
        let max_sn = AtomicU64::new(0);

        let file = tokio::fs::File::open(file_path).await?;
        let reader = tokio::io::BufReader::new(file);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            if let Ok(record) = serde_json::from_str::<serde_json::Value>(line.as_str()) {
                if let Some(sn) = record.get("sn").and_then(|s| s.as_u64()) {
                    if sn > max_sn.load(Ordering::Relaxed) {
                        max_sn.store(sn, Ordering::Relaxed);
                    }
                }
            }
        }

        Ok(max_sn)
    }

    pub async fn append_channel_record(&self, record: ChannelRecord) -> Result<u64> {
        let date = Self::parse_date_from_time(&record.time);
        let key = ChannelRecordKey {
            agent_id: record.agent_id.clone(),
            role_id: record.role_id.clone(),
            channel_id: record.channel_id.clone(),
            date: date.clone(),
        };

        let year_role_dir = self.ensure_year_role_dir(&record.agent_id, &record.role_id, &record.time).await?;
        let file_path = year_role_dir.join(format!("channel-{}-records-{}.jsonl", record.channel_id, date));

        let counter = self.channel_sn_counters.entry(key).or_insert_with(|| AtomicU64::new(0));
        let sn = counter.fetch_add(1, Ordering::Relaxed);

        let record_with_sn = ChannelRecordWithSn {
            user_id: record.user_id,
            time: record.time,
            msg_type: record.msg_type,
            content: record.content,
            sn,
        };

        let line = serde_json::to_string(&record_with_sn)? + "\n";
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?
            .write_all(line.as_bytes())
            .await?;

        Ok(sn)
    }

    pub async fn append_thinking_record(&self, record: ThinkingRecord) -> Result<u64> {
        let date = Self::parse_date_from_time(&record.time);
        let key = RecordKey {
            agent_id: record.agent_id.clone(),
            role_id: record.role_id.clone(),
            date: date.clone(),
        };

        let year_role_dir = self.ensure_year_role_dir(&record.agent_id, &record.role_id, &record.time).await?;
        let file_path = year_role_dir.join(format!("thinking-records-{}.jsonl", date));

        let counter = self.thinking_sn_counters.entry(key).or_insert_with(|| AtomicU64::new(0));
        let sn = counter.fetch_add(1, Ordering::Relaxed);

        let record_with_sn = ThinkingRecordWithSn {
            content: record.content,
            key: record.key,
            time: record.time,
            sn,
        };

        let line = serde_json::to_string(&record_with_sn)? + "\n";
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?
            .write_all(line.as_bytes())
            .await?;

        Ok(sn)
    }

    pub async fn append_tool_record(&self, record: ToolRecord) -> Result<u64> {
        let date = Self::parse_date_from_time(&record.time);
        let key = RecordKey {
            agent_id: record.agent_id.clone(),
            role_id: record.role_id.clone(),
            date: date.clone(),
        };

        let year_role_dir = self.ensure_year_role_dir(&record.agent_id, &record.role_id, &record.time).await?;
        let file_path = year_role_dir.join(format!("tool-records-{}.jsonl", date));

        let counter = self.tool_sn_counters.entry(key).or_insert_with(|| AtomicU64::new(0));
        let sn = counter.fetch_add(1, Ordering::Relaxed);

        let record_with_sn = ToolRecordWithSn {
            tool_name: record.tool_name,
            tool_params: record.tool_params,
            tool_result: record.tool_result,
            key: record.key,
            time: record.time,
            sn,
        };

        let line = serde_json::to_string(&record_with_sn)? + "\n";
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?
            .write_all(line.as_bytes())
            .await?;

        Ok(sn)
    }
}
