use async_trait::async_trait;
use dashmap::DashMap;
use futures::future;
use kai_file::index::{FilePathGenerator};
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt};
use tokio::sync::Mutex;
use tracing::*;

use kissbot_memory::data::{ChannelParser, FileHook, RequestParser, ThinkParser, ToolCallParser, ToolResultParser};
use kissbot_memory::index::MemoryIndexer;
use kissbot_api::memory::*;
use crate::error::{Error, Result};
use kai_file::ReverseLineReader;
use kai_file::{FileAppendWriter, FileAppendWriterContext, ErrorHandler, FileObjectAppender};

struct FileState {
    pub sn: u64,
    pub time: Arc<String>,
}

async fn load_existing_file_state(file_path: &PathBuf) -> Result<FileState> {
    let mut result = FileState {
        sn: 0,
        time: Arc::new(String::from("2000-01-01 00:00:00")),
    };

    if !file_path.exists() {
        return Ok(result);
    }

    let mut reader = ReverseLineReader::new(file_path, None, None).await?;

    while let Some(line_with_pos) = reader.next_line().await? {
        if !line_with_pos.line.is_empty() {
            if let Ok(record) = serde_json::from_str::<serde_json::Value>(line_with_pos.line.as_str()) {
                let sn_opt = record.get("sn").and_then(|s| s.as_u64());
                let time_opt = record.get("time").and_then(|s| s.as_str());
                if let Some(sn) = sn_opt {
                    if let Some(time) = time_opt {
                        result.sn = sn;
                        result.time = Arc::new(time.to_string());
                    }
                }
            }
            break;
        }
    }

    Ok(result)
}

struct RecordWriterContext<K, R, P, H> {
    _marker: PhantomData<(K, R)>,
    state: Option<FileState>,
    parser: Arc<P>,
    hook: Arc<H>,
}

impl<K, R, P, H> RecordWriterContext<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: MemoryRecord,
    P: FilePathGenerator<K>,
    H: FileHook<K>,
{
    pub fn new(parser: Arc<P>, hook: Arc<H>) -> Self {
        Self {
            _marker: PhantomData,
            state: None,
            parser,
            hook,
        }
    }
}

#[async_trait]
impl<K, R, P, H> FileAppendWriterContext<K, R> for RecordWriterContext<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    R: MemoryRecord + Send + Sync + 'static,
    P: FilePathGenerator<K> + Send + Sync + 'static,
    H: FileHook<K> + Send + Sync + 'static,
{
    async fn write(&mut self, key: &K, records: Vec<R>) -> std::result::Result<(), kai_file::Error> {
        let file_path = self.parser.get_path(key).await?;

        let file_state = self.state.get_or_insert(
            load_existing_file_state(&file_path).await
                .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?
        );

        // 分配 SN
        let mut records = records;
        for (i, record) in records.iter_mut().enumerate() {
            record.set_sn(file_state.sn + 1 + i as u64);
        }

        // 按 time 排序（相同 time 按 sn）
        records.sort_by(|a, b| a.cmp(b));

        if file_state.time.as_str() > records[0].time() {
            // 乱序 → 全量重写
            let mut all_records: Vec<R> = Vec::new();
            let file = tokio::fs::File::open(&file_path).await?;
            let reader = tokio::io::BufReader::new(file);
            let mut lines = reader.lines();
            while let Some(line) = lines.next_line().await? {
                let record: R = serde_json::from_str(&line)?;
                all_records.push(record);
            }
            all_records.extend(records);
            all_records.sort_by(|a, b| a.cmp(b));

            let mut file = tokio::fs::OpenOptions::new()
                .create(true).write(true).open(&file_path).await?;

            file_state.sn = 0;
            for record in &mut all_records {
                file_state.sn += 1;
                record.set_sn(file_state.sn);
                let line = serde_json::to_string(record)? + "\n";
                tokio::io::AsyncWriteExt::write_all(&mut file, line.as_bytes()).await?;
            }
            if let Some(last) = all_records.last() {
                file_state.time = last.time_string();
            }

            self.hook.on_force_append(key);
        } else {
            // 有序 → 追加
            let mut file = tokio::fs::OpenOptions::new()
                .create(true).append(true).open(&file_path).await?;

            for record in &mut records {
                file_state.sn += 1;
                record.set_sn(file_state.sn);
                let line = serde_json::to_string(record)? + "\n";
                tokio::io::AsyncWriteExt::write_all(&mut file, line.as_bytes()).await?;
                file_state.time = record.time_string();
            }

            self.hook.on_append(key);
        }

        Ok(())
    }
}

struct RecordAppendWriter<K, R, P, H>
{
    map: DashMap<K, Arc<Mutex<RecordWriterContext<K, R, P, H>>>>,
    parser: Arc<P>,
    hook: Arc<H>,
}

impl<K, R, P, H> RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: MemoryRecord,
    P: FilePathGenerator<K>,
    H: FileHook<K>,
{
    pub fn new(parser: Arc<P>, hook: Arc<H>) -> Self {
        Self {
            map: DashMap::new(),
            parser,
            hook,
        }
    }
}

#[async_trait]
impl<K, R, P, H> FileAppendWriter<K, R, RecordWriterContext<K, R, P, H>> for RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    R: MemoryRecord + Send + Sync + 'static,
    P: FilePathGenerator<K> + Send + Sync + 'static,
    H: FileHook<K> + Send + Sync + 'static,
{
    async fn get_lock(&self, key: &K) -> Arc<Mutex<RecordWriterContext<K, R, P, H>>> {
        if let Some(ctx) = self.map.get(key) {
            return ctx.value().clone();
        }
        self.map.entry(key.clone()).or_insert_with(|| {
            let ctx = RecordWriterContext::new(self.parser.clone(), self.hook.clone());
            Arc::new(Mutex::new(ctx))
        }).value().clone()
    }

    async fn remove_lock(&self, _key: &K) {
        // no op
    }
}

#[derive(Clone)]
struct ChannelFileIndexHook;

impl FileHook<RecordKey> for ChannelFileIndexHook {
    fn on_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_channel_obsolete(key);
    }

    fn on_force_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_channel_all_obsolete(key);
    }
}

#[derive(Clone)]
struct ThinkFileIndexHook;

impl FileHook<RecordKey> for ThinkFileIndexHook {
    fn on_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_think_obsolete(key);
    }

    fn on_force_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_think_all_obsolete(key);
    }
}

#[derive(Clone)]
struct ToolCallFileIndexHook;

impl FileHook<RecordKey> for ToolCallFileIndexHook {
    fn on_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_tool_call_obsolete(key);
    }

    fn on_force_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_tool_call_all_obsolete(key);
    }
}

#[derive(Clone)]
struct ToolResultFileIndexHook;

impl FileHook<RecordKey> for ToolResultFileIndexHook {
    fn on_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_tool_result_obsolete(key);
    }

    fn on_force_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_tool_result_all_obsolete(key);
    }
}

pub(crate) struct LogErrorHandler;

#[async_trait]
impl<K, R> ErrorHandler<K, R> for LogErrorHandler
where 
    K: Debug + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    async fn on_write_error(&self, key: &K, _batch: Vec<R>, error: &kai_file::Error) {
        error!("[memory-store] write error for key={:?}: {}", key, error);
    }
}

type RecordAppender<K,R,P,H> = FileObjectAppender<K, R, RecordAppendWriter<K, R, P, H>, RecordWriterContext<K, R, P, H>, LogErrorHandler>;

async fn append_record<Q,K,R,P,H>(requests: Vec<Q>, force: bool, parser: Arc<P>, appender: Arc<RecordAppender<K, R, P, H>>) -> Result<()>
where
    Q: Send + Sync + 'static,
    K: Debug + Eq + Hash + Send + Sync + Clone + 'static,
    R: MemoryRecord + Send + Sync + Clone + 'static,
    P: RequestParser<Q, K, R> + FilePathGenerator<K> + Send + Sync + 'static,
    H: FileHook<K> + Send + Sync + 'static,
{
    let mut records_map: HashMap<K, Vec<R>> = HashMap::new();
    for request in requests {
        let (key, record) = parser.parse_request(request);
        records_map.entry(key).or_default().push(record);
    }

    for (key, records) in records_map.iter() {
        let lock = appender.get_lock(&key).await;
        let mut gaurd = lock.lock().await;

        let file_path = parser.get_path(&key).await?;

        let state = if let Some(old_state) = gaurd.state.take() {
            old_state
        } else {
            load_existing_file_state(&file_path).await?
        };
        let time = state.time.clone();
        gaurd.state = Some(state);

        // 根据records生成方式，records非空
        let mut min_time = records[0].time();
        for record in records {
            if record.time() < min_time {
                min_time = record.time();
            }
        }

        if time.as_str() > min_time && !force {
            return Err(Error::RecordNotInOrder(
                format!("{:?}", key),
                time.as_str().to_string(),
                records[0].time().to_string(),
            ));
        }
    }

    let mut futs = Vec::new();
    for (key, records) in records_map.drain() {
        let f = appender.append(key, records);
        futs.push(f);
    }
    future::join_all(futs).await;

    Ok(())
}

type ChannelAppender = RecordAppender<RecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>;

type ThinkAppender = RecordAppender<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>;

type ToolCallAppender = RecordAppender<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>;

type ToolResultAppender = RecordAppender<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>;

pub struct RecordManager {
    channel_parser: Arc<ChannelParser>,
    channel_appender: Arc<ChannelAppender>,

    think_parser: Arc<ThinkParser>,
    think_appender: Arc<ThinkAppender>,

    tool_call_parser: Arc<ToolCallParser>,
    tool_call_appender: Arc<ToolCallAppender>,

    tool_result_parser: Arc<ToolResultParser>,
    tool_result_appender: Arc<ToolResultAppender>,
}

static RECORD_MANAGER: OnceLock<RecordManager> = OnceLock::new();

fn create_record_appender<K,R,P,H>(writer: Arc<RecordAppendWriter<K, R, P, H>>) -> RecordAppender<K, R, P, H>
where
    K: Debug + Eq + Hash + Send + Sync + Clone + 'static,
    R: MemoryRecord + Send + Sync + Clone + 'static,
    P: FilePathGenerator<K> + Send + Sync + 'static,
    H: FileHook<K> + Send + Sync + 'static,
{
    FileObjectAppender::new(
        writer.clone(),
        Arc::new(LogErrorHandler),
        Duration::from_millis(100),
        10,
    )
}

impl RecordManager {
    pub fn new() -> Self {
        let channel_parser = Arc::new(ChannelParser);
        let channel_writer = Arc::new(RecordAppendWriter::new(
            channel_parser.clone(),
            Arc::new(ChannelFileIndexHook)
        ));
        let channel_appender = Arc::new(create_record_appender(channel_writer.clone()));

        let think_parser = Arc::new(ThinkParser);
        let think_writer = Arc::new(RecordAppendWriter::new(
            think_parser.clone(),
            Arc::new(ThinkFileIndexHook),
        ));
        let think_appender = Arc::new(create_record_appender(think_writer.clone()));

        let tool_call_parser = Arc::new(ToolCallParser);
        let tool_call_writer = Arc::new(RecordAppendWriter::new(
            tool_call_parser.clone(),
            Arc::new(ToolCallFileIndexHook)
        ));
        let tool_call_appender = Arc::new(create_record_appender(tool_call_writer.clone()));

        let tool_result_parser = Arc::new(ToolResultParser);
        let tool_result_writer = Arc::new(RecordAppendWriter::new(
            tool_result_parser.clone(),
            Arc::new(ToolResultFileIndexHook),
        ));
        let tool_result_appender = Arc::new(create_record_appender(tool_result_writer.clone()));

        Self {
            channel_parser,
            channel_appender,
            think_parser,
            think_appender,
            tool_call_parser,
            tool_call_appender,
            tool_result_parser,
            tool_result_appender,
        }
    }

    pub fn get() -> &'static Self {
        RECORD_MANAGER.get_or_init(|| RecordManager::new())
    }

    pub async fn append_channel_record(&self, requests: Vec<ChannelRequest>, force: bool) -> Result<()> {
        append_record(requests, force, self.channel_parser.clone(), self.channel_appender.clone()).await
    }

    pub async fn append_think_record(&self, requests: Vec<ThinkRequest>, force: bool) -> Result<()> {
        append_record(requests, force, self.think_parser.clone(), self.think_appender.clone()).await
    }

    pub async fn append_tool_call_record(&self, requests: Vec<ToolCallRequest>, force: bool) -> Result<()> {
        append_record(requests, force, self.tool_call_parser.clone(), self.tool_call_appender.clone()).await
    }

    pub async fn append_tool_result_record(&self, requests: Vec<ToolResultRequest>, force: bool) -> Result<()> {
        append_record(requests, force, self.tool_result_parser.clone(), self.tool_result_appender.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    pub(crate) struct NoopFileHook;

    impl<K> FileHook<K> for NoopFileHook {
        fn on_append(&self, _key: &K) {}
        fn on_force_append(&self, _key: &K) {}
    }

    #[tokio::test]
    async fn test_load_state_file_not_exists() {
        let state = load_existing_file_state(&PathBuf::from("/tmp/nonexistent_file_xyz.jsonl")).await.unwrap();
        assert_eq!(state.sn, 0);
        assert_eq!(*state.time, "2000-01-01 00:00:00");
    }

    #[tokio::test]
    async fn test_load_state_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.jsonl");
        tokio::fs::write(&file_path, "").await.unwrap();

        let state = load_existing_file_state(&file_path).await.unwrap();
        assert_eq!(state.sn, 0);
        assert_eq!(*state.time, "2000-01-01 00:00:00");
    }

    #[tokio::test]
    async fn test_load_state_with_records() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("records.jsonl");

        // 写入 3 条 JSONL 记录
        let lines = vec![
            r#"{"sn":1,"time":"2026-06-25 10:00:00","content":"msg1"}"#,
            r#"{"sn":2,"time":"2026-06-25 10:01:00","content":"msg2"}"#,
            r#"{"sn":3,"time":"2026-06-25 10:02:00","content":"msg3"}"#,
        ];
        let content = lines.join("\n") + "\n";
        tokio::fs::write(&file_path, content).await.unwrap();

        let state = load_existing_file_state(&file_path).await.unwrap();
        assert_eq!(state.sn, 3);
        assert_eq!(*state.time, "2026-06-25 10:02:00");
    }

    use std::collections::HashMap;
    use std::sync::{Once, OnceLock};
    use std::time::Duration;
    use kissbot_api::Content;
    use kissbot_memory::Config as MemoryConfig;

    static TEST_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static INIT_CONFIG: Once = Once::new();

    fn init_test_config() {
        let dir = TEST_DIR.get_or_init(|| tempfile::tempdir().unwrap());
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        // 用 Once 保护 env set_var 和 config 初始化（仅执行一次）
        INIT_CONFIG.call_once(|| {
            let json = format!(r#"{{"memory": {{"root_dir": "{}"}}}}"#, root_dir_str);
            std::fs::write(&config_path, json).unwrap();
            unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
            MemoryConfig::get();
        });
    }

    #[tokio::test]
    async fn test_append_new_file() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(RecordAppendWriter::new(Arc::new(ChannelParser), Arc::new(NoopFileHook)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("test_append_new".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("hello".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
        ];

        let mut records_map: HashMap<RecordKey, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        // 验证文件被创建
        let expected_path = root
            .join("test_append_new")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        // 读取文件内容验证
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ChannelRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
        assert!(matches!(record.content, Content::Text(v) if v.as_str() == "hello"));
    }

    #[tokio::test]
    async fn test_append_multiple_records() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(RecordAppendWriter::new(Arc::new(ChannelParser), Arc::new(NoopFileHook)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("test_append_multi".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("msg1".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_append_multi".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("msg2".to_string())),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_append_multi".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("msg3".to_string())),
                time: Arc::new("2026-06-25 10:02:00".to_string()),
            },
        ];

        let mut records_map: HashMap<RecordKey, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        let expected_path = root
            .join("test_append_multi")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 3);

        // 验证 sn 顺序
        for (i, line) in lines.iter().enumerate() {
            let record: ChannelRecord = serde_json::from_str(line).unwrap();
            assert_eq!(record.sn(), (i + 1) as u64);
        }
    }

    #[tokio::test]
    async fn test_append_sequential() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(RecordAppendWriter::new(Arc::new(ChannelParser), Arc::new(NoopFileHook)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        // 第一次写入
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("test_append_seq".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("first".to_string())),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        let mut records_map: HashMap<RecordKey, Vec<ChannelRecord>> = HashMap::new();
        for request in req1 {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        // 第二次写入
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("test_append_seq".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("second".to_string())),
            time: Arc::new("2026-06-25 10:01:00".to_string()),
        }];
        let mut records_map: HashMap<RecordKey, Vec<ChannelRecord>> = HashMap::new();
        for request in req2 {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        let expected_path = root
            .join("test_append_seq")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);

        // 第一条 sn=1，第二条 sn=2
        let r1: ChannelRecord = serde_json::from_str(lines[0]).unwrap();
        let r2: ChannelRecord = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r1.sn(), 1);
        assert!(matches!(r1.content, Content::Text(v) if v.as_str() == "first"));
        assert_eq!(r2.sn(), 2);
        assert!(matches!(r2.content, Content::Text(v) if v.as_str() == "second"));
    }

    #[tokio::test]
    async fn test_append_think_record() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(RecordAppendWriter::new(Arc::new(ThinkParser), Arc::new(NoopFileHook)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        let requests = vec![ThinkRequest {
            agent_id: Arc::new("test_think".to_string()),
            role_name: Arc::new("".to_string()),
            reasoning_content: Arc::new("I think...".to_string()),
            thinking: Arc::new(String::new()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        let mut records_map: HashMap<RecordKey, Vec<ThinkRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ThinkParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        let expected_path = root
            .join("test_think")
            .join("memory-store")
            .join("2026-")
            .join("think-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ThinkRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
        assert_eq!(*record.reasoning_content, "I think...");
    }

    #[tokio::test]
    async fn test_append_tool_call_record() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(RecordAppendWriter::new(Arc::new(ToolCallParser), Arc::new(NoopFileHook)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        let requests = vec![ToolCallRequest {
            agent_id: Arc::new("test_tool_call".to_string()),
            role_name: Arc::new("".to_string()),
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        let mut records_map: HashMap<RecordKey, Vec<ToolCallRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ToolCallParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        let expected_path = root
            .join("test_tool_call")
            .join("memory-store")
            .join("2026-")
            .join("tool-call-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ToolCallRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
    }

    #[tokio::test]
    async fn test_append_tool_result_record() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(RecordAppendWriter::new(Arc::new(ToolResultParser), Arc::new(NoopFileHook)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        let requests = vec![ToolResultRequest {
            agent_id: Arc::new("test_tool_result".to_string()),
            role_name: Arc::new("".to_string()),
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        let mut records_map: HashMap<RecordKey, Vec<ToolResultRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ToolResultParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        let expected_path = root
            .join("test_tool_result")
            .join("memory-store")
            .join("2026-")
            .join("tool-result-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ToolResultRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
    }

    #[tokio::test]
    async fn test_append_out_of_order_rejected() {
        init_test_config();

        let rm = RecordManager::new();

        // 先写入一条 time=10:02:00
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("test_ooor".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("later".to_string())),
            time: Arc::new("2026-06-25 10:02:00".to_string()),
        }];
        rm.append_channel_record(req1, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // 再写入一条 time=10:00:00 — 早于已有记录，应该被拒绝
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("test_ooor".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("earlier".to_string())),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        let result = rm.append_channel_record(req2, false).await;
        assert!(result.is_err());
        match result {
            Err(Error::RecordNotInOrder(_, latest, new)) => {
                assert_eq!(latest, "2026-06-25 10:02:00");
                assert_eq!(new, "2026-06-25 10:00:00");
            }
            _ => panic!("expected RecordNotInOrder error"),
        }
    }

    #[tokio::test]
    async fn test_append_force_out_of_order() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let rm = RecordManager::new();

        // 先写入一条 time=10:02:00
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("test_force_ooo".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("later".to_string())),
            time: Arc::new("2026-06-25 10:02:00".to_string()),
        }];
        rm.append_channel_record(req1, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // force=true 写入一条 time=10:00:00 — 强制重排序
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("test_force_ooo".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("earlier".to_string())),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        rm.append_channel_record(req2, true).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // 验证文件有 2 条记录，按 time 排序，sn 重编号
        let expected_path = root
            .join("test_force_ooo")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);

        // 第一条 sn=1, time=10:00:00, content=earlier
        let r1: ChannelRecord = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(r1.sn(), 1);
        assert!(matches!(r1.content, Content::Text(v) if v.as_str() == "earlier"));

        // 第二条 sn=2, time=10:02:00, content=later
        let r2: ChannelRecord = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r2.sn(), 2);
        assert!(matches!(r2.content, Content::Text(v) if v.as_str() == "later"));
    }

    #[tokio::test]
    async fn test_append_force_with_existing_data() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let rm = RecordManager::new();

        // 先写入 3 条，time 分别是 10:01, 10:02, 10:03
        let req1 = vec![
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("second".to_string())),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("third".to_string())),
                time: Arc::new("2026-06-25 10:02:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("fourth".to_string())),
                time: Arc::new("2026-06-25 10:03:00".to_string()),
            },
        ];
        rm.append_channel_record(req1, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // force 写入 2 条更早的记录：09:59, 10:00
        let req2 = vec![
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("first".to_string())),
                time: Arc::new("2026-06-25 09:59:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("first-half".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
        ];
        rm.append_channel_record(req2, true).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // 验证文件有 5 条记录，按 time 排序，sn 重编号
        let expected_path = root
            .join("test_force_existing")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 5);

        let expected_order = ["first", "first-half", "second", "third", "fourth"];
        for (i, line) in lines.iter().enumerate() {
            let record: ChannelRecord = serde_json::from_str(line).unwrap();
            assert_eq!(record.sn(), (i + 1) as u64, "sn mismatch at line {}", i);
            assert!(matches!(&record.content, Content::Text(v) if v.as_str() == expected_order[i]), "content mismatch at line {}", i);
        }
    }

    #[tokio::test]
    async fn test_append_multiple_keys() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(RecordAppendWriter::new(Arc::new(ChannelParser), Arc::new(NoopFileHook)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        // 两个不同 key 的请求（不同 agent_id）
        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("test_mk_a".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self_a".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("agent1-msg".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_mk_b".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u2".to_string()),
                self_user_id: Arc::new("self_b".to_string()),
                group_id: Arc::new("g2".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("agent2-msg".to_string())),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
        ];

        let mut records_map: HashMap<RecordKey, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        // 验证两个文件都存在（不同 agent_id → 不同 RecordKey → 不同目录），sn 各自从 1 开始
        let path1 = root
            .join("test_mk_a")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
        let path2 = root
            .join("test_mk_b")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");

        assert!(path1.exists(), "file for agent1 should exist");
        assert!(path2.exists(), "file for agent2 should exist");

        let r1: ChannelRecord = serde_json::from_str(tokio::fs::read_to_string(&path1).await.unwrap().trim()).unwrap();
        let r2: ChannelRecord = serde_json::from_str(tokio::fs::read_to_string(&path2).await.unwrap().trim()).unwrap();
        assert_eq!(r1.sn(), 1);
        assert_eq!(r2.sn(), 1);
        assert!(matches!(r1.content, Content::Text(v) if v.as_str() == "agent1-msg"));
        assert!(matches!(r2.content, Content::Text(v) if v.as_str() == "agent2-msg"));
    }

    #[tokio::test]
    async fn test_append_merged_identities_same_file() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let rm = RecordManager::new();

        // 两个请求：同一 agent/role/date、不同 messenger/user/self_user/group → 合并到同一文件、sn 连续
        let reqs = vec![
            ChannelRequest {
                agent_id: Arc::new("test_merged_ids".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                self_user_id: Arc::new("self1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("msg1".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_merged_ids".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("web".to_string()),
                user_id: Arc::new("u2".to_string()),
                self_user_id: Arc::new("self2".to_string()),
                group_id: Arc::new("g2".to_string()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new(String::new()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("msg2".to_string())),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
        ];
        rm.append_channel_record(reqs, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // 仅存在一个文件，两条记录 sn 连续
        let expected_path = root
            .join("test_merged_ids")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);

        let r1: ChannelRecord = serde_json::from_str(lines[0]).unwrap();
        let r2: ChannelRecord = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r1.sn(), 1);
        assert_eq!(r2.sn(), 2);
        assert!(matches!(r1.content, Content::Text(v) if v.as_str() == "msg1"));
        assert!(matches!(r2.content, Content::Text(v) if v.as_str() == "msg2"));
        // 各记录保留自身身份字段
        assert_eq!(*r1.messenger_id, "telegram");
        assert_eq!(*r1.user_id, "u1");
        assert_eq!(*r1.self_user_id, "self1");
        assert_eq!(*r1.group_id, "g1");
        assert_eq!(*r2.messenger_id, "web");
        assert_eq!(*r2.user_id, "u2");
        assert_eq!(*r2.self_user_id, "self2");
        assert_eq!(*r2.group_id, "g2");
    }
}
