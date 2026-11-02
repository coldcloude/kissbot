use async_trait::async_trait;
use dashmap::DashMap;
use futures_util::future;
use kai_file::index::FilePathGenerator;
use tokio::fs::{File, OpenOptions};
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use arcstr::ArcStr;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::*;

use kissbot_memory::data::ChannelParser;
use kissbot_memory::index::MemoryIndexer;
use kissbot_api::memory::*;
use crate::error::{Error, Result};
use crate::notify::publish_channel;
use kai_file::ReverseLineReader;
use kai_file::{FileAppendWriter, FileAppendWriterContext, ErrorHandler, FileObjectAppender};

struct FileState {
    pub sn: u64,
    pub time: ArcStr,
}

async fn load_existing_file_state(file_path: &PathBuf) -> Result<FileState> {
    let mut result = FileState {
        sn: 0,
        time: WATERMARK_ZERO_TIME.clone(),
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
                        result.time = ArcStr::from(time);
                    }
                }
            }
            break;
        }
    }

    Ok(result)
}

/// 通道记录按 (time, sn) 排序
fn record_order(a: &ChannelRecord, b: &ChannelRecord) -> std::cmp::Ordering {
    (a.time.as_str(), a.sn).cmp(&(b.time.as_str(), b.sn))
}

/// 单个通道记录文件的写入上下文：文件状态 + 路径解析
struct ChannelWriterContext {
    state: Option<FileState>,
    parser: Arc<ChannelParser>,
}

#[async_trait]
impl FileAppendWriterContext<Arc<RecordKey>, ChannelRecord> for ChannelWriterContext {
    async fn write(&mut self, key: &Arc<RecordKey>, records: Vec<ChannelRecord>) -> std::result::Result<(), kai_file::Error> {
        let file_path = self.parser.get_path(key).await?;

        let file_state = self.state.get_or_insert(
            load_existing_file_state(&file_path).await
                .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?
        );

        // 分配 SN
        let mut records = records;
        for (i, record) in records.iter_mut().enumerate() {
            record.sn = file_state.sn + 1 + i as u64;
        }

        // 按 time 排序（相同 time 按 sn）
        records.sort_by(record_order);

        if file_state.time.as_str() > records[0].time.as_str() {
            // 乱序 → 全量重写
            let mut all_records: Vec<ChannelRecord> = Vec::new();
            let file = File::open(&file_path).await?;
            let reader = BufReader::new(file);
            let mut lines = reader.lines();
            while let Some(line) = lines.next_line().await? {
                all_records.push(serde_json::from_str(&line)?);
            }
            all_records.extend(records);
            all_records.sort_by(record_order);

            let mut file = OpenOptions::new()
                .create(true).write(true).open(&file_path).await?;

            file_state.sn = 0;
            for record in &mut all_records {
                file_state.sn += 1;
                record.sn = file_state.sn;
                let line = serde_json::to_string(record)? + "\n";
                file.write_all(line.as_bytes()).await?;
            }
            if let Some(last) = all_records.last() {
                file_state.time = last.time.clone();
            }
            // tokio::fs::File 的 write_all 可能只把数据交给后台阻塞任务就返回：flush 等到这次写入
            // 真正落盘，否则紧接着的索引标记与通知会让读方看到不完整的数据
            file.flush().await?;

            MemoryIndexer::get().mark_channel_all_obsolete(key.clone());
            publish_channel(key, &all_records);
        } else {
            // 有序 → 追加
            let mut file = OpenOptions::new()
                .create(true).append(true).open(&file_path).await?;

            for record in &mut records {
                file_state.sn += 1;
                record.sn = file_state.sn;
                let line = serde_json::to_string(record)? + "\n";
                file.write_all(line.as_bytes()).await?;
                file_state.time = record.time.clone();
            }
            // 同上：flush 保证索引标记与通知触发时数据已可被读方看到
            file.flush().await?;

            MemoryIndexer::get().mark_channel_obsolete(key.clone());
            publish_channel(key, &records);
        }

        Ok(())
    }
}

/// 通道记录写入器：每个记录文件一把写锁
struct ChannelRecordWriter {
    map: DashMap<Arc<RecordKey>, Arc<Mutex<ChannelWriterContext>>>,
    parser: Arc<ChannelParser>,
}

impl ChannelRecordWriter {
    fn new(parser: Arc<ChannelParser>) -> Self {
        Self { map: DashMap::new(), parser }
    }
}

#[async_trait]
impl FileAppendWriter<Arc<RecordKey>, ChannelRecord, ChannelWriterContext> for ChannelRecordWriter {
    async fn get_lock(&self, key: &Arc<RecordKey>) -> Arc<Mutex<ChannelWriterContext>> {
        if let Some(ctx) = self.map.get(key) {
            return ctx.value().clone();
        }
        self.map.entry(key.clone()).or_insert_with(|| {
            let ctx = ChannelWriterContext { state: None, parser: self.parser.clone() };
            Arc::new(Mutex::new(ctx))
        }).value().clone()
    }

    async fn remove_lock(&self, _key: &Arc<RecordKey>) {
        // no op
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

type ChannelAppender = FileObjectAppender<Arc<RecordKey>, ChannelRecord, ChannelRecordWriter, ChannelWriterContext, LogErrorHandler>;

pub struct RecordManager {
    parser: Arc<ChannelParser>,
    appender: Arc<ChannelAppender>,
}

static RECORD_MANAGER: OnceLock<RecordManager> = OnceLock::new();

impl RecordManager {
    pub fn new() -> Self {
        let parser = Arc::new(ChannelParser);
        let writer = Arc::new(ChannelRecordWriter::new(parser.clone()));
        let appender = Arc::new(FileObjectAppender::new(
            writer,
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        ));
        Self { parser, appender }
    }

    pub fn get() -> &'static Self {
        RECORD_MANAGER.get_or_init(|| RecordManager::new())
    }

    pub async fn append_channel_record(&self, requests: Vec<ChannelRequest>, force: bool) -> Result<()> {
        let mut records_map: HashMap<Arc<RecordKey>, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = self.parser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        if records_map.is_empty() {
            return Ok(());
        }

        // 乱序检查：在写锁内读取文件状态，避免与并发写入竞争
        for (key, records) in &records_map {
            let lock = self.appender.get_lock(key).await;
            let mut guard = lock.lock().await;

            let file_path = self.parser.get_path(key).await?;
            let state = match guard.state.take() {
                Some(old_state) => old_state,
                None => load_existing_file_state(&file_path).await?,
            };
            let last_time = state.time.clone();
            guard.state = Some(state);

            let min_time = records.iter().map(|record| record.time.as_str()).min().unwrap_or("");
            if last_time.as_str() > min_time && !force {
                return Err(Error::RecordNotInOrder(
                    ArcStr::from(format!("{:?}", key)),
                    last_time,
                    ArcStr::from(min_time),
                ));
            }
        }

        let futs: Vec<_> = records_map
            .into_iter()
            .map(|(key, records)| self.appender.append(key, records))
            .collect();
        // 等待写入后再返回
        let promises = future::join_all(futs).await;
        for result in future::join_all(promises).await {
            if let Ok(Err(e)) = result {
                return Err(Error::from(e));
            }
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
        assert_eq!(state.time, "2000-01-01 00:00:00");
    }

    #[tokio::test]
    async fn test_load_state_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.jsonl");
        tokio::fs::write(&file_path, "").await.unwrap();

        let state = load_existing_file_state(&file_path).await.unwrap();
        assert_eq!(state.sn, 0);
        assert_eq!(state.time, "2000-01-01 00:00:00");
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
        assert_eq!(state.time, "2026-06-25 10:02:00");
    }

    use std::collections::HashMap;
    use std::sync::{Once, OnceLock};
    use std::time::Duration;
    use kissbot_api::Content;
    use kissbot_api::RoleEvent;
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

        let writer = Arc::new(ChannelRecordWriter::new(Arc::new(ChannelParser)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        let requests = vec![
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_append_new"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("hello")),
                time: ArcStr::from("2026-06-25 10:00:00"),
            },
        ];

        let mut records_map: HashMap<Arc<RecordKey>, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(record_order);
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
        assert_eq!(record.sn, 1);
        assert!(matches!(record.content, Content::Text(v) if v.as_str() == "hello"));
    }

    #[tokio::test]
    async fn test_append_multiple_records() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(ChannelRecordWriter::new(Arc::new(ChannelParser)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        let requests = vec![
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_append_multi"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("msg1")),
                time: ArcStr::from("2026-06-25 10:00:00"),
            },
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_append_multi"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("msg2")),
                time: ArcStr::from("2026-06-25 10:01:00"),
            },
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_append_multi"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("msg3")),
                time: ArcStr::from("2026-06-25 10:02:00"),
            },
        ];

        let mut records_map: HashMap<Arc<RecordKey>, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(record_order);
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
            assert_eq!(record.sn, (i + 1) as u64);
        }
    }

    #[tokio::test]
    async fn test_append_sequential() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(ChannelRecordWriter::new(Arc::new(ChannelParser)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        // 第一次写入
        let req1 = vec![ChannelRequest {
            meta: None,
            agent_id: ArcStr::from("test_append_seq"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            messenger_id: ArcStr::from("telegram"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::new(),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("first")),
            time: ArcStr::from("2026-06-25 10:00:00"),
        }];
        let mut records_map: HashMap<Arc<RecordKey>, Vec<ChannelRecord>> = HashMap::new();
        for request in req1 {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(record_order);
            appender.append(key, records).await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        // 第二次写入
        let req2 = vec![ChannelRequest {
            meta: None,
            agent_id: ArcStr::from("test_append_seq"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            messenger_id: ArcStr::from("telegram"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::new(),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("second")),
            time: ArcStr::from("2026-06-25 10:01:00"),
        }];
        let mut records_map: HashMap<Arc<RecordKey>, Vec<ChannelRecord>> = HashMap::new();
        for request in req2 {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(record_order);
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
        assert_eq!(r1.sn, 1);
        assert!(matches!(r1.content, Content::Text(v) if v.as_str() == "first"));
        assert_eq!(r2.sn, 2);
        assert!(matches!(r2.content, Content::Text(v) if v.as_str() == "second"));
    }

    #[tokio::test]
    async fn test_append_out_of_order_rejected() {
        init_test_config();

        let rm = RecordManager::new();

        // 先写入一条 time=10:02:00
        let req1 = vec![ChannelRequest {
            meta: None,
            agent_id: ArcStr::from("test_ooor"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            messenger_id: ArcStr::from("telegram"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::new(),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("later")),
            time: ArcStr::from("2026-06-25 10:02:00"),
        }];
        rm.append_channel_record(req1, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // 再写入一条 time=10:00:00 — 早于已有记录，应该被拒绝
        let req2 = vec![ChannelRequest {
            meta: None,
            agent_id: ArcStr::from("test_ooor"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            messenger_id: ArcStr::from("telegram"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::new(),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("earlier")),
            time: ArcStr::from("2026-06-25 10:00:00"),
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
            meta: None,
            agent_id: ArcStr::from("test_force_ooo"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            messenger_id: ArcStr::from("telegram"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::new(),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("later")),
            time: ArcStr::from("2026-06-25 10:02:00"),
        }];
        rm.append_channel_record(req1, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // force=true 写入一条 time=10:00:00 — 强制重排序
        let req2 = vec![ChannelRequest {
            meta: None,
            agent_id: ArcStr::from("test_force_ooo"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            messenger_id: ArcStr::from("telegram"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::new(),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("earlier")),
            time: ArcStr::from("2026-06-25 10:00:00"),
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
        assert_eq!(r1.sn, 1);
        assert!(matches!(r1.content, Content::Text(v) if v.as_str() == "earlier"));

        // 第二条 sn=2, time=10:02:00, content=later
        let r2: ChannelRecord = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r2.sn, 2);
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
                meta: None,
                agent_id: ArcStr::from("test_force_existing"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("second")),
                time: ArcStr::from("2026-06-25 10:01:00"),
            },
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_force_existing"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("third")),
                time: ArcStr::from("2026-06-25 10:02:00"),
            },
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_force_existing"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("fourth")),
                time: ArcStr::from("2026-06-25 10:03:00"),
            },
        ];
        rm.append_channel_record(req1, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // force 写入 2 条更早的记录：09:59, 10:00
        let req2 = vec![
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_force_existing"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("first")),
                time: ArcStr::from("2026-06-25 09:59:00"),
            },
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_force_existing"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("first-half")),
                time: ArcStr::from("2026-06-25 10:00:00"),
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
            assert_eq!(record.sn, (i + 1) as u64, "sn mismatch at line {}", i);
            assert!(matches!(&record.content, Content::Text(v) if v.as_str() == expected_order[i]), "content mismatch at line {}", i);
        }
    }

    #[tokio::test]
    async fn test_append_multiple_keys() {
        init_test_config();
        let root = &MemoryConfig::get().root_dir;

        let writer = Arc::new(ChannelRecordWriter::new(Arc::new(ChannelParser)));
        let appender = FileObjectAppender::new(
            writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(50),
            100,
        );

        // 两个不同 key 的请求（不同 agent_id）
        let requests = vec![
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_mk_a"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self_a"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("agent1-msg")),
                time: ArcStr::from("2026-06-25 10:00:00"),
            },
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_mk_b"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u2"),
                self_user_id: ArcStr::from("self_b"),
                group_id: ArcStr::from("g2"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("agent2-msg")),
                time: ArcStr::from("2026-06-25 10:01:00"),
            },
        ];

        let mut records_map: HashMap<Arc<RecordKey>, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }
        for (key, mut records) in records_map {
            records.sort_by(record_order);
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
        assert_eq!(r1.sn, 1);
        assert_eq!(r2.sn, 1);
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
                meta: None,
                agent_id: ArcStr::from("test_merged_ids"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("telegram"),
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self1"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("msg1")),
                time: ArcStr::from("2026-06-25 10:00:00"),
            },
            ChannelRequest {
                meta: None,
                agent_id: ArcStr::from("test_merged_ids"),
                role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
                messenger_id: ArcStr::from("web"),
                user_id: ArcStr::from("u2"),
                self_user_id: ArcStr::from("self2"),
                group_id: ArcStr::from("g2"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::new(),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("msg2")),
                time: ArcStr::from("2026-06-25 10:01:00"),
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
        assert_eq!(r1.sn, 1);
        assert_eq!(r2.sn, 2);
        assert!(matches!(r1.content, Content::Text(v) if v.as_str() == "msg1"));
        assert!(matches!(r2.content, Content::Text(v) if v.as_str() == "msg2"));
        // 各记录保留自身身份字段
        assert_eq!(r1.messenger_id, "telegram");
        assert_eq!(r1.user_id, "u1");
        assert_eq!(r1.self_user_id, "self1");
        assert_eq!(r1.group_id, "g1");
        assert_eq!(r2.messenger_id, "web");
        assert_eq!(r2.user_id, "u2");
        assert_eq!(r2.self_user_id, "self2");
        assert_eq!(r2.group_id, "g2");
    }
}
