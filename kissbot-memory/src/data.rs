use std::{cmp::Ordering, path::PathBuf, sync::Arc};

use chrono::{Duration, NaiveDate};
use crate::DirectoryManager;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use kissbot_api::*;

use crate::error::Result;

pub trait FileHook<K> {
    fn on_append(&self, key: &K);
    fn on_force_append(&self, key: &K);
}

//===================== Record in file ======================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub user_id: Arc<String>,
    pub is_self: usize,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub time: Arc<String>,
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

//===================== Record file key ======================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ChannelRecordKey {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub date: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub date: Arc<String>,
}

pub trait FileKey {
    fn agent_id(&self) -> &str;
    fn role_name(&self) -> &str;
    fn date(&self) -> &str;
}

macro_rules! impl_file_key {
    ($($t:ty),*) => {
        $(impl FileKey for $t {
            fn agent_id(&self) -> &str {
                self.agent_id.as_str()
            }
            fn role_name(&self) -> &str {
                self.role_name.as_str()
            }
            fn date(&self) -> &str {
                self.date.as_str()
            }
        })*
    };
}

impl_file_key!(
    ChannelRecordKey,
    RecordKey
);

//===================== Record result ======================

pub type ChannelRecordResult = ChannelRecordGeneric<SyncString>;

pub type ThinkRecordResult = ThinkRecordGeneric<SyncString>;

pub type ToolCallRecordResult = ToolCallRecordGeneric<SyncString, SyncValue>;

pub type ToolResultRecordResult = ToolResultRecordGeneric<SyncString, SyncValue>;

//===================== functions ======================

pub fn parse_date_from_time(time: &str) -> String {
    time[0..10].to_string()
}

pub fn get_internal_dates(start: &str, end: &str) -> Result<Vec<String>> {
    let start_date = NaiveDate::parse_from_str(start, "%Y-%m-%d")?;
    let end_date = NaiveDate::parse_from_str(end, "%Y-%m-%d")?;

    let mut dates = Vec::new();
    let mut current = start_date + Duration::days(1);
    while current < end_date {
        dates.push(current.format("%Y-%m-%d").to_string());
        current += Duration::days(1);
    }
    Ok(dates)
}

pub fn get_date_time_segments(start: &str, end: &str) -> Result<Vec<(String, String)>> {
    let start_date = parse_date_from_time(start);
    let end_date = parse_date_from_time(end);
    let internal_dates = get_internal_dates(start, end)?;

    let mut segments = Vec::new();
    segments.push((start.to_string(), format!("{} 23:59:59", start_date)));
    for date in internal_dates {
        segments.push((format!("{} 00:00:00", date.as_str()), format!("{} 23:59:59", date.as_str())));
    }
    segments.push((format!("{} 00:00:00", end_date), end.to_string()));
    Ok(segments)
}

pub async fn ensure_year_role_dir(agent_id: &str, role_name: &str, date: &str) -> Result<PathBuf> {
    let year = &date[0..4];
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

pub trait FilePathGenerator<K>
where
    K: FileKey,
{
    fn get_file_name(&self, key: &K) -> String;
}

pub trait RequestParser<Q,K,R>
where
    R: Record,
{
    fn parse_request(&self, request: Q) -> (K, R);
}

pub trait QueryParser<Q,K> {
    fn parse_query(&self, query: Q) -> Vec<(K, (String, String))>;
}

pub trait RecordCombiner<K,R,RR> {
    fn combine_record(&self, key: &K, record: &R) -> RR;
}

pub async fn ensure_file_path<K,P>(key: &K, parser: &P) -> Result<PathBuf>
where
    K: FileKey,
    P: FilePathGenerator<K>,
{
    let year_role_dir = ensure_year_role_dir(key.agent_id(), key.role_name(), key.date()).await?;
    let file_name = parser.get_file_name(key);
    let file_path = year_role_dir.join(file_name);
    Ok(file_path)
}

pub fn parse_query(query: QueryRequest) -> Vec<(RecordKey, (String, String))> {
    let mut results = Vec::new();
    if let Ok(date_times) = get_date_time_segments(&query.start_time, &query.end_time) {
        for time in date_times {
            let date = parse_date_from_time(&time.0);
            results.push((RecordKey {
                agent_id: Arc::new(query.agent_id.clone()),
                role_name: Arc::new(query.role_name.clone()),
                date: Arc::new(date),
            }, time));
        }
    }
    results
}

pub struct ChannelParser;

impl FilePathGenerator<ChannelRecordKey> for ChannelParser {
    fn get_file_name(&self, key: &ChannelRecordKey) -> String {
        format!("channel-{}={}={}-records-{}.jsonl", &key.messenger_id, &key.user_id, &key.group_id, &key.date)
    }
}

impl RequestParser<ChannelRequestDTO, ChannelRecordKey, ChannelRecord> for ChannelParser {
    fn parse_request(&self, request: ChannelRequestDTO) -> (ChannelRecordKey, ChannelRecord) {
        let user_id = Arc::new(request.user_id);
        let key = ChannelRecordKey {
            agent_id: Arc::new(request.agent_id),
            role_name: Arc::new(request.role_name),
            messenger_id: Arc::new(request.messenger_id),
            user_id: user_id.clone(),
            group_id: Arc::new(request.group_id),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ChannelRecord {
            user_id: user_id,
            is_self: request.is_self,
            msg_type: Arc::new(request.msg_type),
            content: Arc::new(request.content),
            time: Arc::new(request.time),
            sn: 0,
        };
        (key, record)
    }
}

impl QueryParser<QueryChannelRequest, ChannelRecordKey> for ChannelParser {
    fn parse_query(&self, query: QueryChannelRequest) -> Vec<(ChannelRecordKey, (String, String))> {
        let agent_id = Arc::new(query.agent_id.clone());
        let role_name = Arc::new(query.role_name.clone());
        let messenger_id = Arc::new(query.messenger_id.clone());
        let user_id = Arc::new(query.user_id.clone());
        let group_id = Arc::new(query.group_id.clone());
        let mut results = Vec::new();
        if let Ok(date_times) = get_date_time_segments(&query.start_time, &query.end_time) {
            for time in date_times {
                let date = parse_date_from_time(&time.0);
                results.push((ChannelRecordKey {
                    agent_id: agent_id.clone(),
                    role_name: role_name.clone(),
                    messenger_id: messenger_id.clone(),
                    user_id: user_id.clone(),
                    group_id: group_id.clone(),
                    date: Arc::new(date),
                }, time));
            }
        }
        results
    }
}

impl RecordCombiner<ChannelRecordKey, ChannelRecord, ChannelRecordResult> for ChannelParser {
    fn combine_record(&self, key: &ChannelRecordKey, record: &ChannelRecord) -> ChannelRecordResult {
        ChannelRecordResult {
            agent_id: key.agent_id.clone(),
            role_name: key.role_name.clone(),
            messenger_id: key.messenger_id.clone(),
            group_id: key.group_id.clone(),
            user_id: key.user_id.clone(),
            is_self: record.is_self,
            msg_type: record.msg_type.clone(),
            content: record.content.clone(),
            time: record.time.clone(),
            sn: record.sn,
        }
    }
}

pub struct ThinkParser;

impl FilePathGenerator<RecordKey> for ThinkParser {
    fn get_file_name(&self, key: &RecordKey) -> String {
        format!("think-records-{}.jsonl", key.date)
    }
}

impl RequestParser<ThinkRequest, RecordKey, ThinkRecord> for ThinkParser {
    fn parse_request(&self, request: ThinkRequest) -> (RecordKey, ThinkRecord) {
        let key = RecordKey {
            agent_id: Arc::new(request.agent_id),
            role_name: Arc::new(request.role_name),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ThinkRecord {
            content: Arc::new(request.content),
            key: Arc::new(request.key),
            time: Arc::new(request.time),
            sn: 0,
        };
        (key, record)
    }
}

impl QueryParser<QueryRequest, RecordKey> for ThinkParser {
    fn parse_query(&self, query: QueryRequest) -> Vec<(RecordKey, (String, String))> {
        parse_query(query)
    }
}

impl RecordCombiner<RecordKey, ThinkRecord, ThinkRecordResult> for ThinkParser {
    fn combine_record(&self, key: &RecordKey, record: &ThinkRecord) -> ThinkRecordResult {
        ThinkRecordResult {
            agent_id: key.agent_id.clone(),
            role_name: key.role_name.clone(),
            content: record.content.clone(),
            key: record.key.clone(),
            time: record.time.clone(),
            sn: record.sn,
        }
    }
}

pub struct ToolCallParser;

impl FilePathGenerator<RecordKey> for ToolCallParser {
    fn get_file_name(&self, key: &RecordKey) -> String {
        format!("tool-call-records-{}.jsonl", key.date)
    }
}

impl RequestParser<ToolCallRequest, RecordKey, ToolCallRecord> for ToolCallParser {
    fn parse_request(&self, request: ToolCallRequest) -> (RecordKey, ToolCallRecord) {
        let key = RecordKey {
            agent_id: Arc::new(request.agent_id),
            role_name: Arc::new(request.role_name),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ToolCallRecord {
            tool_name: Arc::new(request.tool_name),
            tool_params: Arc::new(request.tool_params),
            key: Arc::new(request.key),
            time: Arc::new(request.time),
            sn: 0,
        };
        (key, record)
    }
}

impl QueryParser<QueryRequest, RecordKey> for ToolCallParser {
    fn parse_query(&self, query: QueryRequest) -> Vec<(RecordKey, (String, String))> {
        parse_query(query)
    }
}

impl RecordCombiner<RecordKey, ToolCallRecord, ToolCallRecordResult> for ToolCallParser {
    fn combine_record(&self, key: &RecordKey, record: &ToolCallRecord) -> ToolCallRecordResult {
        ToolCallRecordResult {
            agent_id: key.agent_id.clone(),
            role_name: key.role_name.clone(),
            tool_name: record.tool_name.clone(),
            tool_params: record.tool_params.clone(),
            key: record.key.clone(),
            time: record.time.clone(),
            sn: record.sn,
        }
    }
}

pub struct ToolResultParser;

impl FilePathGenerator<RecordKey> for ToolResultParser {
    fn get_file_name(&self, key: &RecordKey) -> String {
        format!("tool-result-records-{}.jsonl", key.date)
    }
}

impl RequestParser<ToolResultRequest, RecordKey, ToolResultRecord> for ToolResultParser {
    fn parse_request(&self, request: ToolResultRequest) -> (RecordKey, ToolResultRecord) {
        let key = RecordKey {
            agent_id: Arc::new(request.agent_id),
            role_name: Arc::new(request.role_name),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ToolResultRecord {
            tool_result: Arc::new(request.tool_result),
            key: Arc::new(request.key),
            time: Arc::new(request.time),
            sn: 0,
        };
        (key, record)
    }
}

impl QueryParser<QueryRequest, RecordKey> for ToolResultParser {
    fn parse_query(&self, query: QueryRequest) -> Vec<(RecordKey, (String, String))> {
        parse_query(query)
    }
}

impl RecordCombiner<RecordKey, ToolResultRecord, ToolResultRecordResult> for ToolResultParser {
    fn combine_record(&self, key: &RecordKey, record: &ToolResultRecord) -> ToolResultRecordResult {
        ToolResultRecordResult {
            agent_id: key.agent_id.clone(),
            role_name: key.role_name.clone(),
            tool_result: record.tool_result.clone(),
            key: record.key.clone(),
            time: record.time.clone(),
            sn: record.sn,
        }
    }
}
