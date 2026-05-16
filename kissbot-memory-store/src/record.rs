use dashmap::DashMap;
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::data::{ChannelParser, ChannelRecord, ChannelRecordKey, FileHook, FilePathGenerator, Record, RecordKey, RequestParser, ThinkParser, ThinkRecord, ToolCallParser, ToolCallRecord, ToolResultParser, ToolResultRecord};
use crate::error::{Error, Result};
use crate::index::MemoryIndexer;
use kissbot_api::store::*;
use kai_file::ReverseLineReader;

struct FileState {
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

struct RecordContext<Q,K,R,P>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: Record,
    P: RequestParser<Q,K,R> + FilePathGenerator<K>,
{
    _marker: PhantomData<(Q,R)>,
    states: DashMap<K, FileLock>,
    parser: P,
}

impl<Q,K,R,P> RecordContext<Q,K,R,P>
where
    K: Eq + Hash + Clone + Send + Sync,
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

            let file_path = self.parser.ensure_file_path(&key).await?;

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
