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
impl FilePathGenerator<RecordKey> for ChannelParser {
    async fn get_path(&self, key: &RecordKey) -> std::result::Result<PathBuf,kai_file::Error> {
        let year_role_dir = ensure_year_role_dir(key.agent_id.as_str(), key.role_name.as_str(), key.date.as_str()).await?;
        let file_name = format!("channel-records-{}.jsonl", key.date);
        Ok(year_role_dir.join(file_name))
    }
}

impl RequestParser<ChannelRequest, RecordKey, ChannelRecord> for ChannelParser {
    fn parse_request(&self, request: ChannelRequest) -> (RecordKey, ChannelRecord) {
        // 所有 channel 记录归入同一文件（每 agent+role+date）；
        // record 保存完整身份：user_id=发送者、self_user_id=agent 绑定用户（接收方）、messenger_id/group_id
        let key = RecordKey {
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            date: Arc::new(as_date(&request.time).to_string()),
        };
        let record = ChannelRecord {
            user_id: request.user_id.clone(),
            self_user_id: request.self_user_id.clone(),
            messenger_id: request.messenger_id.clone(),
            group_id: request.group_id.clone(),
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

impl QueryParser<QueryRequest, RecordKey> for ChannelParser {
    fn parse_query(&self, query: QueryRequest) -> Vec<(RecordKey, (String, String))> {
        parse_query(query)
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
            reasoning_content: request.reasoning_content.clone(),
            thinking: request.thinking.clone(),
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

    use std::sync::{Once, OnceLock};

    static TEST_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static INIT_CONFIG: Once = Once::new();

    /// 初始化全局 Config（临时目录），供依赖文件系统路径的测试使用（与 index.rs 测试同款模式）
    fn init_test_config() {
        let dir = TEST_DIR.get_or_init(|| tempfile::tempdir().unwrap());
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        // SAFETY: 单线程测试初始化，仅执行一次
        INIT_CONFIG.call_once(|| {
            std::fs::write(&config_path, format!(r#"{{"memory":{{"root_dir":"{}"}}}}"#, root_dir_str)).unwrap();
            unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
            crate::Config::get();
        });
    }

    #[tokio::test]
    async fn test_channel_file_name() {
        init_test_config();
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ChannelParser;
        let path = parser.get_path(&key).await.unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(file_name, "channel-records-2026-06-24.jsonl");
    }

    #[tokio::test]
    async fn test_channel_file_dir_empty_role() {
        // 空 role_name 目录应为 `2026-`（非 `2026`）
        init_test_config();
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("".to_string()),
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
            self_user_id: Arc::new("self1".to_string()),
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
        // key 为公共 RecordKey（无 messenger/user/group 字段）
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*key.role_name, "default");
        assert_eq!(*key.date, "2026-06-24");
        // record 保存完整身份：user_id=发送者、self_user_id=绑定用户、messenger_id/group_id
        assert_eq!(*record.user_id, "u1");
        assert_eq!(*record.self_user_id, "self1");
        assert_eq!(*record.messenger_id, "telegram");
        assert_eq!(*record.group_id, "g1");
        assert_eq!(record.is_self, 1);
        assert!(matches!(record.content, Content::Text(v) if v.as_str() == "hello"));
        assert_eq!(record.sn, 0);
    }

    #[test]
    fn test_think_request_parser() {
        let request = ThinkRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            reasoning_content: Arc::new("thinking...".to_string()),
            thinking: Arc::new(String::new()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ThinkParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*key.date, "2026-06-24");
        assert_eq!(*record.reasoning_content, "thinking...");
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
