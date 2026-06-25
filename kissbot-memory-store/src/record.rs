use dashmap::DashMap;
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use kissbot_memory::data::{ChannelParser, ChannelRecord, ChannelRecordKey, FileHook, FileKey, FilePathGenerator, Record, RecordKey, RequestParser, ThinkParser, ThinkRecord, ToolCallParser, ToolCallRecord, ToolResultParser, ToolResultRecord, ensure_file_path};
use kissbot_memory::index::MemoryIndexer;
use kissbot_api::store::*;
use crate::error::{Error, Result};
use kai_file::ReverseLineReader;

pub(crate) struct FileState {
    pub sn: u64,
    pub time: Arc<String>,
}

async fn write_records_to_file<R: Record>(file: &mut tokio::fs::File, records: &mut Vec<R>, state: &mut FileState) -> Result<()>{
    for record in records {
        record.set_sn(state.sn + 1);
        let line = serde_json::to_string(&record)? + "\n";
        file.write_all(line.as_bytes()).await?;
        state.sn = record.sn();
        state.time = record.time();
    }
    Ok(())
}

type FileLock = Arc<Mutex<Option<FileState>>>;

async fn get_lock<K>(files_map: &DashMap<K, FileLock>, key: &K) -> FileLock
where K: Eq + Hash + Clone + Send + Sync,
{
    files_map.entry(key.clone()).or_insert_with(|| Arc::new(Mutex::new(None))).clone()
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

struct RecordContext<Q,K,R,P>
where
    K: Eq + Hash + Clone + FileKey + Send + Sync,
    R: Record,
    P: RequestParser<Q,K,R> + FilePathGenerator<K>,
{
    _marker: PhantomData<(Q,R)>,
    states: DashMap<K, FileLock>,
    parser: P,
}

impl<Q,K,R,P> RecordContext<Q,K,R,P>
where
    K: Eq + Hash + Clone + FileKey + Send + Sync,
    R: Record,
    P: RequestParser<Q,K,R> + FilePathGenerator<K>,
{
    pub fn new(parser: P) -> Self {
        Self {
            _marker: PhantomData,
            states: DashMap::new(),
            parser,
        }
    }

    pub async fn append_record<H>(&self, requests: Vec<Q>, force: bool, hook: H) -> Result<()>
    where
        H: FileHook<K>,
    {
        //不同key对应不同文件，不同锁
        let mut records_map = HashMap::new();
        for request in requests {
            let (key, record) = self.parser.parse_request(request);
            records_map.entry(key).or_insert_with(|| Vec::new()).push(record);
        }
        for (key, mut records) in records_map.drain() {
            let lock = get_lock(&self.states, &key).await;
            let mut gaurd = lock.lock().await;

            let file_path = ensure_file_path(&key, &self.parser).await?;

            let mut state = if let Some(old_state) = gaurd.take() {
                old_state
            } else {
                load_existing_file_state(&file_path).await?
            };
  
            for i in 0..records.len() {
                records[i].set_sn(state.sn + 1 + i as u64);
            }
            records.sort_by(|a, b| a.cmp(b));

            if state.time.as_str() > records[0].time().as_str() {
                if force {
                    //强行插入，则要读入所有记录，全部排序
                    let mut max_sn = 0 as u64;
                    let mut all_records = Vec::new();
                    let file = tokio::fs::File::open(&file_path).await?;
                    let reader = tokio::io::BufReader::new(file);
                    let mut lines = reader.lines();
                    while let Some(line) = lines.next_line().await? {
                        let record = serde_json::from_str::<R>(line.as_str())?;
                        max_sn = max_sn.max(record.sn());
                        all_records.push(record);
                    }
                    for record in records {
                        all_records.push(record);
                    }
                    all_records.sort_by(|a, b| a.cmp(b));
                    state.sn = 0;
                    let mut result: Result<()> = Ok(());
                    match tokio::fs::OpenOptions::new().create(true).write(true).open(&file_path).await {
                        Ok(mut file) => {
                            match write_records_to_file(&mut file, &mut all_records, &mut state).await {
                                Ok(_) => {
                                    *gaurd = Some(state);
                                }
                                Err(e) => {
                                    result = Err(e);
                                }
                            }
                        },
                        Err(e) => {
                            result = Err(Error::Io(e));
                        }
                    }
                    hook.on_force_append(&key);
                    if let Err(err) = result {
                        return Err(err);
                    }
                }
                else {
                    return Err(Error::RecordNotInOrder(state.time.as_str().to_string(), records[0].time().to_string()));
                }
            }
            else {
                let mut result: Result<()> = Ok(());
                match tokio::fs::OpenOptions::new().create(true).append(true).open(&file_path).await {
                    Ok(mut file) => {
                        match write_records_to_file(&mut file, &mut records, &mut state).await {
                            Ok(_) => {
                                *gaurd = Some(state);
                            }
                            Err(e) => {
                                result = Err(e);
                            }
                        }
                    }
                    Err(e) => {
                        result = Err(Error::Io(e));
                    }
                }
                hook.on_append(&key);
                if let Err(err) = result {
                    return Err(err);
                }
            }
        }

        Ok(())
    }
}

struct ChannelFileIndexHook;

impl FileHook<ChannelRecordKey> for ChannelFileIndexHook {
    fn on_append(&self, key: &ChannelRecordKey) {
        MemoryIndexer::get().mark_channel_obsolete(key);
    }

    fn on_force_append(&self, key: &ChannelRecordKey) {
        MemoryIndexer::get().mark_channel_all_obsolete(key);
    }
}

struct ThinkFileIndexHook;

impl FileHook<RecordKey> for ThinkFileIndexHook {
    fn on_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_think_obsolete(key);
    }

    fn on_force_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_think_all_obsolete(key);
    }
}

struct ToolCallFileIndexHook;

impl FileHook<RecordKey> for ToolCallFileIndexHook {
    fn on_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_tool_call_obsolete(key);
    }

    fn on_force_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_tool_call_all_obsolete(key);
    }
}

struct ToolResultFileIndexHook;

impl FileHook<RecordKey> for ToolResultFileIndexHook {
    fn on_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_tool_result_obsolete(key);
    }

    fn on_force_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_tool_result_all_obsolete(key);
    }
}

pub struct RecordManager {
    channel_context: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser>,
    think_context: RecordContext<ThinkRequest, RecordKey, ThinkRecord, ThinkParser>,
    tool_call_context: RecordContext<ToolCallRequest, RecordKey, ToolCallRecord, ToolCallParser>,
    tool_result_context: RecordContext<ToolResultRequest, RecordKey, ToolResultRecord, ToolResultParser>,
}

static RECORD_MANAGER: OnceLock<RecordManager> = OnceLock::new();

impl RecordManager {
    pub fn new() -> Self {
        Self {
            channel_context: RecordContext::new(ChannelParser {}),
            think_context: RecordContext::new(ThinkParser {}),
            tool_call_context: RecordContext::new(ToolCallParser {}),
            tool_result_context: RecordContext::new(ToolResultParser {}),
        }
    }

    pub fn get() -> &'static Self {
        RECORD_MANAGER.get_or_init(|| RecordManager::new())
    }

    pub async fn append_channel_record(&self, requests: Vec<ChannelRequest>, force: bool) -> Result<()> {
        self.channel_context.append_record(requests, force, ChannelFileIndexHook{}).await
    }

    pub async fn append_think_record(&self, requests: Vec<ThinkRequest>, force: bool) -> Result<()> {
        self.think_context.append_record(requests, force, ThinkFileIndexHook{}).await
    }

    pub async fn append_tool_call_record(&self, requests: Vec<ToolCallRequest>, force: bool) -> Result<()> {
        self.tool_call_context.append_record(requests, force, ToolCallFileIndexHook{}).await
    }

    pub async fn append_tool_result_record(&self, requests: Vec<ToolResultRequest>, force: bool) -> Result<()> {
        self.tool_result_context.append_record(requests, force, ToolResultFileIndexHook{}).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空操作 FileHook，不调用 MemoryIndexer
    struct NoopFileHook;

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

    use std::sync::Once;
    use kissbot_memory::Config as MemoryConfig;

    static INIT: Once = Once::new();

    fn init_test_config(root_dir: &std::path::Path) {
        INIT.call_once(|| {
            unsafe { std::env::set_var(
                "KISSBOT_MEMORY_CONFIG",
                root_dir.join("memory-config.json").to_str().unwrap()
            ); }
            let json_content = format!(r#"{{"root_dir": "{}"}}"#, root_dir.display().to_string());
            std::fs::write(root_dir.join("memory-config.json"), &json_content).unwrap();
            // 提前初始化 MemoryConfig 的 OnceLock
            let _ = MemoryConfig::get();
        });
    }

    #[tokio::test]
    async fn test_append_new_file() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());
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
                content: Arc::new("hello".to_string()),
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
        assert_eq!(*record.content, "hello");
    }

    #[tokio::test]
    async fn test_append_multiple_records() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());
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
                content: Arc::new("msg1".to_string()),
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
                content: Arc::new("msg2".to_string()),
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
                content: Arc::new("msg3".to_string()),
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
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());
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
            content: Arc::new("first".to_string()),
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
            content: Arc::new("second".to_string()),
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
        assert_eq!(*r1.content, "first");
        assert_eq!(r2.sn(), 2);
        assert_eq!(*r2.content, "second");
    }

    #[tokio::test]
    async fn test_append_think_record() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());
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
}
