use async_trait::async_trait;
use dashmap::DashMap;
use kai_file::index::{FilePathGenerator, Record};
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use kissbot_memory::data::{ChannelParser, FileHook, RequestParser, ThinkParser, ToolCallParser, ToolResultParser};
use kissbot_memory::index::MemoryIndexer;
use kissbot_api::memory::*;
use crate::error::{Error, Result};
use kai_file::ReverseLineReader;
use kai_file::error::Result as FileResult;
use kai_file::{FileAppendWriter, FileAppendWriterContext, ErrorHandler, FileObjectAppender};

pub(crate) struct FileState {
    pub sn: u64,
    pub time: Arc<String>,
}

async fn write_records_to_file<R: MemoryRecord>(file: &mut tokio::fs::File, records: &mut Vec<R>, state: &mut FileState) -> Result<()>{
    for record in records {
        record.set_sn(state.sn + 1);
        let line = serde_json::to_string(&record)? + "\n";
        file.write_all(line.as_bytes()).await?;
        state.sn = record.sn();
        state.time = record.time_string();
    }
    Ok(())
}

pub(crate) async fn load_existing_file_state(file_path: &PathBuf) -> Result<FileState> {
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

pub(crate) struct RecordWriterContext<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
{
    state: Option<FileState>,
    parser: P,
    hook: H,
    _phantom: PhantomData<(K, R)>,
}

impl<K, R, P, H> RecordWriterContext<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: MemoryRecord,
    P: FilePathGenerator<K>,
    H: FileHook<K>,
{
    pub fn new(parser: P, hook: H) -> Self {
        Self {
            state: None,
            parser,
            hook,
            _phantom: PhantomData,
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
    async fn write(&mut self, key: &K, records: Vec<R>) -> FileResult<()> {
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

pub(crate) struct RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
{
    map: DashMap<K, Arc<Mutex<RecordWriterContext<K, R, P, H>>>>,
    parser: P,
    hook: H,
    _phantom: PhantomData<(R,)>,
}

impl<K, R, P, H> RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: MemoryRecord,
    P: FilePathGenerator<K>,
    H: FileHook<K>,
{
    pub fn new(parser: P, hook: H) -> Self {
        Self {
            map: DashMap::new(),
            parser,
            hook,
            _phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<K, R, P, H> FileAppendWriter<K, R, RecordWriterContext<K, R, P, H>> for RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    R: MemoryRecord + Send + Sync + 'static,
    P: FilePathGenerator<K> + Send + Sync + 'static + Clone,
    H: FileHook<K> + Send + Sync + 'static + Clone,
{
    async fn get_lock(&self, key: &K) -> Arc<Mutex<RecordWriterContext<K, R, P, H>>> {
        match self.map.entry(key.clone()) {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                let ctx = Arc::new(Mutex::new(
                    RecordWriterContext::new(self.parser.clone(), self.hook.clone())
                ));
                entry.insert(ctx.clone());
                ctx
            }
        }
    }

    async fn remove_lock(&self, key: &K) {
        if let Some(entry) = self.map.get(key) {
            if Arc::strong_count(entry.value()) == 1 {
                drop(entry);
                self.map.remove(key);
            }
        }
    }
}

#[derive(Clone)]
struct ChannelFileIndexHook;

impl FileHook<ChannelRecordKey> for ChannelFileIndexHook {
    fn on_append(&self, key: &ChannelRecordKey) {
        MemoryIndexer::get().mark_channel_obsolete(key);
    }

    fn on_force_append(&self, key: &ChannelRecordKey) {
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
impl<K: std::fmt::Debug + Send + Sync + 'static, R: Send + Sync + 'static>
    ErrorHandler<K, R> for LogErrorHandler
{
    async fn on_write_error(&self, key: &K, _batch: Vec<R>, error: &kai_file::Error) {
        eprintln!("[memory-store] write error for key={:?}: {}", key, error);
    }
}

#[cfg(test)]
pub(crate) struct NoopFileHook;

#[cfg(test)]
impl<K> FileHook<K> for NoopFileHook {
    fn on_append(&self, _key: &K) {}
    fn on_force_append(&self, _key: &K) {}
}

type ChannelWriter = RecordAppendWriter<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>;
type ChannelContext = RecordWriterContext<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>;
type ChannelAppender = FileObjectAppender<ChannelRecordKey, ChannelRecord, ChannelWriter, ChannelContext, LogErrorHandler>;

type ThinkWriter = RecordAppendWriter<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>;
type ThinkContext = RecordWriterContext<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>;
type ThinkAppender = FileObjectAppender<RecordKey, ThinkRecord, ThinkWriter, ThinkContext, LogErrorHandler>;

type ToolCallWriter = RecordAppendWriter<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>;
type ToolCallContext = RecordWriterContext<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>;
type ToolCallAppender = FileObjectAppender<RecordKey, ToolCallRecord, ToolCallWriter, ToolCallContext, LogErrorHandler>;

type ToolResultWriter = RecordAppendWriter<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>;
type ToolResultContext = RecordWriterContext<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>;
type ToolResultAppender = FileObjectAppender<RecordKey, ToolResultRecord, ToolResultWriter, ToolResultContext, LogErrorHandler>;

pub struct RecordManager {
    channel_writer: Arc<ChannelWriter>,
    channel_appender: ChannelAppender,

    think_writer: Arc<ThinkWriter>,
    think_appender: ThinkAppender,

    tool_call_writer: Arc<ToolCallWriter>,
    tool_call_appender: ToolCallAppender,

    tool_result_writer: Arc<ToolResultWriter>,
    tool_result_appender: ToolResultAppender,
}

static RECORD_MANAGER: OnceLock<RecordManager> = OnceLock::new();

impl RecordManager {
    pub fn new() -> Self {
        let channel_writer = Arc::new(RecordAppendWriter::new(ChannelParser {}, ChannelFileIndexHook {}));
        let channel_appender = FileObjectAppender::new(
            channel_writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        );

        let think_writer = Arc::new(RecordAppendWriter::new(ThinkParser {}, ThinkFileIndexHook {}));
        let think_appender = FileObjectAppender::new(
            think_writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        );

        let tool_call_writer = Arc::new(RecordAppendWriter::new(ToolCallParser {}, ToolCallFileIndexHook {}));
        let tool_call_appender = FileObjectAppender::new(
            tool_call_writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        );

        let tool_result_writer = Arc::new(RecordAppendWriter::new(ToolResultParser {}, ToolResultFileIndexHook {}));
        let tool_result_appender = FileObjectAppender::new(
            tool_result_writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        );

        Self {
            channel_writer,
            channel_appender,
            think_writer,
            think_appender,
            tool_call_writer,
            tool_call_appender,
            tool_result_writer,
            tool_result_appender,
        }
    }

    pub fn get() -> &'static Self {
        RECORD_MANAGER.get_or_init(|| RecordManager::new())
    }

    pub async fn append_channel_record(&self, requests: Vec<ChannelRequest>, force: bool) -> Result<()> {
        let mut records_map: HashMap<ChannelRecordKey, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }

        for (key, records) in &records_map {
            let mut sorted = records.clone();
            sorted.sort_by(|a, b| a.cmp(b));

            let lock = self.channel_writer.get_lock(key).await;
            let ctx = lock.lock().await;
            if let Some(ref state) = ctx.state {
                if state.time.as_str() > sorted[0].time() {
                    if !force {
                        return Err(Error::RecordNotInOrder(
                            state.time.to_string(),
                            sorted[0].time().to_string(),
                        ));
                    }
                }
            }
        }

        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            self.channel_appender.append(key, records).await;
        }

        Ok(())
    }

    pub async fn append_think_record(&self, requests: Vec<ThinkRequest>, force: bool) -> Result<()> {
        let mut records_map: HashMap<RecordKey, Vec<ThinkRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ThinkParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }

        for (key, records) in &records_map {
            let mut sorted = records.clone();
            sorted.sort_by(|a, b| a.cmp(b));

            let lock = self.think_writer.get_lock(key).await;
            let ctx = lock.lock().await;
            if let Some(ref state) = ctx.state {
                if state.time.as_str() > sorted[0].time() {
                    if !force {
                        return Err(Error::RecordNotInOrder(
                            state.time.to_string(),
                            sorted[0].time().to_string(),
                        ));
                    }
                }
            }
        }

        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            self.think_appender.append(key, records).await;
        }

        Ok(())
    }

    pub async fn append_tool_call_record(&self, requests: Vec<ToolCallRequest>, force: bool) -> Result<()> {
        let mut records_map: HashMap<RecordKey, Vec<ToolCallRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ToolCallParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }

        for (key, records) in &records_map {
            let mut sorted = records.clone();
            sorted.sort_by(|a, b| a.cmp(b));

            let lock = self.tool_call_writer.get_lock(key).await;
            let ctx = lock.lock().await;
            if let Some(ref state) = ctx.state {
                if state.time.as_str() > sorted[0].time() {
                    if !force {
                        return Err(Error::RecordNotInOrder(
                            state.time.to_string(),
                            sorted[0].time().to_string(),
                        ));
                    }
                }
            }
        }

        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            self.tool_call_appender.append(key, records).await;
        }

        Ok(())
    }

    pub async fn append_tool_result_record(&self, requests: Vec<ToolResultRequest>, force: bool) -> Result<()> {
        let mut records_map: HashMap<RecordKey, Vec<ToolResultRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ToolResultParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }

        for (key, records) in &records_map {
            let mut sorted = records.clone();
            sorted.sort_by(|a, b| a.cmp(b));

            let lock = self.tool_result_writer.get_lock(key).await;
            let ctx = lock.lock().await;
            if let Some(ref state) = ctx.state {
                if state.time.as_str() > sorted[0].time() {
                    if !force {
                        return Err(Error::RecordNotInOrder(
                            state.time.to_string(),
                            sorted[0].time().to_string(),
                        ));
                    }
                }
            }
        }

        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            self.tool_result_appender.append(key, records).await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    use std::sync::{Once, OnceLock};
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

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("test_append_new".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("hello".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
        ];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        // 验证文件被创建
        let expected_path = root
            .join("test_append_new")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
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

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("test_append_multi".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("msg1".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_append_multi".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("msg2".to_string())),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_append_multi".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("msg3".to_string())),
                time: Arc::new("2026-06-25 10:02:00".to_string()),
            },
        ];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        let expected_path = root
            .join("test_append_multi")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
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

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 第一次写入
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("test_append_seq".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Content::Text(Arc::new("first".to_string())),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        ctx.append_record(req1, false, NoopFileHook).await.unwrap();

        // 第二次写入
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("test_append_seq".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Content::Text(Arc::new("second".to_string())),
            time: Arc::new("2026-06-25 10:01:00".to_string()),
        }];
        ctx.append_record(req2, false, NoopFileHook).await.unwrap();

        let expected_path = root
            .join("test_append_seq")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
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

        let ctx: RecordContext<ThinkRequest, RecordKey, ThinkRecord, ThinkParser> =
            RecordContext::new(ThinkParser {});

        let requests = vec![ThinkRequest {
            agent_id: Arc::new("test_think".to_string()),
            role_name: Arc::new("".to_string()),
            content: Arc::new("I think...".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        let expected_path = root
            .join("test_think")
            .join("memory-store")
            .join("2026")
            .join("think-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ThinkRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
        assert_eq!(*record.content, "I think...");
    }

    #[tokio::test]
    async fn test_append_tool_call_record() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let ctx: RecordContext<ToolCallRequest, RecordKey, ToolCallRecord, ToolCallParser> =
            RecordContext::new(ToolCallParser {});

        let requests = vec![ToolCallRequest {
            agent_id: Arc::new("test_tool_call".to_string()),
            role_name: Arc::new("".to_string()),
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        let expected_path = root
            .join("test_tool_call")
            .join("memory-store")
            .join("2026")
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

        let ctx: RecordContext<ToolResultRequest, RecordKey, ToolResultRecord, ToolResultParser> =
            RecordContext::new(ToolResultParser {});

        let requests = vec![ToolResultRequest {
            agent_id: Arc::new("test_tool_result".to_string()),
            role_name: Arc::new("".to_string()),
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        let expected_path = root
            .join("test_tool_result")
            .join("memory-store")
            .join("2026")
            .join("tool-result-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ToolResultRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
    }

    #[tokio::test]
    async fn test_append_out_of_order_rejected() {
        init_test_config();

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 先写入一条 time=10:02:00
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("test_ooor".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Content::Text(Arc::new("later".to_string())),
            time: Arc::new("2026-06-25 10:02:00".to_string()),
        }];
        ctx.append_record(req1, false, NoopFileHook).await.unwrap();

        // 再写入一条 time=10:00:00 — 早于已有记录，应该被拒绝
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("test_ooor".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Content::Text(Arc::new("earlier".to_string())),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        let result = ctx.append_record(req2, false, NoopFileHook).await;
        assert!(result.is_err());
        match result {
            Err(Error::RecordNotInOrder(latest, new)) => {
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

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 先写入一条 time=10:02:00
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("test_force_ooo".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Content::Text(Arc::new("later".to_string())),
            time: Arc::new("2026-06-25 10:02:00".to_string()),
        }];
        ctx.append_record(req1, false, NoopFileHook).await.unwrap();

        // force=true 写入一条 time=10:00:00 — 强制重排序
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("test_force_ooo".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Content::Text(Arc::new("earlier".to_string())),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        ctx.append_record(req2, true, NoopFileHook).await.unwrap();

        // 验证文件有 2 条记录，按 time 排序，sn 重编号
        let expected_path = root
            .join("test_force_ooo")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
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

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 先写入 3 条，time 分别是 10:01, 10:02, 10:03
        let req1 = vec![
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("second".to_string())),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("third".to_string())),
                time: Arc::new("2026-06-25 10:02:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("fourth".to_string())),
                time: Arc::new("2026-06-25 10:03:00".to_string()),
            },
        ];
        ctx.append_record(req1, false, NoopFileHook).await.unwrap();

        // force 写入 2 条更早的记录：09:59, 10:00
        let req2 = vec![
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("first".to_string())),
                time: Arc::new("2026-06-25 09:59:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_force_existing".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("first-half".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
        ];
        ctx.append_record(req2, true, NoopFileHook).await.unwrap();

        // 验证文件有 5 条记录，按 time 排序，sn 重编号
        let expected_path = root
            .join("test_force_existing")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
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

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 两个不同 key 的请求（不同 agent_id）
        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("test_mk_a".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("agent1-msg".to_string())),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("test_mk_b".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u2".to_string()),
                group_id: Arc::new("g2".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Content::Text(Arc::new("agent2-msg".to_string())),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
        ];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        // 验证两个文件都存在，sn 各自从 1 开始
        let path1 = root
            .join("test_mk_a")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
        let path2 = root
            .join("test_mk_b")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u2=g2-records-2026-06-25.jsonl");

        assert!(path1.exists(), "file for agent1 should exist");
        assert!(path2.exists(), "file for agent2 should exist");

        let r1: ChannelRecord = serde_json::from_str(tokio::fs::read_to_string(&path1).await.unwrap().trim()).unwrap();
        let r2: ChannelRecord = serde_json::from_str(tokio::fs::read_to_string(&path2).await.unwrap().trim()).unwrap();
        assert_eq!(r1.sn(), 1);
        assert_eq!(r2.sn(), 1);
        assert!(matches!(r1.content, Content::Text(v) if v.as_str() == "agent1-msg"));
        assert!(matches!(r2.content, Content::Text(v) if v.as_str() == "agent2-msg"));
    }
}
