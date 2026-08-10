use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use kissbot_api::QueryRequest;
use tokio::sync::OnceCell;

use crate::data::{ChannelParser, ThinkParser, ToolCallParser, ToolResultParser};
use crate::error::Result;
use crate::DirectoryManager;
use kai_file::FileIndexContext;

use kissbot_api::memory::*;

pub struct MemoryIndexer {
    channel_indices: FileIndexContext<QueryRequest, RecordKey, ChannelRecord, ChannelParser>,
    think_indices: FileIndexContext<QueryRequest, RecordKey, ThinkRecord, ThinkParser>,
    tool_call_indices: FileIndexContext<QueryRequest, RecordKey, ToolCallRecord, ToolCallParser>,
    tool_result_indices: FileIndexContext<QueryRequest, RecordKey, ToolResultRecord, ToolResultParser>,
    /// channel 文件日期缓存：(agent_id, role_name) → 已存在的日期（懒加载扫描 + append 钩子增量维护）
    channel_date_sets: DashMap<(String, String), BTreeSet<String>>,
    /// date_sets 懒加载守卫（首次 recent 查询前扫描一次存量文件）
    channel_dates_loaded: OnceCell<()>,
}

static MEMORY_INDEXER: OnceLock<MemoryIndexer> = OnceLock::new();

impl MemoryIndexer {
    pub fn new() -> Self {
        Self {
            channel_indices: FileIndexContext::new(ChannelParser {}),
            think_indices: FileIndexContext::new(ThinkParser {}),
            tool_call_indices: FileIndexContext::new(ToolCallParser {}),
            tool_result_indices: FileIndexContext::new(ToolResultParser {}),
            channel_date_sets: DashMap::new(),
            channel_dates_loaded: OnceCell::new(),
        }
    }

    pub fn get() -> &'static Self {
        MEMORY_INDEXER.get_or_init(|| MemoryIndexer::new())
    }

    pub fn mark_channel_obsolete(&self, key: &RecordKey) {
        self.channel_indices.mark_obsolete(key);
        // date_sets 增量维护：append 后该日期的 channel 文件必然存在（与懒加载扫描幂等）
        self.channel_date_sets.entry((key.agent_id.as_str().to_string(), key.role_name.as_str().to_string()))
            .or_default().insert(key.date.as_str().to_string());
    }

    pub fn mark_channel_all_obsolete(&self, key: &RecordKey) {
        self.channel_indices.mark_all_obsolete(key);
        // 全量重写后文件仍存在（日期不变），同样补入 date_sets
        self.channel_date_sets.entry((key.agent_id.as_str().to_string(), key.role_name.as_str().to_string()))
            .or_default().insert(key.date.as_str().to_string());
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

    pub async fn query_channel_records(&self, query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        Ok(self.channel_indices.query_all(query).await?)
    }

    /// 最近 N 条 channel 记录（跨日期文件，参考 channel-web message_store::get_recent）：
    /// date_sets 日期倒序逐个 query_last(remaining)，取满即停；组按日期升序返回、组内升序；无时间过滤
    pub async fn query_channel_recent(&self, agent_id: &str, role_name: &str, count: u32) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        // 懒加载：首次 recent 查询前扫描一次存量 channel 文件（此后由 mark_channel_obsolete/all_obsolete 增量维护）
        // 扫描为尽力而为：失败仅影响存量发现，增量 append 仍可用，故忽略结果
        let _ = self.channel_dates_loaded.get_or_init(|| async {
            let _ = self.scan_channel_dates().await;
        }).await;

        let mut remaining = count;
        let mut results: Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)> = Vec::new();
        if let Some(dates) = self.channel_date_sets.get(&(agent_id.to_string(), role_name.to_string())) {
            for date in dates.iter().rev() {  // 最新日期在前
                if remaining == 0 { break; }
                let key = RecordKey {
                    agent_id: Arc::new(agent_id.to_string()),
                    role_name: Arc::new(role_name.to_string()),
                    date: Arc::new(date.clone()),
                };
                let msgs = self.channel_indices.query_last(&key, remaining).await?;
                if !msgs.is_empty() {
                    remaining -= msgs.len() as u32;
                    results.push((key, msgs));
                }
            }
        }
        results.reverse();  // 日期倒序收集 → 升序返回（与 query_all 一致）
        Ok(results)
    }

    /// 扫描存量 channel 文件填充 date_sets：枚举 <root>/<agent_id>/memory-store/<year>-<role_name>/channel-records-<date>.jsonl
    async fn scan_channel_dates(&self) -> Result<()> {
        let root = DirectoryManager::get().root_dir().to_path_buf();
        let mut agent_entries = tokio::fs::read_dir(&root).await?;
        while let Some(agent_entry) = agent_entries.next_entry().await? {
            if !agent_entry.path().is_dir() { continue; }
            let Some(agent_id) = agent_entry.file_name().to_str().map(String::from) else { continue; };
            // 真实 agent 判断：有 memory-store 子目录（store 写路径只建 memory-store，不建 uuid 文件，
            // 故不能照抄 list_agents 的 uuid 过滤；read_dir 失败（无 store 目录）自然跳过）
            let store_dir = agent_entry.path().join("memory-store");
            let mut year_entries = match tokio::fs::read_dir(&store_dir).await {
                Ok(d) => d,
                Err(_) => continue,  // 无 store 目录 → 跳过该 agent
            };
            while let Some(year_entry) = year_entries.next_entry().await? {
                if !year_entry.path().is_dir() { continue; }
                // year-role 目录形如 "2026-default"；role = 去掉 "YYYY-" 前缀
                let year_name = year_entry.file_name().to_string_lossy().to_string();
                let Some(role_name) = year_name.get(5..).map(String::from) else { continue; };
                let mut file_entries = tokio::fs::read_dir(year_entry.path()).await?;
                while let Some(file_entry) = file_entries.next_entry().await? {
                    let name = file_entry.file_name().to_string_lossy().to_string();
                    if let Some(date) = name.strip_prefix("channel-records-").and_then(|n| n.strip_suffix(".jsonl")) {
                        self.channel_date_sets.entry((agent_id.clone(), role_name.clone()))
                            .or_default().insert(date.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn query_think_records(&self, query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ThinkRecord>)>)>> {
        Ok(self.think_indices.query_all(query).await?)
    }

    pub async fn query_tool_call_records(&self, query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ToolCallRecord>)>)>> {
        Ok(self.tool_call_indices.query_all(query).await?)
    }

    pub async fn query_tool_result_records(&self, query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ToolResultRecord>)>)>> {
        Ok(self.tool_result_indices.query_all(query).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_api::Content;
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
        let date = "2026-06-24";
        let dir = format!("{}-{}", &date[..4], role_name);
        let filename = format!("channel-records-{}.jsonl", date);

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

        // timeline: 00:00:00 < A(08:00) < start(09:00) < B(10:00) < C(11:00) < end(13:00) < F(14:00)
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"A"},"time":"2026-06-24 08:00:00","sn":1}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"B"},"time":"2026-06-24 10:00:00","sn":2}"#).await;

        let indexer = MemoryIndexer::new();
        // query range excludes A, includes B
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 1);
        assert!(matches!(results[0].1[0].1.content.clone(), Content::Text(v) if v.as_str() == "B"));

        // write C (in range) after MemoryIndexer created
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"C"},"time":"2026-06-24 11:00:00","sn":3}"#).await;

        // mark + query — incremental load picks up C
        indexer.mark_channel_obsolete(&key);
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 2);
        assert!(matches!(results[0].1[0].1.content.clone(), Content::Text(v) if v.as_str() == "B"));
        assert!(matches!(results[0].1[1].1.content.clone(), Content::Text(v) if v.as_str() == "C"));

        // delete file, write D(before start), E(in range), F(after end)
        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(agent_id).await.unwrap();
        let file_path = store_dir.join(&dir).join(&filename);
        tokio::fs::remove_file(&file_path).await.unwrap();
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"D"},"time":"2026-06-24 08:30:00","sn":4}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"E"},"time":"2026-06-24 10:30:00","sn":5}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"F"},"time":"2026-06-24 14:00:00","sn":6}"#).await;

        // mark_all — full rebuild, only E in range
        indexer.mark_channel_all_obsolete(&key);
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 1);
        assert!(matches!(results[0].1[0].1.content.clone(), Content::Text(v) if v.as_str() == "E"));
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
            r#"{"reasoning_content":"A","thinking":"","key":"k1","time":"2026-06-24 08:00:00","sn":1}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"reasoning_content":"B","thinking":"","key":"k1","time":"2026-06-24 10:00:00","sn":2}"#).await;

        let indexer = MemoryIndexer::new();
        let results = indexer.query_think_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 1);
        assert_eq!(results[0].1[0].1.reasoning_content.as_str(), "B");

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"reasoning_content":"C","thinking":"","key":"k1","time":"2026-06-24 11:00:00","sn":3}"#).await;
        indexer.mark_think_obsolete(&key);
        let results = indexer.query_think_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 2);
        assert_eq!(results[0].1[0].1.reasoning_content.as_str(), "B");
        assert_eq!(results[0].1[1].1.reasoning_content.as_str(), "C");

        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(agent_id).await.unwrap();
        let file_path = store_dir.join(format!("{}-{}", &date[..4], role_name)).join(&filename);
        tokio::fs::remove_file(&file_path).await.unwrap();
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"reasoning_content":"D","thinking":"","key":"k1","time":"2026-06-24 08:30:00","sn":4}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"reasoning_content":"E","thinking":"","key":"k1","time":"2026-06-24 10:30:00","sn":5}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"reasoning_content":"F","thinking":"","key":"k1","time":"2026-06-24 14:00:00","sn":6}"#).await;

        indexer.mark_think_all_obsolete(&key);
        let results = indexer.query_think_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 1);
        assert_eq!(results[0].1[0].1.reasoning_content.as_str(), "E");
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
        assert_eq!(results[0].1.len(), 1);
        assert_eq!(results[0].1[0].1.tool_name.as_str(), "B");

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_name":"C","tool_params":{},"key":"k1","time":"2026-06-24 11:00:00","sn":3}"#).await;
        indexer.mark_tool_call_obsolete(&key);
        let results = indexer.query_tool_call_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 2);
        assert_eq!(results[0].1[0].1.tool_name.as_str(), "B");
        assert_eq!(results[0].1[1].1.tool_name.as_str(), "C");

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
        assert_eq!(results[0].1.len(), 1);
        assert_eq!(results[0].1[0].1.tool_name.as_str(), "E");
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
        assert_eq!(results[0].1.len(), 1);

        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"tool_result":{"v":"C"},"key":"k1","time":"2026-06-24 11:00:00","sn":3}"#).await;
        indexer.mark_tool_result_obsolete(&key);
        let results = indexer.query_tool_result_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 2);

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
        assert_eq!(results[0].1.len(), 1);
    }

    // 提取 ChannelRecord 的 Text 内容（测试断言用）
    fn text_of(r: &Arc<ChannelRecord>) -> String {
        match &r.content {
            Content::Text(t) => t.as_str().to_string(),
            _ => String::new(),
        }
    }

    #[tokio::test]
    async fn test_query_channel_recent_cross_files() {
        init_test_config();
        // 用独立 agent id 避免与其他测试共享全局 temp root 时相互污染
        let agent_id = "recent_agent";
        let role_name = "r1";
        let rec = |time: &str, text: &str| format!(
            r#"{{"user_id":"u1","self_user_id":"self1","messenger_id":"web","group_id":"g1","is_self":0,"messenger_name":"","user_name":"u","group_name":"","content":{{"msg_type":"Text","data":"{}"}},"time":"{}","sn":1}}"#,
            text, time);
        append_jsonl(agent_id, role_name, "channel-records-2026-08-01.jsonl", "2026-08-01", &rec("2026-08-01 09:00:00", "a")).await;
        append_jsonl(agent_id, role_name, "channel-records-2026-08-01.jsonl", "2026-08-01", &rec("2026-08-01 10:00:00", "b")).await;
        append_jsonl(agent_id, role_name, "channel-records-2026-08-02.jsonl", "2026-08-02", &rec("2026-08-02 09:00:00", "c")).await;
        append_jsonl(agent_id, role_name, "channel-records-2026-08-03.jsonl", "2026-08-03", &rec("2026-08-03 09:00:00", "d")).await;
        append_jsonl(agent_id, role_name, "channel-records-2026-08-03.jsonl", "2026-08-03", &rec("2026-08-03 10:00:00", "e")).await;

        let indexer = MemoryIndexer::new();
        // 懒加载扫描 → 跨文件取最近 3 条（c, d, e），按时间升序
        let results = indexer.query_channel_recent(agent_id, role_name, 3).await.unwrap();
        let flat: Vec<String> = results.iter().flat_map(|(_, v)| v.iter()).map(|(_, r)| text_of(r)).collect();
        assert_eq!(flat, vec!["c", "d", "e"], "跨日期文件取最近 3 条");

        // count 超总量 → 全部（按时间升序）
        let results = indexer.query_channel_recent(agent_id, role_name, 100).await.unwrap();
        let flat: Vec<String> = results.iter().flat_map(|(_, v)| v.iter()).map(|(_, r)| text_of(r)).collect();
        assert_eq!(flat, vec!["a", "b", "c", "d", "e"], "count 超总量返回全部");

        // count == 0 → 空
        assert!(indexer.query_channel_recent(agent_id, role_name, 0).await.unwrap().is_empty(), "count=0 返回空");
    }

    #[tokio::test]
    async fn test_query_channel_recent_incremental_after_obsolete() {
        init_test_config();
        let agent_id = "recent_agent2";
        let role_name = "r1";
        let rec = |time: &str, text: &str| format!(
            r#"{{"user_id":"u1","self_user_id":"self1","messenger_id":"web","group_id":"g1","is_self":0,"messenger_name":"","user_name":"u","group_name":"","content":{{"msg_type":"Text","data":"{}"}},"time":"{}","sn":1}}"#,
            text, time);
        append_jsonl(agent_id, role_name, "channel-records-2026-08-01.jsonl", "2026-08-01", &rec("2026-08-01 09:00:00", "a")).await;

        let indexer = MemoryIndexer::new();
        let out = indexer.query_channel_recent(agent_id, role_name, 10).await.unwrap();
        assert_eq!(out.iter().map(|(_, v)| v.len()).sum::<usize>(), 1);

        // 新日期文件 + mark_channel_obsolete（append 钩子路径）→ date_sets 增量补入
        append_jsonl(agent_id, role_name, "channel-records-2026-08-02.jsonl", "2026-08-02", &rec("2026-08-02 09:00:00", "b")).await;
        let key = RecordKey {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            date: Arc::new("2026-08-02".to_string()),
        };
        indexer.mark_channel_obsolete(&key);
        let out = indexer.query_channel_recent(agent_id, role_name, 10).await.unwrap();
        let flat: Vec<String> = out.iter().flat_map(|(_, v)| v.iter()).map(|(_, r)| text_of(r)).collect();
        assert_eq!(flat, vec!["a", "b"], "mark_channel_obsolete 后新日期可查");
    }
}