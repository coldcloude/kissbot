use dashmap::DashMap;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::error::Result;

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
pub struct ChannelRecord {
    pub user_id: Arc<String>,
    pub time: Arc<String>,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingRequest {
    pub agent_id: String,
    pub role_name: String,
    pub content: String,
    pub key: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingRecord {
    pub content: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
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
pub struct ToolCallRecord {
    pub tool_name: Arc<String>,
    pub tool_params: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
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
pub struct ToolResultRecord {
    pub tool_result: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ChannelRecordKey {
    pub agent_id: Arc<String>,
    pub role_id: Arc<String>,
    pub channel_id: Arc<String>,
    pub date: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub agent_id: Arc<String>,
    pub role_id: Arc<String>,
    pub date: Arc<String>,
}

async fn ensure_year_role_dir(agent_id: &str, role_name: &str, time: &str) -> Result<PathBuf> {
    let year = &time[0..4];
    let store_dir = DirectoryManager::get().ensure_agent_store_dir(agent_id).await?;
    let year_role_dir = if role_name.is_empty() {
        store_dir.join(year)
    } else {
        store_dir.join(format!("{}-{}", year, role_name))
    };
    
    if !year_role_dir.exists() {
        tokio::fs::create_dir_all(&year_role_dir).await?;
    }
    
    Ok(year_role_dir)
}

fn parse_date_from_time(time: &str) -> String {
    time[0..10].to_string()
}

type FileState = Arc<Mutex<Option<u64>>>;

async fn get_lock<K>(files_map: &DashMap<K, FileState>, key: &K) -> FileState
where K: Eq + Hash + Clone + Send + Sync,
{
    files_map.entry(key.clone()).or_insert_with(|| Arc::new(Mutex::new(None))).clone()
}

async fn load_existing_sn(file_path: &PathBuf) -> Result<u64> {
    if !file_path.exists() {
        return Ok(0);
    }
    
    let mut max_sn = 0;

    let file = tokio::fs::File::open(file_path).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        if let Ok(record) = serde_json::from_str::<serde_json::Value>(line.as_str()) {
            if let Some(sn) = record.get("sn").and_then(|s| s.as_u64()) {
                if sn > max_sn {
                    max_sn = sn;
                }
            }
        }
    }

    Ok(max_sn + 1)
}

pub struct RecordManager {
    channel_states: DashMap<ChannelRecordKey, FileState>,
    thinking_states: DashMap<RecordKey, FileState>,
    tool_call_states: DashMap<RecordKey, FileState>,
    tool_result_states: DashMap<RecordKey, FileState>,
}

static RECORD_MANAGER: OnceLock<RecordManager> = OnceLock::new();

impl RecordManager {
    pub fn new() -> Self {
        Self {
            channel_states: DashMap::new(),
            thinking_states: DashMap::new(),
            tool_call_states: DashMap::new(),
            tool_result_states: DashMap::new(),
        }
    }

    pub fn get() -> &'static Self {
        RECORD_MANAGER.get_or_init(|| RecordManager::new())
    }

    pub async fn append_channel_record(&self, record: ChannelRequest) -> Result<u64> {
        let year_role_dir = ensure_year_role_dir(&record.agent_id, &record.role_name, &record.time).await?;
        let date = parse_date_from_time(&record.time);
        let file_path = year_role_dir.join(format!("channel-{}-records-{}.jsonl", &record.channel_id, &date));
        let key = ChannelRecordKey {
            agent_id: Arc::new(record.agent_id),
            role_id: Arc::new(record.role_name),
            channel_id: Arc::new(record.channel_id),
            date: Arc::new(date),
        };

        let lock = get_lock(&self.channel_states, &key).await;
        let mut gaurd = lock.lock().await;

        let sn = if let Some(old_sn) = *gaurd {
            old_sn
        } else {
            load_existing_sn(&file_path).await?
        };

        let record = ChannelRecord {
            user_id: Arc::new(record.user_id),
            time: Arc::new(record.time),
            msg_type: Arc::new(record.msg_type),
            content: Arc::new(record.content),
            sn,
        };

        let line = serde_json::to_string(&record)? + "\n";
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?
            .write_all(line.as_bytes())
            .await?;

        *gaurd = Some(sn);

        Ok(sn)
    }

    pub async fn append_thinking_record(&self, record: ThinkingRequest) -> Result<u64> {
        let year_role_dir = ensure_year_role_dir(&record.agent_id, &record.role_name, &record.time).await?;
        let date = parse_date_from_time(&record.time);
        let file_path = year_role_dir.join(format!("thinking-records-{}.jsonl", date));
        let key = RecordKey {
            agent_id: Arc::new(record.agent_id),
            role_id: Arc::new(record.role_name),
            date: Arc::new(date),
        };

        let lock = get_lock(&self.thinking_states, &key).await;
        let mut gaurd = lock.lock().await;

        let sn = if let Some(old_sn) = *gaurd {
            old_sn
        } else {
            load_existing_sn(&file_path).await?
        };

        let record = ThinkingRecord {
            content: Arc::new(record.content),
            key: Arc::new(record.key),
            time: Arc::new(record.time),
            sn,
        };

        let line = serde_json::to_string(&record)? + "\n";
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?
            .write_all(line.as_bytes())
            .await?;

        *gaurd = Some(sn);

        Ok(sn)
    }

    pub async fn append_tool_call_record(&self, record: ToolCallRequest) -> Result<u64> {
        let year_role_dir = ensure_year_role_dir(&record.agent_id, &record.role_name, &record.time).await?;
        let date = parse_date_from_time(&record.time);
        let file_path = year_role_dir.join(format!("tool-call-records-{}.jsonl", date));
        let key = RecordKey {
            agent_id: Arc::new(record.agent_id),
            role_id: Arc::new(record.role_name),
            date: Arc::new(date),
        };

        let lock = get_lock(&self.tool_call_states, &key).await;
        let mut gaurd = lock.lock().await;

        let sn = if let Some(old_sn) = *gaurd {
            old_sn
        } else {
            load_existing_sn(&file_path).await?
        };

        let record = ToolCallRecord {
            tool_name: Arc::new(record.tool_name),
            tool_params: Arc::new(record.tool_params),
            key: Arc::new(record.key),
            time: Arc::new(record.time),
            sn,
        };

        let line = serde_json::to_string(&record)? + "\n";
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?
            .write_all(line.as_bytes())
            .await?;

        *gaurd = Some(sn);

        Ok(sn)
    }

    pub async fn append_tool_result_record(&self, record: ToolResultRequest) -> Result<u64> {
        let year_role_dir = ensure_year_role_dir(&record.agent_id, &record.role_name, &record.time).await?;
        let date = parse_date_from_time(&record.time);
        let file_path = year_role_dir.join(format!("tool-result-records-{}.jsonl", date));
        let key = RecordKey {
            agent_id: Arc::new(record.agent_id),
            role_id: Arc::new(record.role_name),
            date: Arc::new(date),
        };

        let lock = get_lock(&self.tool_result_states, &key).await;
        let mut gaurd = lock.lock().await;

        let sn = if let Some(old_sn) = *gaurd {
            old_sn
        } else {
            load_existing_sn(&file_path).await?
        };

        let record = ToolResultRecord {
            tool_result: Arc::new(record.tool_result),
            key: Arc::new(record.key),
            time: Arc::new(record.time),
            sn,
        };

        let line = serde_json::to_string(&record)? + "\n";
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?
            .write_all(line.as_bytes())
            .await?;

        *gaurd = Some(sn);

        Ok(sn)
    }
}
