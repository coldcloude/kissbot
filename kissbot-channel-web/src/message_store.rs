use std::collections::BTreeSet;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use kai_date;
use kai_file::FileIndexContext;
use kai_file::index::{FilePathGenerator, QueryParser};
use kissbot_api::channel::IncomingMessage;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::Result;

// ========== DTOs ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineMessage {
    pub line: u32,
    pub message: IncomingMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupedMessages {
    pub key: String,
    pub messages: Vec<LineMessage>,
}

// ========== Query types ==========

#[derive(Debug, Clone)]
pub struct TimeRangeQuery {
    pub start: String,
    pub end: String,
}

type DateKey = String;
type GroupIndex = FileIndexContext<TimeRangeQuery, DateKey, IncomingMessage, GroupParser>;

// ========== FilePathGenerator / QueryParser ==========

struct GroupParser {
    group_dir: PathBuf,
}

impl GroupParser {
    fn new(base_dir: &Path, group_id: &str) -> Self {
        Self {
            group_dir: base_dir.join(group_id),
        }
    }
}

#[async_trait::async_trait]
impl FilePathGenerator<DateKey> for GroupParser {
    async fn get_path(&self, key: &DateKey) -> std::result::Result<PathBuf, kai_file::Error> {
        Ok(self.group_dir.join(format!("{}.jsonl", key)))
    }
}

impl QueryParser<TimeRangeQuery, DateKey> for GroupParser {
    fn parse_query(&self, query: TimeRangeQuery) -> Vec<(DateKey, (String, String))> {
        let start_date = kai_date::as_date(&query.start);
        let end_date = kai_date::as_date(&query.end);

        let mut keys = Vec::new();
        // Add start date
        if start_date == end_date {
            keys.push((start_date.to_string(), (query.start.clone(), query.end.clone())));
        } else {
            keys.push((start_date.to_string(), (query.start.clone(), format!("{} 23:59:59", start_date))));
            // Internal dates
            let internal = kai_date::get_internal_dates(start_date, end_date).unwrap_or_default();
            for d in &internal {
                keys.push((d.clone(), (format!("{} 00:00:00", d), format!("{} 23:59:59", d))));
            }
            keys.push((end_date.to_string(), (format!("{} 00:00:00", end_date), query.end.clone())));
        }
        keys
    }
}

// ========== MessageStore ==========

pub struct MessageStore {
    base_dir: PathBuf,
    writer_tx: flume::Sender<IncomingMessage>,
    indices: DashMap<String, GroupIndex>,
    date_sets: DashMap<String, BTreeSet<String>>,
}

impl MessageStore {
    pub fn new(base_dir: PathBuf, _messenger_id: String) -> Arc<Self> {
        let (tx, rx) = flume::unbounded();
        let store = Arc::new(Self {
            base_dir,
            writer_tx: tx,
            indices: DashMap::new(),
            date_sets: DashMap::new(),
        });
        let cloned = store.clone();
        tokio::spawn(async move {
            cloned.writer_loop(rx).await;
        });
        store
    }

    pub fn append(&self, msg: IncomingMessage) {
        let _ = self.writer_tx.send(msg);
    }

    async fn writer_loop(&self, rx: flume::Receiver<IncomingMessage>) {
        while let Ok(msg) = rx.recv_async().await {
            if let Err(e) = self.write_one(msg).await {
                tracing::error!("Message store write error: {}", e);
            }
        }
    }

    async fn write_one(&self, msg: IncomingMessage) -> Result<()> {
        let date_key = kai_date::as_date(&msg.time).to_string();
        let group_id = msg.group_id.as_str();

        // Ensure group directory exists
        let group_dir = self.base_dir.join(group_id);
        tokio::fs::create_dir_all(&group_dir).await?;

        // Append to file
        let file_path = group_dir.join(format!("{}.jsonl", date_key));
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path).await?;
        let line = serde_json::to_string(&msg)?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;

        // Update date set
        self.date_sets
            .entry(group_id.to_string())
            .or_insert_with(BTreeSet::new)
            .insert(date_key.clone());

        // Mark index for incremental update
        if let Some(index) = self.indices.get(group_id) {
            index.mark_obsolete(&date_key);
        }

        Ok(())
    }

    // ========== Index management ==========

    async fn ensure_index(&self, group_id: &str) -> Result<()> {
        if self.indices.contains_key(group_id) {
            return Ok(());
        }
        let parser = GroupParser::new(&self.base_dir, group_id);
        let index = GroupIndex::new(parser);
        if let Some(dates) = self.date_sets.get(group_id) {
            for date in dates.iter() {
                index.mark_all_obsolete(date);
            }
        }
        self.indices.insert(group_id.to_string(), index);
        Ok(())
    }

    // ========== Query methods ==========

    pub async fn get_recent(&self, group_id: &str, n: u32) -> Result<Vec<GroupedMessages>> {
        self.ensure_index(group_id).await?;
        let index = self.indices.get(group_id).unwrap();

        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        if let Some(dates) = self.date_sets.get(group_id) {
            for date_key in dates.iter().rev() {
                if remaining == 0 {
                    break;
                }
                let msgs = index.query_last(date_key, remaining).await?;
                if !msgs.is_empty() {
                    let count = msgs.len() as u32;
                    let messages: Vec<LineMessage> = msgs.into_iter()
                        .map(|(line, msg)| LineMessage { line, message: msg })
                        .collect();
                    results.push(GroupedMessages {
                        key: date_key.clone(),
                        messages,
                    });
                    remaining = remaining.saturating_sub(count);
                }
            }
        }

        results.reverse();
        Ok(results)
    }

    pub async fn get_before(&self, group_id: &str, key: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        self.ensure_index(group_id).await?;
        let index = self.indices.get(group_id).unwrap();

        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        let key_s = key.to_string();
        let msgs = index.query_before(&key_s, line, remaining).await?;
        let count = msgs.len() as u32;
        if count > 0 {
            let messages: Vec<LineMessage> = msgs.into_iter()
                .map(|(l, msg)| LineMessage { line: l, message: msg })
                .collect();
            results.push(GroupedMessages {
                key: key.to_string(),
                messages,
            });
            remaining = remaining.saturating_sub(count);
        }

        if remaining > 0 {
            if let Some(dates) = self.date_sets.get(group_id) {
                let mut cursor = key.to_string();
                loop {
                    if remaining == 0 { break; }
                    let prev = dates.range::<str, _>((Bound::Unbounded, Bound::Excluded(cursor.as_str()))).next_back();
                    match prev {
                        Some(prev_key) => {
                            let msgs = index.query_last(prev_key, remaining).await?;
                            if !msgs.is_empty() {
                                let count = msgs.len() as u32;
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages {
                                    key: prev_key.clone(),
                                    messages,
                                });
                                remaining = remaining.saturating_sub(count);
                            }
                            cursor = prev_key.clone();
                        }
                        None => break,
                    }
                }
            }
        }

        results.reverse();
        Ok(results)
    }

    pub async fn get_after(&self, group_id: &str, key: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        self.ensure_index(group_id).await?;
        let index = self.indices.get(group_id).unwrap();

        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        let key_s = key.to_string();
        let msgs = index.query_after(&key_s, line + 1, remaining).await?;
        let count = msgs.len() as u32;
        if count > 0 {
            let messages: Vec<LineMessage> = msgs.into_iter()
                .map(|(l, msg)| LineMessage { line: l, message: msg })
                .collect();
            results.push(GroupedMessages { key: key.to_string(), messages });
            remaining = remaining.saturating_sub(count);
        }

        if remaining > 0 {
            if let Some(dates) = self.date_sets.get(group_id) {
                let mut cursor = key.to_string();
                loop {
                    if remaining == 0 { break; }
                    let next = dates.range::<str, _>((Bound::Excluded(cursor.as_str()), Bound::Unbounded)).next();
                    match next {
                        Some(next_key) => {
                            let msgs = index.query_first(next_key, remaining).await?;
                            if !msgs.is_empty() {
                                let count = msgs.len() as u32;
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages { key: next_key.clone(), messages });
                                remaining = remaining.saturating_sub(count);
                            }
                            cursor = next_key.clone();
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(results)
    }

    pub async fn get_range(&self, group_id: &str, start: &str, end: &str) -> Result<Vec<GroupedMessages>> {
        self.ensure_index(group_id).await?;
        let index = self.indices.get(group_id).unwrap();

        let query = TimeRangeQuery {
            start: start.to_string(),
            end: end.to_string(),
        };

        let results: Vec<(DateKey, Vec<(u32, IncomingMessage)>)> = index.query_all(query).await?;

        let grouped: Vec<GroupedMessages> = results.into_iter()
            .filter(|(_, msgs)| !msgs.is_empty())
            .map(|(key, msgs)| {
                let messages: Vec<LineMessage> = msgs.into_iter()
                    .map(|(line, msg)| LineMessage { line, message: msg })
                    .collect();
                GroupedMessages { key, messages }
            })
            .collect();

        Ok(grouped)
    }
}
