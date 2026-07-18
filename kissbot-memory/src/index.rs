use kissbot_api::{QueryChannelRequest, QueryRequest};
use std::sync::{OnceLock};

use crate::data::{ChannelParser, ChannelRecord, ChannelRecordKey, ChannelRecordResult, RecordKey, ThinkParser, ThinkRecord, ThinkRecordResult, ToolCallParser, ToolCallRecord, ToolCallRecordResult, ToolResultParser, ToolResultRecord, ToolResultRecordResult};
use crate::error::Result;
use kai_file::FileIndexContext;

pub struct MemoryIndexer {
    channel_indices: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRecordResult, ChannelParser>,
    think_indices: FileIndexContext<QueryRequest, RecordKey, ThinkRecord, ThinkRecordResult, ThinkParser>,
    tool_call_indices: FileIndexContext<QueryRequest, RecordKey, ToolCallRecord, ToolCallRecordResult, ToolCallParser>,
    tool_result_indices: FileIndexContext<QueryRequest, RecordKey, ToolResultRecord, ToolResultRecordResult, ToolResultParser>,
}

static MEMORY_INDEXER: OnceLock<MemoryIndexer> = OnceLock::new();

impl MemoryIndexer {
    pub fn new() -> Self {
        Self {
            channel_indices: FileIndexContext::new(ChannelParser {}),
            think_indices: FileIndexContext::new(ThinkParser {}),
            tool_call_indices: FileIndexContext::new(ToolCallParser {}),
            tool_result_indices: FileIndexContext::new(ToolResultParser {}),
        }
    }

    pub fn get() -> &'static Self {
        MEMORY_INDEXER.get_or_init(|| MemoryIndexer::new())
    }

    pub fn mark_channel_obsolete(&self, key: &ChannelRecordKey) {
        self.channel_indices.mark_obsolete(key);
    }

    pub fn mark_channel_all_obsolete(&self, key: &ChannelRecordKey) {
        self.channel_indices.mark_all_obsolete(key);
    }

    pub fn mark_think_obsolete(&self, key: &RecordKey) {
        self.think_indices.mark_obsolete(key);
    }

    pub fn mark_think_all_obsolete(&self, key: &RecordKey) {
        self.think_indices.mark_all_obsolete(key);
    }

    pub fn mark_tool_call_obsolete(&self, key: &RecordKey) {
        self.tool_call_indices.mark_obsolete(key);
    }

    pub fn mark_tool_call_all_obsolete(&self, key: &RecordKey) {
        self.tool_call_indices.mark_all_obsolete(key);
    }

    pub fn mark_tool_result_obsolete(&self, key: &RecordKey) {
        self.tool_result_indices.mark_obsolete(key);
    }

    pub fn mark_tool_result_all_obsolete(&self, key: &RecordKey) {
        self.tool_result_indices.mark_all_obsolete(key);
    }

    pub async fn query_channel_records(&self, query: QueryChannelRequest) -> Result<Vec<ChannelRecordResult>> {
        let result = self.channel_indices.query_all(query).await?;
        Ok(result)
    }

    pub async fn query_think_records(&self, query: QueryRequest) -> Result<Vec<ThinkRecordResult>> {
        let result = self.think_indices.query_all(query).await?;
        Ok(result)
    }

    pub async fn query_tool_call_records(&self, query: QueryRequest) -> Result<Vec<ToolCallRecordResult>> {
        let result = self.tool_call_indices.query_all(query).await?;
        Ok(result)
    }

    pub async fn query_tool_result_records(&self, query: QueryRequest) -> Result<Vec<ToolResultRecordResult>> {
        let result = self.tool_result_indices.query_all(query).await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;

    // ========== MemoryIndexer: mark + query ==========

    use std::sync::{Arc, Once, OnceLock};

    static TEST_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static INIT_CONFIG: Once = Once::new();

    /// Initializes the global Config singleton for tests using a temp directory.
    /// Safe to call multiple times — only the first call initializes.
    fn init_test_config() {
        let dir = TEST_DIR.get_or_init(|| tempfile::tempdir().unwrap());
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        // SAFETY: single-threaded test init
        // 用 Once 保护 env set_var 和 config 初始化（仅执行一次）
        INIT_CONFIG.call_once(|| {
            std::fs::write(&config_path, format!(r#"{{"memory":{{"root_dir":"{}"}}}}"#, root_dir_str)).unwrap();
            unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
            crate::Config::get();
        });
    }

    async fn append_jsonl(agent_id: &str, role_name: &str, filename: &str, date: &str, line: &str) {
        init_test_config();
        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(agent_id).await.unwrap();
        let year_role_dir = store_dir.join(format!("{}-{}", &date[..4], role_name));
        tokio::fs::create_dir_all(&year_role_dir).await.unwrap();
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(year_role_dir.join(filename))
            .await
            .unwrap();
        use tokio::io::AsyncWriteExt;
        f.write_all(line.as_bytes()).await.unwrap();
        f.write_all(b"\n").await.unwrap();
        f.flush().await.unwrap();
    }

    #[tokio::test]
    async fn test_mark_and_query_channel() {
        let agent_id = "agent1";
        let role_name = "default";
        let messenger_id = "telegram";
        let user_id = "u1";
        let group_id = "g1";
        let date = "2026-06-24";
        let dir = format!("{}-{}", &date[..4], role_name);
        let filename = format!("channel-{}={}={}-records-{}.jsonl", messenger_id, user_id, group_id, date);

        let key = ChannelRecordKey {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            messenger_id: Arc::new(messenger_id.to_string()),
            user_id: Arc::new(user_id.to_string()),
            group_id: Arc::new(group_id.to_string()),
            date: Arc::new(date.to_string()),
        };
        let query_range = |s: &str, e: &str| QueryChannelRequest {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            messenger_id: Arc::new(messenger_id.to_string()),
            user_id: Arc::new(user_id.to_string()),
            group_id: Arc::new(group_id.to_string()),
            start_time: Arc::new(format!("{} {}", date, s)),
            end_time: Arc::new(format!("{} {}", date, e)),
        };

        // timeline: 00:00:00 < A(08:00) < start(09:00) < B(10:00) < C(11:00) < end(13:00) < F(14:00)
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"msg_type":"text","content":"A","time":"2026-06-24 08:00:00","sn":1}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"msg_type":"text","content":"B","time":"2026-06-24 10:00:00","sn":2}"#).await;

        let indexer = MemoryIndexer::new();
        // query range excludes A, includes B
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content.as_str(), "B");

        // write C (in range) after MemoryIndexer created
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"msg_type":"text","content":"C","time":"2026-06-24 11:00:00","sn":3}"#).await;

        // mark + query — incremental load picks up C
        indexer.mark_channel_obsolete(&key);
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content.as_str(), "B");
        assert_eq!(results[1].content.as_str(), "C");

        // delete file, write D(before start), E(in range), F(after end)
        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(agent_id).await.unwrap();
        let file_path = store_dir.join(&dir).join(&filename);
        tokio::fs::remove_file(&file_path).await.unwrap();
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"msg_type":"text","content":"D","time":"2026-06-24 08:30:00","sn":4}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"msg_type":"text","content":"E","time":"2026-06-24 10:30:00","sn":5}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"msg_type":"text","content":"F","time":"2026-06-24 14:00:00","sn":6}"#).await;

        // mark_all — full rebuild, only E in range
        indexer.mark_channel_all_obsolete(&key);
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content.as_str(), "E");
    }

    #[tokio::test]
    async fn test_mark_and_query_think() {
        let agent_id = "agent1";
        let role_name = "default";
        let date = "2026-06-24";
        let filename = format!("think-records-{}.jsonl", date);

        let key = RecordKey {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            date: Arc::new(date.to_string()),
        };
        let query_range = |s: &str, e: &str| QueryRequest {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            start_time: Arc::new(format!("{} {}", date, s)),
            end_time: Arc::new(format!("{} {}", date, e)),
        };

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"content":"A","key":"k1","time":"2026-06-24 08:00:00","sn":1}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"content":"B","key":"k1","time":"2026-06-24 10:00:00","sn":2}"#).await;

        let indexer = MemoryIndexer::new();
        let results = indexer.query_think_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content.as_str(), "B");

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"content":"C","key":"k1","time":"2026-06-24 11:00:00","sn":3}"#).await;
        indexer.mark_think_obsolete(&key);
        let results = indexer.query_think_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content.as_str(), "B");
        assert_eq!(results[1].content.as_str(), "C");

        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(agent_id).await.unwrap();
        let file_path = store_dir.join(format!("{}-{}", &date[..4], role_name)).join(&filename);
        tokio::fs::remove_file(&file_path).await.unwrap();
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"content":"D","key":"k1","time":"2026-06-24 08:30:00","sn":4}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"content":"E","key":"k1","time":"2026-06-24 10:30:00","sn":5}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"content":"F","key":"k1","time":"2026-06-24 14:00:00","sn":6}"#).await;

        indexer.mark_think_all_obsolete(&key);
        let results = indexer.query_think_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content.as_str(), "E");
    }

    #[tokio::test]
    async fn test_mark_and_query_tool_call() {
        let agent_id = "agent1";
        let role_name = "default";
        let date = "2026-06-24";
        let filename = format!("tool-call-records-{}.jsonl", date);

        let key = RecordKey {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            date: Arc::new(date.to_string()),
        };
        let query_range = |s: &str, e: &str| QueryRequest {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            start_time: Arc::new(format!("{} {}", date, s)),
            end_time: Arc::new(format!("{} {}", date, e)),
        };

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_name":"A","tool_params":{},"key":"k1","time":"2026-06-24 08:00:00","sn":1}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_name":"B","tool_params":{},"key":"k1","time":"2026-06-24 10:00:00","sn":2}"#).await;

        let indexer = MemoryIndexer::new();
        let results = indexer.query_tool_call_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name.as_str(), "B");

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_name":"C","tool_params":{},"key":"k1","time":"2026-06-24 11:00:00","sn":3}"#).await;
        indexer.mark_tool_call_obsolete(&key);
        let results = indexer.query_tool_call_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tool_name.as_str(), "B");
        assert_eq!(results[1].tool_name.as_str(), "C");

        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(agent_id).await.unwrap();
        let file_path = store_dir.join(format!("{}-{}", &date[..4], role_name)).join(&filename);
        tokio::fs::remove_file(&file_path).await.unwrap();
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_name":"D","tool_params":{},"key":"k1","time":"2026-06-24 08:30:00","sn":4}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_name":"E","tool_params":{},"key":"k1","time":"2026-06-24 10:30:00","sn":5}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_name":"F","tool_params":{},"key":"k1","time":"2026-06-24 14:00:00","sn":6}"#).await;

        indexer.mark_tool_call_all_obsolete(&key);
        let results = indexer.query_tool_call_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name.as_str(), "E");
    }

    #[tokio::test]
    async fn test_mark_and_query_tool_result() {
        let agent_id = "agent1";
        let role_name = "default";
        let date = "2026-06-24";
        let filename = format!("tool-result-records-{}.jsonl", date);

        let key = RecordKey {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            date: Arc::new(date.to_string()),
        };
        let query_range = |s: &str, e: &str| QueryRequest {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            start_time: Arc::new(format!("{} {}", date, s)),
            end_time: Arc::new(format!("{} {}", date, e)),
        };

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_result":{"v":"A"},"key":"k1","time":"2026-06-24 08:00:00","sn":1}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_result":{"v":"B"},"key":"k1","time":"2026-06-24 10:00:00","sn":2}"#).await;

        let indexer = MemoryIndexer::new();
        let results = indexer.query_tool_result_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 1);

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_result":{"v":"C"},"key":"k1","time":"2026-06-24 11:00:00","sn":3}"#).await;
        indexer.mark_tool_result_obsolete(&key);
        let results = indexer.query_tool_result_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 2);

        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(agent_id).await.unwrap();
        let file_path = store_dir.join(format!("{}-{}", &date[..4], role_name)).join(&filename);
        tokio::fs::remove_file(&file_path).await.unwrap();
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_result":{"v":"D"},"key":"k1","time":"2026-06-24 08:30:00","sn":4}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_result":{"v":"E"},"key":"k1","time":"2026-06-24 10:30:00","sn":5}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_result":{"v":"F"},"key":"k1","time":"2026-06-24 14:00:00","sn":6}"#).await;

        indexer.mark_tool_result_all_obsolete(&key);
        let results = indexer.query_tool_result_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results.len(), 1);
    }
}