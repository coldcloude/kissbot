use kissbot_api::{QueryChannelRequest, QueryRequest};
use std::sync::{Arc, OnceLock};

use crate::data::{ChannelParser, ThinkParser, ToolCallParser, ToolResultParser};
use crate::error::Result;
use kai_file::FileIndexContext;

use kissbot_api::memory::*;

pub struct MemoryIndexer {
    channel_indices: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser>,
    think_indices: FileIndexContext<QueryRequest, RecordKey, ThinkRecord, ThinkParser>,
    tool_call_indices: FileIndexContext<QueryRequest, RecordKey, ToolCallRecord, ToolCallParser>,
    tool_result_indices: FileIndexContext<QueryRequest, RecordKey, ToolResultRecord, ToolResultParser>,
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

    pub async fn query_channel_records(&self, query: QueryChannelRequest) -> Result<Vec<(ChannelRecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        Ok(self.channel_indices.query_all(query).await?)
    }

    /// 枚举 <root>/<agent_id>/memory-store/<year>-<role_name>/channel-*.jsonl，
    /// 解析文件名中的 (messenger, user, group) 组合（按文件日期过滤在时间范围内），去重返回
    /// 供 agent 先取组合、再对每个组合用精确 key 时间区间查询（记忆打包流程）
    pub async fn query_combos(&self, query: QueryRequest) -> Result<Vec<ChannelCombo>> {
        use kai_date::as_date;
        // 时间戳短于日期长度时 as_date 切片会 panic——视为无有效范围，直接返回（防御客户端短输入，短路不触文件系统）
        if query.start_time.len() < 10 || query.end_time.len() < 10 {
            return Ok(Vec::new());
        }
        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(query.agent_id.as_str()).await?;
        let mut combos: Vec<ChannelCombo> = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&store_dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if let Some((_, role)) = dir_name.split_once('-') {
                if role == query.role_name.as_str() && entry.path().is_dir() {
                    let mut year_dir = tokio::fs::read_dir(entry.path()).await?;
                    while let Some(f) = year_dir.next_entry().await? {
                        let fname = f.file_name().to_string_lossy().to_string();
                        if !fname.starts_with("channel-") || !fname.ends_with(".jsonl") { continue; }
                        // 文件名：channel-{m}={u}={g}-records-{yyyy-mm-dd}.jsonl
                        let body = fname.trim_start_matches("channel-");
                        let Some((prefix, date)) = body.rsplit_once("-records-") else { continue; };
                        let date = date.trim_end_matches(".jsonl");
                        if date < as_date(query.start_time.as_str()) || date > as_date(query.end_time.as_str()) {
                            continue;  // 文件日期不在时间范围内
                        }
                        let mut parts = prefix.splitn(3, '=');
                        let (Some(m), Some(u), Some(g)) = (parts.next(), parts.next(), parts.next()) else { continue; };
                        combos.push(ChannelCombo {
                            messenger_id: Arc::new(m.to_string()),
                            user_id: Arc::new(u.to_string()),
                            group_id: Arc::new(g.to_string()),
                        });
                    }
                }
            }
        }
        // 去重（按组合三元组排序后相邻去重）
        combos.sort_by(|a, b| {
            a.messenger_id.as_str().cmp(b.messenger_id.as_str())
                .then(a.user_id.as_str().cmp(b.user_id.as_str()))
                .then(a.group_id.as_str().cmp(b.group_id.as_str()))
        });
        combos.dedup_by(|a, b| {
            a.messenger_id == b.messenger_id && a.user_id == b.user_id && a.group_id == b.group_id
        });
        Ok(combos)
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
            r#"{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"","group_name":"","msg_type":"text","content":{"msg_type":"Text","data":"A"},"time":"2026-06-24 08:00:00","sn":1}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"","group_name":"","msg_type":"text","content":{"msg_type":"Text","data":"B"},"time":"2026-06-24 10:00:00","sn":2}"#).await;

        let indexer = MemoryIndexer::new();
        // query range excludes A, includes B
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 1);
        assert!(matches!(results[0].1[0].1.content.clone(), Content::Text(v) if v.as_str() == "B"));

        // write C (in range) after MemoryIndexer created
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"","group_name":"","msg_type":"text","content":{"msg_type":"Text","data":"C"},"time":"2026-06-24 11:00:00","sn":3}"#).await;

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
            r#"{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"","group_name":"","msg_type":"text","content":{"msg_type":"Text","data":"D"},"time":"2026-06-24 08:30:00","sn":4}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"","group_name":"","msg_type":"text","content":{"msg_type":"Text","data":"E"},"time":"2026-06-24 10:30:00","sn":5}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"","group_name":"","msg_type":"text","content":{"msg_type":"Text","data":"F"},"time":"2026-06-24 14:00:00","sn":6}"#).await;

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

    // ========== 组合枚举（messenger/user/group 组合，供 agent 按组合精确查询） ==========

    #[tokio::test]
    async fn test_query_combos_enumerates_channel_files() {
        init_test_config();
        // 写两个 channel 文件（不同 messenger/user/group，同 role）
        let date = "2026-08-05";
        append_jsonl("combo_agent", "r1", "channel-web=self1=g1-records-2026-08-05.jsonl", date,
            r#"{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"n","group_name":"","content":{"msg_type":"Text","data":"hi"},"time":"2026-08-05 10:00:00","sn":1}"#).await;
        append_jsonl("combo_agent", "r1", "channel-tg=self2=g2-records-2026-08-05.jsonl", date,
            r#"{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"n","group_name":"","content":{"msg_type":"Text","data":"hi2"},"time":"2026-08-05 10:01:00","sn":1}"#).await;

        let indexer = MemoryIndexer::new();
        let query = QueryRequest {
            agent_id: Arc::new("combo_agent".into()),
            role_name: Arc::new("r1".into()),
            start_time: Arc::new("2026-08-05 00:00:00".into()),
            end_time: Arc::new("2026-08-05 23:59:59".into()),
        };
        let combos = indexer.query_combos(query).await.unwrap();
        assert_eq!(combos.len(), 2);
        let mut ids: Vec<(String, String, String)> = combos.iter().map(|c| {
            (c.messenger_id.to_string(), c.user_id.to_string(), c.group_id.to_string())
        }).collect();
        ids.sort();
        assert_eq!(ids[0], ("tg".into(), "self2".into(), "g2".into()));
        assert_eq!(ids[1], ("web".into(), "self1".into(), "g1".into()));
    }

    #[tokio::test]
    async fn test_query_combos_short_time_input_returns_empty() {
        // 时间戳短于日期长度（<10 字符）不应 panic，返回空（守卫短路）
        let indexer = MemoryIndexer::new();
        let query = QueryRequest {
            agent_id: Arc::new("combo_agent".into()),
            role_name: Arc::new("r1".into()),
            start_time: Arc::new("2026".into()),
            end_time: Arc::new("2026-08-05 23:59:59".into()),
        };
        let combos = indexer.query_combos(query).await.unwrap();
        assert!(combos.is_empty(), "短时间戳应返回空而非 panic");
    }
}