use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use kai_date::{as_date, as_year, get_date_time_segments};
use kai_file::index::{FilePathGenerator, QueryParser};
use crate::DirectoryManager;

use kissbot_api::memory::*;

pub trait FileHook<K> {
    fn on_append(&self, key: &K);
    fn on_force_append(&self, key: &K);
}

//===================== functions ======================

pub async fn ensure_year_role_dir(agent_id: &str, role_name: &str, date: &str) -> std::result::Result<PathBuf,kai_file::Error> {
    let year = as_year(date);
    match DirectoryManager::get().ensure_agent_store_dir(agent_id).await {
        Ok(store_dir) => {
            let year_role_dir = store_dir.join(format!("{}-{}", year, role_name));
            
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

#[derive(Clone)]
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
            messenger_name: request.messenger_name.clone(),
            user_name: request.user_name.clone(),
            group_name: request.group_name.clone(),
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

#[derive(Clone)]
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

#[derive(Clone)]
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

#[derive(Clone)]
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

#[cfg(test)]
mod tests {
    use kissbot_api::Content;

use super::*;

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
    async fn test_channel_file_dir_empty_role() {
        // 空 role_name 目录应为 `2026-`（非 `2026`）
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ChannelParser;
        let path = parser.get_path(&key).await.unwrap();
        let dir_name = path.parent().unwrap().file_name().unwrap().to_str().unwrap();
        assert_eq!(dir_name, "2026-");
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
            messenger_name: Arc::new("TelegramName".to_string()),
            user_name: Arc::new("U1Name".to_string()),
            group_name: Arc::new("G1Name".to_string()),
            content: Content::Text(Arc::new("hello".to_string())),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ChannelParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*key.messenger_id, "telegram");
        assert_eq!(*key.date, "2026-06-24");
        assert_eq!(*record.user_id, "u1");
        assert!(matches!(record.content, Content::Text(v) if v.as_str() == "hello"));
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
}
