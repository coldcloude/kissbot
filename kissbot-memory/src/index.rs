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
        // 目录聚合模式：messenger/user/group 均为空串 → 按 agent+role 扫描全部 channel 文件
        if query.messenger_id.is_empty() && query.user_id.is_empty() && query.group_id.is_empty() {
            return self.query_channel_aggregate(query).await;
        }
        let mut result = self.channel_indices.query_all(query.clone()).await?;
        take_recent(&mut result, query.limit);
        Ok(result)
    }

    /// 目录聚合：枚举 <root>/<agent_id>/memory-store/<year>-<role_name>/channel-*.jsonl，
    /// 每文件 ReverseLineReader 尾部读取（上限 1024 行，覆盖最近窗口），合并按 (time, sn) 排序，
    /// 时间窗过滤，limit 截取最近 N（messenger/user/group 空串时使用，供 agent 记忆打包按 role 全量读取）
    async fn query_channel_aggregate(&self, query: QueryChannelRequest) -> Result<Vec<(ChannelRecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        use kai_file::ReverseLineReader;
        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(query.agent_id.as_str()).await?;
        // 收集所有 <year>-<role_name> 目录下的 channel 文件
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&store_dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if let Some((_, role)) = dir_name.split_once('-') {
                if role == query.role_name.as_str() && entry.path().is_dir() {
                    let mut year_dir = tokio::fs::read_dir(entry.path()).await?;
                    while let Some(f) = year_dir.next_entry().await? {
                        let fname = f.file_name().to_string_lossy().to_string();
                        if fname.starts_with("channel-") && fname.ends_with(".jsonl") {
                            files.push(f.path());
                        }
                    }
                }
            }
        }
        // 每文件尾部读（上限 1024 行）
        let mut merged: Vec<(u32, Arc<ChannelRecord>)> = Vec::new();
        for path in files {
            let mut reader = ReverseLineReader::new(&path, None, None).await?;
            let mut count = 0;
            while let Some(line_with_pos) = reader.next_line().await? {
                let s = line_with_pos.line.trim();
                if s.is_empty() { continue; }
                if let Ok(rec) = serde_json::from_str::<ChannelRecord>(s) {
                    merged.push((rec.sn as u32, Arc::new(rec)));
                    count += 1;
                    if count >= 1024 { break; }
                }
            }
        }
        // 按 (time, sn) 升序
        merged.sort_by(|a, b| {
            a.1.time.as_str().cmp(b.1.time.as_str()).then(a.1.sn.cmp(&b.1.sn))
        });
        // 时间窗过滤
        merged.retain(|(_, r)| {
            r.time.as_str() >= query.start_time.as_str() && r.time.as_str() <= query.end_time.as_str()
        });
        // limit 截取最近 N（时间升序，保留尾部）
        if let Some(limit) = query.limit {
            if merged.len() > limit {
                merged.drain(..merged.len() - limit);
            }
        }
        let key = ChannelRecordKey {
            agent_id: query.agent_id.clone(),
            role_name: query.role_name.clone(),
            messenger_id: Arc::new(String::new()),
            user_id: Arc::new(String::new()),
            group_id: Arc::new(String::new()),
            date: Arc::new(String::new()),
        };
        Ok(vec![(key, merged)])
    }

    pub async fn query_think_records(&self, query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ThinkRecord>)>)>> {
        let mut result = self.think_indices.query_all(query.clone()).await?;
        take_recent(&mut result, query.limit);
        Ok(result)
    }

    pub async fn query_tool_call_records(&self, query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ToolCallRecord>)>)>> {
        let mut result = self.tool_call_indices.query_all(query.clone()).await?;
        take_recent(&mut result, query.limit);
        Ok(result)
    }

    pub async fn query_tool_result_records(&self, query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ToolResultRecord>)>)>> {
        let mut result = self.tool_result_indices.query_all(query.clone()).await?;
        take_recent(&mut result, query.limit);
        Ok(result)
    }
}

/// 精确 key 路径 + limit：每组记录按 (time, sn) 排序后截取最近 N（保持现有返回结构）
fn take_recent<K, R>(grouped: &mut Vec<(K, Vec<(u32, Arc<R>)>)>, limit: Option<usize>)
where
    K: Clone + Send + Sync,
    R: kissbot_api::memory::MemoryRecord + Send + Sync + 'static,
{
    if let Some(limit) = limit {
        for (_, records) in grouped.iter_mut() {
            records.sort_by(|a, b| {
                a.1.time().cmp(b.1.time()).then(a.1.sn().cmp(&b.1.sn()))
            });
            if records.len() > limit {
                records.drain(..records.len() - limit);
            }
        }
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
            limit: None,
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
            limit: None,
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
            limit: None,
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
            limit: None,
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

    // ========== 目录聚合 + limit（messenger/user/group 空串 → 按 agent+role 扫描全部 channel 文件） ==========

    #[tokio::test]
    async fn test_query_channel_recent_with_limit_and_directory_aggregate() {
        let agent_id = "agg_agent";
        let role_name = "r1";
        let date = "2026-08-05";
        // 两个 channel 文件（不同 messenger），模拟该 role 下多个 channel 的历史
        let web_file = "channel-web=self1=g1-records-2026-08-05.jsonl";
        let tg_file = "channel-tg=self1=g1-records-2026-08-05.jsonl";
        let rec = |time: &str, text: &str| format!(
            r#"{{"user_id":"u1","is_self":0,"messenger_name":"","user_name":"name","group_name":"","content":{{"msg_type":"Text","data":"{}"}},"time":"{}","sn":1}}"#,
            text, time
        );
        append_jsonl(agent_id, role_name, web_file, date, &rec("2026-08-05 10:00:00", "m1-早")).await;
        append_jsonl(agent_id, role_name, tg_file, date, &rec("2026-08-05 10:01:00", "m2-中")).await;
        append_jsonl(agent_id, role_name, web_file, date, &rec("2026-08-05 10:02:00", "m1-晚")).await;

        let indexer = MemoryIndexer::new();
        // 目录聚合：messenger/user/group 空串 + limit=2（取最近 2 条）
        let query = QueryChannelRequest {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            messenger_id: Arc::new(String::new()),
            user_id: Arc::new(String::new()),
            group_id: Arc::new(String::new()),
            start_time: Arc::new("2026-08-05 00:00:00".to_string()),
            end_time: Arc::new("2026-08-05 23:59:59".to_string()),
            limit: Some(2),
        };
        let results = indexer.query_channel_records(query).await.unwrap();
        let mut flat: Vec<(String, String)> = Vec::new();  // (time, text)
        for (_, records) in &results {
            for (_, r) in records {
                if let Content::Text(t) = &r.content {
                    flat.push((r.time.to_string(), t.as_str().to_string()));
                }
            }
        }
        flat.sort();
        assert_eq!(flat.len(), 2, "limit=2 只返回最近 2 条");
        assert_eq!(flat[0].1, "m2-中");
        assert_eq!(flat[1].1, "m1-晚");
    }
}