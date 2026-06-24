use std::{cmp::Ordering, path::PathBuf, sync::Arc};

use chrono::{Duration, NaiveDate};
use crate::DirectoryManager;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use kissbot_api::store::*;

use crate::error::{Error, Result};

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

pub type ChannelRecordResult = kissbot_api::store::ChannelRecord;

pub type ThinkRecordResult = kissbot_api::store::ThinkRecord;

pub type ToolCallRecordResult = kissbot_api::store::ToolCallRecord;

pub type ToolResultRecordResult = kissbot_api::store::ToolResultRecord;

//===================== functions ======================

pub fn parse_date_from_time(time: &str) -> String {
    time[0..10].to_string()
}

pub fn get_internal_dates(start: &str, end: &str) -> Result<Vec<String>> {
    if start > end {
        return Err(Error::InvalidTimeRange(start.to_string(), end.to_string()));
    }
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
    if start > end {
        return Err(Error::InvalidTimeRange(start.to_string(), end.to_string()));
    }
    let start_date = parse_date_from_time(start);
    let end_date = parse_date_from_time(end);

    if start_date == end_date {
        return Ok(vec![(start.to_string(), end.to_string())]);
    }

    let internal_dates = get_internal_dates(&start_date, &end_date)?;

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
                agent_id: query.agent_id.clone(),
                role_name: query.role_name.clone(),
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

impl RequestParser<ChannelRequest, ChannelRecordKey, ChannelRecord> for ChannelParser {
    fn parse_request(&self, request: ChannelRequest) -> (ChannelRecordKey, ChannelRecord) {
        let user_id = request.user_id.clone();
        let key = ChannelRecordKey {
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            messenger_id: request.messenger_id.clone(),
            user_id: user_id.clone(),
            group_id: request.group_id.clone(),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ChannelRecord {
            user_id: user_id,
            is_self: request.is_self,
            msg_type: request.msg_type.clone(),
            content: request.content.clone(),
            time: request.time.clone(),
            sn: 0,
        };
        (key, record)
    }
}

impl QueryParser<QueryChannelRequest, ChannelRecordKey> for ChannelParser {
    fn parse_query(&self, query: QueryChannelRequest) -> Vec<(ChannelRecordKey, (String, String))> {
        let agent_id = query.agent_id.clone();
        let role_name = query.role_name.clone();
        let messenger_id = query.messenger_id.clone();
        let user_id = query.user_id.clone();
        let group_id = query.group_id.clone();
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
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ThinkRecord {
            content: request.content.clone(),
            key: request.key.clone(),
            time: request.time.clone(),
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
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ToolCallRecord {
            tool_name: request.tool_name.clone(),
            tool_params: request.tool_params,
            key: request.key.clone(),
            time: request.time.clone(),
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
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ToolResultRecord {
            tool_result: request.tool_result,
            key: request.key.clone(),
            time: request.time.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Time functions ==========

    #[test]
    fn test_parse_date_from_time() {
        assert_eq!(parse_date_from_time("2026-06-24 15:30:00"), "2026-06-24");
        assert_eq!(parse_date_from_time("2026-01-01 00:00:00"), "2026-01-01");
    }

    #[test]
    fn test_get_internal_dates() {
        let dates = get_internal_dates("2026-06-22", "2026-06-25").unwrap();
        assert_eq!(dates, vec!["2026-06-23", "2026-06-24"]);
    }

    #[test]
    fn test_get_internal_dates_same_day() {
        let dates = get_internal_dates("2026-06-22", "2026-06-22").unwrap();
        assert!(dates.is_empty());
    }

    #[test]
    fn test_get_date_time_segments_multi_day() {
        let segments = get_date_time_segments("2026-06-22 14:30:00", "2026-06-24 10:00:00").unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].0, "2026-06-22 14:30:00");
        assert_eq!(segments[0].1, "2026-06-22 23:59:59");
        assert_eq!(segments[1].0, "2026-06-23 00:00:00");
        assert_eq!(segments[1].1, "2026-06-23 23:59:59");
        assert_eq!(segments[2].0, "2026-06-24 00:00:00");
        assert_eq!(segments[2].1, "2026-06-24 10:00:00");
    }

    #[test]
    fn test_get_date_time_segments_same_day() {
        let segments = get_date_time_segments("2026-06-22 14:30:00", "2026-06-22 15:00:00").unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].0, "2026-06-22 14:30:00");
        assert_eq!(segments[0].1, "2026-06-22 15:00:00");
    }

    #[test]
    fn test_get_date_time_segments_reversed() {
        let result = get_date_time_segments("2026-06-24 10:00:00", "2026-06-22 14:30:00");
        assert!(result.is_err());
    }

    // ========== Record trait ==========

    #[test]
    fn test_record_impl() {
        let mut channel = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 5,
        };
        assert_eq!(channel.sn(), 5);
        assert_eq!(*channel.time(), "2026-06-24 10:00:00");
        channel.set_sn(10);
        assert_eq!(channel.sn(), 10);

        let think = ThinkRecord {
            content: Arc::new("think".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:01".to_string()),
            sn: 1,
        };
        assert_eq!(think.sn(), 1);
        assert_eq!(*think.time(), "2026-06-24 10:00:01");
    }

    #[test]
    fn test_record_cmp_time() {
        let r1 = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let r2 = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("world".to_string()),
            time: Arc::new("2026-06-24 10:00:01".to_string()),
            sn: 1,
        };
        assert_eq!(r1.cmp(&r2), std::cmp::Ordering::Less);

        // same time, different sn
        let r3 = ChannelRecord {
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 2,
            ..r1.clone()
        };
        assert_eq!(r1.cmp(&r3), std::cmp::Ordering::Less);
    }

    // ========== Record serde ==========

    #[test]
    fn test_serde_channel_record() {
        let obj = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.user_id, "u1");
        assert_eq!(*deserialized.content, "hello");
        assert_eq!(deserialized.sn, 1);
    }

    #[test]
    fn test_serde_think_record() {
        let obj = ThinkRecord {
            content: Arc::new("think content".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.content, "think content");
        assert_eq!(*deserialized.key, "k1");
    }

    #[test]
    fn test_serde_tool_call_record() {
        let obj = ToolCallRecord {
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolCallRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.tool_name, "get_weather");
        assert_eq!(deserialized.tool_params["city"], "Beijing");
    }

    #[test]
    fn test_serde_tool_result_record() {
        let obj = ToolResultRecord {
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolResultRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.tool_result["temp"], 25);
    }

    // ========== Key serde ==========

    #[test]
    fn test_serde_channel_record_key() {
        let obj = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRecordKey = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.messenger_id, "telegram");
        assert_eq!(*deserialized.date, "2026-06-24");
    }

    #[test]
    fn test_serde_record_key() {
        let obj = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RecordKey = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.role_name, "default");
    }

    // ========== FilePathGenerator ==========

    #[test]
    fn test_channel_file_name() {
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ChannelParser;
        assert_eq!(parser.get_file_name(&key), "channel-m1=u1=g1-records-2026-06-24.jsonl");
    }

    #[test]
    fn test_think_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ThinkParser;
        assert_eq!(parser.get_file_name(&key), "think-records-2026-06-24.jsonl");
    }

    #[test]
    fn test_tool_call_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ToolCallParser;
        assert_eq!(parser.get_file_name(&key), "tool-call-records-2026-06-24.jsonl");
    }

    #[test]
    fn test_tool_result_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ToolResultParser;
        assert_eq!(parser.get_file_name(&key), "tool-result-records-2026-06-24.jsonl");
    }

    // ========== RequestParser ==========

    #[test]
    fn test_channel_request_parser() {
        let request = ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 1,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ChannelParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*key.messenger_id, "telegram");
        assert_eq!(*key.date, "2026-06-24");
        assert_eq!(*record.user_id, "u1");
        assert_eq!(*record.content, "hello");
        assert_eq!(record.sn, 0);
    }

    #[test]
    fn test_think_request_parser() {
        let request = ThinkRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            content: Arc::new("thinking...".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ThinkParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*key.date, "2026-06-24");
        assert_eq!(*record.content, "thinking...");
        assert_eq!(*record.key, "k1");
        assert_eq!(record.sn, 0);
    }

    #[test]
    fn test_tool_call_request_parser() {
        let request = ToolCallRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: serde_json::json!({"city": "Beijing"}).into(),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ToolCallParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*record.tool_name, "get_weather");
        assert_eq!(record.tool_params["city"], "Beijing");
        assert_eq!(record.sn, 0);
    }

    #[test]
    fn test_tool_result_request_parser() {
        let request = ToolResultRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            tool_result: serde_json::json!({"temp": 25}).into(),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ToolResultParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(record.tool_result["temp"], 25);
        assert_eq!(*record.key, "k1");
        assert_eq!(record.sn, 0);
    }

    // ========== RecordCombiner ==========

    #[test]
    fn test_channel_record_combiner() {
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let record = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 1,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 5,
        };
        let result = ChannelParser.combine_record(&key, &record);
        assert_eq!(*result.agent_id, "agent1");
        assert_eq!(*result.messenger_id, "telegram");
        assert_eq!(*result.content, "hello");
        assert_eq!(result.sn, 5);
    }

    #[test]
    fn test_think_record_combiner() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let record = ThinkRecord {
            content: Arc::new("thinking...".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 3,
        };
        let result = ThinkParser.combine_record(&key, &record);
        assert_eq!(*result.agent_id, "agent1");
        assert_eq!(*result.content, "thinking...");
        assert_eq!(result.sn, 3);
    }

    #[test]
    fn test_tool_call_record_combiner() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let record = ToolCallRecord {
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 2,
        };
        let result = ToolCallParser.combine_record(&key, &record);
        assert_eq!(*result.agent_id, "agent1");
        assert_eq!(*result.tool_name, "get_weather");
        assert_eq!(result.sn, 2);
    }

    #[test]
    fn test_tool_result_record_combiner() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let record = ToolResultRecord {
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 7,
        };
        let result = ToolResultParser.combine_record(&key, &record);
        assert_eq!(*result.agent_id, "agent1");
        assert_eq!(result.tool_result["temp"], 25);
        assert_eq!(result.sn, 7);
    }
}
