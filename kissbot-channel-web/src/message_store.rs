use std::collections::{BTreeSet, HashMap};
use std::ops::Bound;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use axum::async_trait;
use dashmap::DashMap;
use kai_date::{self, as_date, get_date_time_segments};
use kai_file::FileIndexContext;
use kai_file::appender::{FileAppendWriter, FileAppendWriterContext, FileObjectAppender, NoopErrorHandler};
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
    pub key: MsgKey,
    pub messages: Vec<LineMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MsgKey {
    pub group_id: String,
    pub date: String,
}

// ========== Query types ==========

#[derive(Debug, Clone)]
pub struct TimeRangeQuery {
    pub group_id: Arc<String>,
    pub start: Arc<String>,
    pub end: Arc<String>,
}

type GroupIndex = FileIndexContext<TimeRangeQuery, MsgKey, IncomingMessage, MessageParser>;

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
impl FilePathGenerator<MsgKey> for MessageParser {
    async fn get_path(&self, key: &MsgKey) -> std::result::Result<PathBuf, kai_file::Error> {
        Ok(self.base_dir
            .join(&key.group_id)
            .join(format!("{}.jsonl", key.date)))
    }
}

impl QueryParser<TimeRangeQuery, MsgKey> for MessageParser {
    fn parse_query(&self, query: TimeRangeQuery) -> Vec<(MsgKey, (String, String))> {
        let mut keys = Vec::new();
        if let Ok(mut ranges) = get_date_time_segments(&query.start, &query.end) {
            for range in ranges.drain(..) {
                let date = as_date(range.0.as_str()).to_string();
                keys.push((MsgKey {
                    group_id: query.group_id.as_str().to_string(),
                    date,
                }, range));
            }
        }
        keys
    }
}

// ========== FileAppendWriter ==========

struct MessageFileWriterContext {
    path: PathBuf,
    index: Weak<GroupIndex>,
    date_sets: Weak<DashMap<String, BTreeSet<String>>>,
}

#[async_trait]
impl FileAppendWriterContext<MsgKey, Arc<IncomingMessage>> for MessageFileWriterContext {
    async fn write(&mut self, key: &MsgKey, records: Vec<Arc<IncomingMessage>>) -> std::result::Result<(), kai_file::Error> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;

        for record in &records {
            let line = serde_json::to_string(record)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        if let Some(index) = self.index.upgrade() {
            index.mark_obsolete(key);
        }

        if let Some(sets) = self.date_sets.upgrade() {
            sets.entry(key.group_id.clone())
                .or_insert_with(BTreeSet::new)
                .insert(key.date.clone());
        }

        Ok(())
    }
}

struct MessageFileWriter {
    base_dir: PathBuf,
    index: Weak<GroupIndex>,
    date_sets: Weak<DashMap<String, BTreeSet<String>>>,
    map: Arc<Mutex<HashMap<MsgKey, Weak<Mutex<MessageFileWriterContext>>>>>,
}

impl MessageFileWriter {
    fn create_context(&self, key: &MsgKey) -> MessageFileWriterContext {
        let path = self.base_dir
            .join(&key.group_id)
            .join(format!("{}.jsonl", key.date));
        MessageFileWriterContext {
            path,
            index: self.index.clone(),
            date_sets: self.date_sets.clone(),
        }
    }
}

#[async_trait]
impl FileAppendWriter<MsgKey, Arc<IncomingMessage>, MessageFileWriterContext> for MessageFileWriter{    
    async fn get_lock(&self, key: &MsgKey) -> Arc<Mutex<MessageFileWriterContext>> {
        let mut guard = self.map.lock().await;
        match guard.entry(key.clone()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                match entry.get().upgrade() {
                    Some(lock) => {
                        lock
                    }
                    None => {
                        let ctx = self.create_context(&key);
                        let lock = Arc::new(Mutex::new(ctx));
                        entry.insert(Arc::downgrade(&lock));
                        lock
                    }
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let ctx = self.create_context(&key);
                let lock = Arc::new(Mutex::new(ctx));
                entry.insert(Arc::downgrade(&lock));
                lock
            }
        }
    }

    async fn remove_lock(&self, key: &MsgKey) {
        let mut guard = self.map.lock().await;
        if let Some(entry) = guard.get(key) {
            if entry.strong_count() == 0 {
                guard.remove(key);
            }
        }
    }
}

// ========== MessageStore ==========

pub struct MessageStore {
    appender: FileObjectAppender<MsgKey, Arc<IncomingMessage>, MessageFileWriter, MessageFileWriterContext>,
    index: Arc<GroupIndex>,
    date_sets: Arc<DashMap<String, BTreeSet<String>>>,
}

impl MessageStore {
    pub fn new(base_dir: PathBuf) -> Arc<Self> {
        let parser = MessageParser::new(base_dir.clone());
        let index: Arc<GroupIndex> = Arc::new(FileIndexContext::new(parser));
        let date_sets: Arc<DashMap<String, BTreeSet<String>>> = Arc::new(DashMap::new());
        let writer = Arc::new(MessageFileWriter {
            base_dir,
            index: Arc::downgrade(&index),
            date_sets: Arc::downgrade(&date_sets),
            map: Arc::new(Mutex::new(HashMap::new()))
        });
        let appender = FileObjectAppender::new(
            writer,
            Arc::new(NoopErrorHandler {}),
            Duration::from_secs(5),
            100,
        );
        Arc::new(Self { appender, index, date_sets })
    }

    pub async fn append(&self, msg: Arc<IncomingMessage>) {
        let date = kai_date::as_date(&msg.time).to_string();
        let key = MsgKey {
            group_id: msg.group_id.to_string(),
            date,
        };
        let _ = self.appender.append(key, vec![msg]).await;
    }

    // ========== Query methods ==========

    pub async fn get_recent(&self, group_id: &str, n: u32) -> Result<Vec<GroupedMessages>> {
        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        if let Some(dates) = self.date_sets.get(group_id) {
            for date in dates.iter().rev() {
                if remaining == 0 { break; }
                let key = MsgKey {
                    group_id: group_id.to_string(),
                    date: date.clone(),
                };
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
                let mut cursor = key.date.to_string();
                loop {
                    if remaining == 0 { break; }
                    let prev = dates.range::<str, _>((Bound::Unbounded, Bound::Excluded(cursor.as_str()))).next_back();
                    match prev {
                        Some(prev_date) => {
                            let prev_key = MsgKey {
                                group_id: key.group_id.clone(),
                                date: prev_date.clone(),
                            };
                            let msgs = self.index.query_last(&prev_key, remaining).await?;
                            if !msgs.is_empty() {
                                let count = msgs.len() as u32;
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages {
                                    key: MsgKey {
                                        group_id: key.group_id.clone(),
                                        date: prev_date.clone(),
                                    },
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
                            let next_key = MsgKey {
                                group_id: key.group_id.clone(),
                                date: next_date.clone(),
                            };
                            let msgs = self.index.query_first(&next_key, remaining).await?;
                            if !msgs.is_empty() {
                                let count = msgs.len() as u32;
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages {
                                    key: MsgKey {
                                        group_id: key.group_id.clone(),
                                        date: next_date.clone()
                                    }, messages });
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
