use std::{cmp::Ordering, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use kai_date::{as_date, as_year, get_date_time_segments};
use kai_file::index::{FilePathGenerator, QueryParser, Record, RecordCombiner};
use crate::DirectoryManager;
use serde::{Deserialize, Serialize};

use kissbot_api::store::*;

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

pub trait MemoryRecord: Record {
    fn sn(&self) -> u64;
    fn set_sn(&mut self, sn: u64);
    fn time_string(&self) -> Arc<String>;
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = self.time().cmp(other.time());
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
            fn time(&self) -> &str {
                self.time.as_str()
            }
        })*
        $(impl MemoryRecord for $t {
            fn sn(&self) -> u64 {
                self.sn
            }
            fn set_sn(&mut self, sn: u64) {
                self.sn = sn;
            }
            fn time_string(&self) -> Arc<String> {
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

//===================== Record result ======================

pub type ChannelRecordResult = kissbot_api::store::ChannelRecord;

pub type ThinkRecordResult = kissbot_api::store::ThinkRecord;

pub type ToolCallRecordResult = kissbot_api::store::ToolCallRecord;

pub type ToolResultRecordResult = kissbot_api::store::ToolResultRecord;

//===================== functions ======================

pub async fn ensure_year_role_dir(agent_id: &str, role_name: &str, date: &str) -> std::result::Result<PathBuf,kai_file::Error> {
    let year = as_year(date);
    match DirectoryManager::get().ensure_agent_store_dir(agent_id).await {
        Ok(store_dir) => {
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
        Err(e) => {
            return Err(kai_file::Error::ExternalError(Box::new(e)));
        }
    }
}

pub trait RequestParser<Q,K,R>
where
    R: MemoryRecord,
{
    fn parse_request(&self, request: Q) -> (K, R);
}

pub fn parse_query(query: QueryRequest) -> Vec<(RecordKey, (String, String))> {
    let mut results = Vec::new();
    if let Ok(date_times) = get_date_time_segments(&query.start_time, &query.end_time) {
        for time in date_times {
            let date = as_date(&time.0);
            results.push((RecordKey {
                agent_id: query.agent_id.clone(),
                role_name: query.role_name.clone(),
                date: Arc::new(date.to_string()),
            }, time));
        }
    }
    results
}

pub struct ChannelParser;

#[async_trait]
impl FilePathGenerator<ChannelRecordKey> for ChannelParser {
    async fn get_path(&self, key: &ChannelRecordKey) -> std::result::Result<PathBuf,kai_file::Error> {
        let year_role_dir = ensure_year_role_dir(key.agent_id.as_str(), key.role_name.as_str(), key.date.as_str()).await?;
        let file_name = format!("channel-{}={}={}-records-{}.jsonl", key.messenger_id.as_str(), key.user_id.as_str(), key.group_id.as_str(), key.date.as_str());
        Ok(year_role_dir.join(file_name))
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
            date: Arc::new(as_date(&request.time).to_string()),
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
                let date = as_date(&time.0);
                results.push((ChannelRecordKey {
                    agent_id: agent_id.clone(),
                    role_name: role_name.clone(),
                    messenger_id: messenger_id.clone(),
                    user_id: user_id.clone(),
                    group_id: group_id.clone(),
                    date: Arc::new(date.to_string()),
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

#[async_trait]
impl FilePathGenerator<RecordKey> for ThinkParser {
    async fn get_path(&self, key: &RecordKey) -> std::result::Result<PathBuf,kai_file::Error> {
        let year_role_dir = ensure_year_role_dir(key.agent_id.as_str(), key.role_name.as_str(), key.date.as_str()).await?;
        let file_name = format!("think-records-{}.jsonl", key.date);
        Ok(year_role_dir.join(file_name))
    }
}

impl RequestParser<ThinkRequest, RecordKey, ThinkRecord> for ThinkParser {
    fn parse_request(&self, request: ThinkRequest) -> (RecordKey, ThinkRecord) {
        let key = RecordKey {
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            date: Arc::new(as_date(&request.time).to_string()),
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

#[async_trait]
impl FilePathGenerator<RecordKey> for ToolCallParser {
    async fn get_path(&self, key: &RecordKey) -> std::result::Result<PathBuf,kai_file::Error> {
        let year_role_dir = ensure_year_role_dir(key.agent_id.as_str(), key.role_name.as_str(), key.date.as_str()).await?;
        let file_name = format!("tool-call-records-{}.jsonl", key.date);
        Ok(year_role_dir.join(file_name))
    }
}

impl RequestParser<ToolCallRequest, RecordKey, ToolCallRecord> for ToolCallParser {
    fn parse_request(&self, request: ToolCallRequest) -> (RecordKey, ToolCallRecord) {
        let key = RecordKey {
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            date: Arc::new(as_date(&request.time).to_string()),
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

#[async_trait]
impl FilePathGenerator<RecordKey> for ToolResultParser {
    async fn get_path(&self, key: &RecordKey) -> std::result::Result<PathBuf,kai_file::Error> {
        let year_role_dir = ensure_year_role_dir(key.agent_id.as_str(), key.role_name.as_str(), key.date.as_str()).await?;
        let file_name = format!("tool-result-records-{}.jsonl", key.date);
        Ok(year_role_dir.join(file_name))
    }
}

impl RequestParser<ToolResultRequest, RecordKey, ToolResultRecord> for ToolResultParser {
    fn parse_request(&self, request: ToolResultRequest) -> (RecordKey, ToolResultRecord) {
        let key = RecordKey {
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            date: Arc::new(as_date(&request.time).to_string()),
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
        assert_eq!(channel.time(), "2026-06-24 10:00:00");
        channel.set_sn(10);
        assert_eq!(channel.sn(), 10);

        let think = ThinkRecord {
            content: Arc::new("think".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:01".to_string()),
            sn: 1,
        };
        assert_eq!(think.sn(), 1);
        assert_eq!(think.time(), "2026-06-24 10:00:01");
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

    #[tokio::test]
    async fn test_channel_file_name() {
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ChannelParser;
        let path = parser.get_path(&key).await.unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(file_name, "channel-m1=u1=g1-records-2026-06-24.jsonl");
    }

    #[tokio::test]
    async fn test_think_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ThinkParser;
        let path = parser.get_path(&key).await.unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(file_name, "think-records-2026-06-24.jsonl");
    }

    #[tokio::test]
    async fn test_tool_call_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ToolCallParser;
        let path = parser.get_path(&key).await.unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(file_name, "tool-call-records-2026-06-24.jsonl");
    }

    #[tokio::test]
    async fn test_tool_result_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ToolResultParser;
        let path = parser.get_path(&key).await.unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(file_name, "tool-result-records-2026-06-24.jsonl");
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
