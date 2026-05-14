use dashmap::DashMap;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::data::{ChannelRecord, Record, ThinkRecord, ToolCallRecord, ToolResultRecord};
use crate::error::{Error, Result};
use kissbot_api::store::*;
use kai_file::ReverseLineReader;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
struct ChannelRecordKey {
    pub agent_id: Arc<String>,
    pub role_id: Arc<String>,
    pub channel_id: Arc<String>,
    pub date: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
struct RecordKey {
    pub agent_id: Arc<String>,
    pub role_id: Arc<String>,
    pub date: Arc<String>,
}

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

async fn ensure_year_role_dir(agent_id: &str, role_name: &str, date: &str) -> Result<PathBuf> {
    let year = &date[0..4];
    let store_dir = DirectoryManager::get().ensure_agent_store_dir(agent_id).await?;
    let year_role_dir = if role_name.is_empty() {
        store_dir.join(year)
    } else {
        store_dir.join(format!("{}-{}", year, role_name))
    };
    
    if !year_role_dir.exists() {
        tokio::fs::create_dir_all(&year_role_dir).await?;
    }
    
    Ok(year_role_dir)
}

fn parse_date_from_time(time: &str) -> String {
    time[0..10].to_string()
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

    let mut reader = ReverseLineReader::new(file_path).await?;

    while let Some(line) = reader.next_line().await? {
        if !line.is_empty() {
            if let Ok(record) = serde_json::from_str::<serde_json::Value>(line.as_str()) {
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

trait RequestParser<Q,K,R>
where
    R: Record,
{
    fn parse(&self, request: Q) -> (K, R);
    async fn ensure_file_path(&self, key: &K) -> Result<PathBuf>;
}

struct ChannelRequestParser;

impl RequestParser<ChannelRequest, ChannelRecordKey, ChannelRecord> for ChannelRequestParser {
    fn parse(&self, request: ChannelRequest) -> (ChannelRecordKey, ChannelRecord) {
        let key = ChannelRecordKey {
            agent_id: Arc::new(request.agent_id),
            role_id: Arc::new(request.role_name),
            channel_id: Arc::new(request.channel_id),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ChannelRecord {
            user_id: Arc::new(request.user_id),
            time: Arc::new(request.time),
            msg_type: Arc::new(request.msg_type),
            content: Arc::new(request.content),
            sn: 0,
        };
        (key, record)
    }

    async fn ensure_file_path(&self, key: &ChannelRecordKey) -> Result<PathBuf> {
        let year_role_dir = ensure_year_role_dir(&key.agent_id, &key.role_id, &key.date).await?;
        let file_path = year_role_dir.join(format!("channel-{}-records-{}.jsonl", &key.channel_id, &key.date));
        Ok(file_path)
    }
}

struct ThinkRequestParser;

impl RequestParser<ThinkRequest, RecordKey, ThinkRecord> for ThinkRequestParser {
    fn parse(&self, request: ThinkRequest) -> (RecordKey, ThinkRecord) {
        let key = RecordKey {
            agent_id: Arc::new(request.agent_id),
            role_id: Arc::new(request.role_name),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ThinkRecord {
            content: Arc::new(request.content),
            key: Arc::new(request.key),
            time: Arc::new(request.time),
            sn: 0,
        };
        (key, record)
    }

    async fn ensure_file_path(&self, key: &RecordKey) -> Result<PathBuf> {
        let year_role_dir = ensure_year_role_dir(&key.agent_id, &key.role_id, &key.date).await?;
        let file_path = year_role_dir.join(format!("think-records-{}.jsonl", key.date));
        Ok(file_path)
    }
}

struct ToolCallRequestParser;

impl RequestParser<ToolCallRequest, RecordKey, ToolCallRecord> for ToolCallRequestParser {
    fn parse(&self, request: ToolCallRequest) -> (RecordKey, ToolCallRecord) {
        let key = RecordKey {
            agent_id: Arc::new(request.agent_id),
            role_id: Arc::new(request.role_name),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ToolCallRecord {
            tool_name: Arc::new(request.tool_name),
            tool_params: Arc::new(request.tool_params),
            key: Arc::new(request.key),
            time: Arc::new(request.time),
            sn: 0,
        };
        (key, record)
    }

    async fn ensure_file_path(&self, key: &RecordKey) -> Result<PathBuf> {
        let year_role_dir = ensure_year_role_dir(&key.agent_id, &key.role_id, &key.date).await?;
        let file_path = year_role_dir.join(format!("tool-call-records-{}.jsonl", key.date));
        Ok(file_path)
    }
}

struct ToolResultRequestParser;

impl RequestParser<ToolResultRequest, RecordKey, ToolResultRecord> for ToolResultRequestParser {
    fn parse(&self, request: ToolResultRequest) -> (RecordKey, ToolResultRecord) {
        let key = RecordKey {
            agent_id: Arc::new(request.agent_id),
            role_id: Arc::new(request.role_name),
            date: Arc::new(parse_date_from_time(&request.time)),
        };
        let record = ToolResultRecord {
            tool_result: Arc::new(request.tool_result),
            key: Arc::new(request.key),
            time: Arc::new(request.time),
            sn: 0,
        };
        (key, record)
    }

    async fn ensure_file_path(&self, key: &RecordKey) -> Result<PathBuf> {
        let year_role_dir = ensure_year_role_dir(&key.agent_id, &key.role_id, &key.date).await?;
        let file_path = year_role_dir.join(format!("tool-result-records-{}.jsonl", key.date));
        Ok(file_path)
    }
}

struct RecordContext<Q,K,R,P>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: Record,
    P: RequestParser<Q,K,R>,
{
    _marker: PhantomData<(Q,R)>,
    states: DashMap<K, FileLock>,
    parser: P,
}

impl<Q,K,R,P> RecordContext<Q,K,R,P>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: Record,
    P: RequestParser<Q,K,R>,
{
    pub fn new(parser: P) -> Self {
        Self {
            _marker: PhantomData,
            states: DashMap::new(),
            parser,
        }
    }

    pub async fn append_record(&self, requests: Vec<Q>, force: bool) -> Result<()> {
        //不同key对应不同文件，不同锁
        let mut records_map = HashMap::new();
        for request in requests {
            let (key, record) = self.parser.parse(request);
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
                    let mut file = tokio::fs::OpenOptions::new().create(true).write(true).open(&file_path).await?;
                    write_records_to_file(&mut file, &mut all_records, &mut state).await?;
                    *gaurd = Some(state);
                }
                else {
                    return Err(Error::RecordNotInOrder(state.time.as_str().to_string(), records[0].time().to_string()));
                }
            }
            else {
                let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(&file_path).await?;
                write_records_to_file(&mut file, &mut records, &mut state).await?;
                *gaurd = Some(state);
            }
        }

        Ok(())
    }
}

pub struct RecordManager {
    channel_context: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRequestParser>,
    think_context: RecordContext<ThinkRequest, RecordKey,ThinkRecord, ThinkRequestParser>,
    tool_call_context: RecordContext<ToolCallRequest, RecordKey, ToolCallRecord, ToolCallRequestParser>,
    tool_result_context: RecordContext<ToolResultRequest, RecordKey, ToolResultRecord, ToolResultRequestParser>,
}

static RECORD_MANAGER: OnceLock<RecordManager> = OnceLock::new();

impl RecordManager {
    pub fn new() -> Self {
        Self {
            channel_context: RecordContext::new(ChannelRequestParser {}),
            think_context: RecordContext::new(ThinkRequestParser {}),
            tool_call_context: RecordContext::new(ToolCallRequestParser {}),
            tool_result_context: RecordContext::new(ToolResultRequestParser {}),
        }
    }

    pub fn get() -> &'static Self {
        RECORD_MANAGER.get_or_init(|| RecordManager::new())
    }

    pub async fn append_channel_record(&self, requests: Vec<ChannelRequest>, force: bool) -> Result<()> {
        self.channel_context.append_record(requests, force).await
    }

    pub async fn append_think_record(&self, requests: Vec<ThinkRequest>, force: bool) -> Result<()> {
        self.think_context.append_record(requests, force).await
    }

    pub async fn append_tool_call_record(&self, requests: Vec<ToolCallRequest>, force: bool) -> Result<()> {
        self.tool_call_context.append_record(requests, force).await
    }

    pub async fn append_tool_result_record(&self, requests: Vec<ToolResultRequest>, force: bool) -> Result<()> {
        self.tool_result_context.append_record(requests, force).await
    }
}
