use std::collections::{BTreeSet, HashMap};
use std::ops::Bound;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use arcstr::ArcStr;
use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use flume::Sender;
use kai_date::{self, as_date, get_date_time_segments};
use kai_file::FileIndexContext;
use kai_file::appender::{FileAppendWriter, FileAppendWriterContext};
use kai_file::index::{FilePathGenerator, QueryParser};
use kissbot_api::channel::IncomingMessage;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::error::Result;

// ========== DTOs ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineMessage {
    pub line: u32,
    pub message: Arc<IncomingMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupedMessages {
    pub key: Arc<MsgKey>,
    pub messages: Vec<LineMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MsgKey {
    pub group_id: ArcStr,
    pub date: ArcStr,
}

// ========== Query types ==========

#[derive(Debug, Clone)]
pub struct TimeRangeQuery {
    pub group_id: ArcStr,
    pub start: ArcStr,
    pub end: ArcStr,
}

// 索引 key 为 Arc<MsgKey>：普通 MsgKey 只在边界处包一次 Arc，容器内一律复用同一把 Arc
type GroupIndex = FileIndexContext<TimeRangeQuery, Arc<MsgKey>, IncomingMessage, MessageParser>;

// ========== FilePathGenerator / QueryParser ==========

struct MessageParser {
    base_dir: PathBuf,
}

impl MessageParser {
    fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
}

#[async_trait::async_trait]
impl FilePathGenerator<Arc<MsgKey>> for MessageParser {
    async fn get_path(&self, key: &Arc<MsgKey>) -> std::result::Result<PathBuf, kai_file::Error> {
        Ok(self.base_dir
            .join(key.group_id.as_str())
            .join(format!("{}.jsonl", key.date)))
    }
}

impl QueryParser<TimeRangeQuery, Arc<MsgKey>> for MessageParser {
    fn parse_query(&self, query: TimeRangeQuery) -> Vec<(Arc<MsgKey>, (String, String))> {
        let mut keys = Vec::new();
        if let Ok(mut ranges) = get_date_time_segments(&query.start, &query.end) {
            for range in ranges.drain(..) {
                let date = ArcStr::from(as_date(range.0.as_str()));
                keys.push((Arc::new(MsgKey {
                    group_id: query.group_id.clone(),
                    date,
                }), range));
            }
        }
        keys
    }
}

// ========== FileAppendWriter ==========

pub struct MessageFileWriterContext {
    path: PathBuf,
    index: Weak<GroupIndex>,
    date_sets: Weak<DashMap<ArcStr, BTreeSet<ArcStr>>>,
    stored_sender: Sender<Vec<IncomingMessage>>,
}

#[async_trait]
impl FileAppendWriterContext<Arc<MsgKey>, IncomingMessage> for MessageFileWriterContext {
    async fn write(&mut self, key: &Arc<MsgKey>, mut records: Vec<IncomingMessage>) -> std::result::Result<(), kai_file::Error> {
        let mut time = ArcStr::from(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
        if key.date.as_str() != as_date(time.as_str()) {
            time = ArcStr::from(format!("{} 23:59:59", key.date.as_str()));
        }
        for record in records.iter_mut() {
            record.time = time.clone();
        }
        
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;

        for record in records.iter() {
            // JSON 与换行符一次写入，避免索引读取时捕获到缺少结尾换行的半行
            let mut line = serde_json::to_string(record)?;
            line.push('\n');
            file.write_all(line.as_bytes()).await?;
        }

        if let Some(sets) = self.date_sets.upgrade() {
            sets.entry(key.group_id.clone())
                .or_insert_with(BTreeSet::new)
                .insert(key.date.clone());
        }

        if let Some(index) = self.index.upgrade() {
            // 索引 key 为 Arc<MsgKey>：在 key 边界从普通 MsgKey 包一次 Arc
            index.mark_obsolete(key);
        }

        self.stored_sender.send_async(records).await
        .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?;

        Ok(())
    }
}

// ========== MessageStore ==========

pub struct MessageStore {
    base_dir: PathBuf,
    index: Arc<GroupIndex>,
    date_sets: Arc<DashMap<ArcStr, BTreeSet<ArcStr>>>,
    stored_sender: Sender<Vec<IncomingMessage>>,
    map: Arc<Mutex<HashMap<Arc<MsgKey>, Weak<Mutex<MessageFileWriterContext>>>>>,
}

impl MessageStore {
    fn create_context(&self, key: &MsgKey) -> MessageFileWriterContext {
        let path = self.base_dir
            .join(key.group_id.as_str())
            .join(format!("{}.jsonl", key.date));
        MessageFileWriterContext {
            path,
            index: Arc::downgrade(&self.index),
            date_sets: Arc::downgrade(&self.date_sets),
            stored_sender: self.stored_sender.clone()
        }
    }
}

#[async_trait]
impl FileAppendWriter<Arc<MsgKey>, IncomingMessage, MessageFileWriterContext> for MessageStore {
    async fn get_lock(&self, key: &Arc<MsgKey>) -> Arc<Mutex<MessageFileWriterContext>> {
        let mut guard = self.map.lock().await;
        // 只读查询传 &MsgKey（Arc<MsgKey>: Borrow<MsgKey>），存活则复用既有写入锁
        if let Some(lock) = guard.get(key.as_ref()).and_then(|entry| entry.upgrade()) {
            return lock;
        }
        // 插入边界：调用方已持有 Arc<MsgKey>，clone 只增加引用计数
        let ctx = self.create_context(key.as_ref());
        let lock = Arc::new(Mutex::new(ctx));
        guard.insert(key.clone(), Arc::downgrade(&lock));
        lock
    }

    async fn remove_lock(&self, key: &Arc<MsgKey>) {
        let mut guard = self.map.lock().await;
        if let Some(entry) = guard.get(key.as_ref()) {
            if entry.strong_count() == 0 {
                guard.remove(key.as_ref());
            }
        }
    }
}

impl MessageStore {
    pub fn new(base_dir: &str, stored_sender: Sender<Vec<IncomingMessage>>) -> Self {
        let base_dir = PathBuf::from(base_dir);
        let parser = MessageParser::new(base_dir.clone());
        let index: Arc<GroupIndex> = Arc::new(FileIndexContext::new(parser));
        let date_sets: Arc<DashMap<ArcStr, BTreeSet<ArcStr>>> = Arc::new(DashMap::new());
        Self {
            base_dir,
            index,
            date_sets,
            stored_sender,
            map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // ========== Query methods ==========

    pub async fn get_recent(&self, group_id: &str, n: u32) -> Result<Vec<GroupedMessages>> {
        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        if let Some(dates) = self.date_sets.get(group_id) {
            for date in dates.iter().rev() {
                if remaining == 0 { break; }
                // key 边界：同一把 Arc<MsgKey> 既用于查询，也作为结果的分组 key
                let key = Arc::new(MsgKey {
                    group_id: ArcStr::from(group_id),
                    date: date.clone(),
                });
                let msgs = self.index.query_last(&key, remaining).await?;
                if !msgs.is_empty() {
                    let count = msgs.len() as u32;
                    let messages: Vec<LineMessage> = msgs.into_iter()
                        .map(|(line, msg)| LineMessage { line, message: msg })
                        .collect();
                    results.push(GroupedMessages { key, messages });
                    remaining = remaining.saturating_sub(count);
                }
            }
        }

        results.reverse();
        Ok(results)
    }

    pub async fn get_before(&self, key: MsgKey, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        // key 边界：普通 MsgKey 包一次 Arc，查询与结果分组复用同一把 Arc
        let key = Arc::new(key);
        let query_line = line.saturating_sub(1);
        if query_line > 0 {
            let msgs = self.index.query_before(&key, query_line, remaining).await?;
            let count = msgs.len() as u32;
            if count > 0 {
                let messages: Vec<LineMessage> = msgs.into_iter()
                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                    .collect();
                results.push(GroupedMessages { key: key.clone(), messages });
                remaining = remaining.saturating_sub(count);
            }
        }

        if remaining > 0 {
            if let Some(dates) = self.date_sets.get(key.group_id.as_str()) {
                let mut cursor = key.date.clone();
                loop {
                    if remaining == 0 { break; }
                    let prev = dates.range::<str, _>((Bound::Unbounded, Bound::Excluded(cursor.as_str()))).next_back();
                    match prev {
                        Some(prev_date) => {
                            let prev_key = Arc::new(MsgKey {
                                group_id: key.group_id.clone(),
                                date: prev_date.clone(),
                            });
                            let msgs = self.index.query_last(&prev_key, remaining).await?;
                            if !msgs.is_empty() {
                                let count = msgs.len() as u32;
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages {
                                    key: prev_key,
                                    messages
                                });
                                remaining = remaining.saturating_sub(count);
                            }
                            cursor = prev_date.clone();
                        }
                        None => break,
                    }
                }
            }
        }

        results.reverse();
        Ok(results)
    }

    pub async fn get_after(&self, key: MsgKey, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        // key 边界：普通 MsgKey 包一次 Arc，查询与结果分组复用同一把 Arc
        let key = Arc::new(key);
        let msgs = self.index.query_after(&key, line + 1, remaining).await?;
        let count = msgs.len() as u32;
        if count > 0 {
            let messages: Vec<LineMessage> = msgs.into_iter()
                .map(|(l, msg)| LineMessage { line: l, message: msg })
                .collect();
            results.push(GroupedMessages { key: key.clone(), messages });
            remaining = remaining.saturating_sub(count);
        }

        if remaining > 0 {
            if let Some(dates) = self.date_sets.get(key.group_id.as_str()) {
                let mut cursor = key.date.clone();
                loop {
                    if remaining == 0 { break; }
                    let next = dates.range::<str, _>((Bound::Excluded(cursor.as_str()), Bound::Unbounded)).next();
                    match next {
                        Some(next_date) => {
                            let next_key = Arc::new(MsgKey {
                                group_id: key.group_id.clone(),
                                date: next_date.clone(),
                            });
                            let msgs = self.index.query_first(&next_key, remaining).await?;
                            if !msgs.is_empty() {
                                let count = msgs.len() as u32;
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages {
                                    key: next_key, messages });
                                remaining = remaining.saturating_sub(count);
                            }
                            cursor = next_date.clone();
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(results)
    }

    pub async fn get_range(&self, query: TimeRangeQuery) -> Result<Vec<GroupedMessages>> {
        let mut results = self.index.query_all(query).await?;
        let mut grouped = Vec::with_capacity(results.len());
        for (key,mut msgs) in results.drain(..) {
            let mut messages = Vec::with_capacity(msgs.len());
            for (l, msg) in msgs.drain(..) {
                messages.push(LineMessage { line: l, message: msg });
            }
            grouped.push(GroupedMessages { key, messages });
        }
        Ok(grouped)
    }
}
