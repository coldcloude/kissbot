use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;
use dashmap::DashMap;
use kai_date;
use kai_file::FileIndexContext;
use kai_file::index::{FilePathGenerator, QueryParser, Record};
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
            keys.push((start_date.to_string(), (query.start, query.end)));
        } else {
            keys.push((start_date.to_string(), (query.start, format!("{} 23:59:59", start_date))));
            // Internal dates
            let internal = kai_date::get_internal_dates(start_date, end_date).unwrap_or_default();
            for d in &internal {
                keys.push((d.clone(), (format!("{} 00:00:00", d), format!("{} 23:59:59", d))));
            }
            keys.push((end_date.to_string(), (format!("{} 00:00:00", end_date), query.end)));
        }
        keys
    }
}

// ========== MessageStore ==========

pub struct MessageStore {
    base_dir: PathBuf,
    messenger_id: String,
    writer_tx: flume::Sender<IncomingMessage>,
    indices: DashMap<String, GroupIndex>,
    date_sets: DashMap<String, BTreeSet<String>>,
}

impl MessageStore {
    pub fn new(base_dir: PathBuf, messenger_id: String) -> Arc<Self> {
        let (tx, rx) = flume::unbounded();
        let store = Arc::new(Self {
            base_dir,
            messenger_id,
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
            index.mark_obsolete(date_key);
        }

        Ok(())
    }
}
