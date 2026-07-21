# MessageStore 重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 统一 FileIndexContext 为单实例、key 改为 MsgKey 三元组、写入侧替换为 FileObjectAppender

**Architecture:** MsgKey (messenger_id, group_id, date) 作为统一 key；MessageFileWriter 实现 FileAppendWriter，通过 Weak<FileIndexContext> 在写入后通知索引更新；单 FileObjectAppender 管理所有 key 的写入

**Tech Stack:** Rust, kai-file (FileObjectAppender, FileIndexContext), kai-date, dashmap, tokio

## Global Constraints

- 所有消息文件路径: `{base_dir}/{messenger_id}/{group_id}/{date}.jsonl`
- MsgKey 字段顺序: messenger_id, group_id, date
- 跨 key 查询基于 `Arc<DashMap<(String, String), BTreeSet<String>>>` 导航
- `MessageFileWriter` 通过 `Weak` 持有 index 和 date_sets 引用
- `append` 返回 `Result`（当前忽略错误）

---

### Task 1: 重写 message_store.rs

**Files:**
- Modify: `kissbot-channel-web/src/message_store.rs`
- Verify: cargo build

完整替换 message_store.rs 的内容为以下新的实现。此任务涉及所有核心类型（MsgKey、TimeRangeQuery、MessageParser、MessageFileWriter、MessageStore）的一次性重写。

**Interfaces:**
- Produces: `MsgKey`, `TimeRangeQuery`, `MessageParser`, `MessageFileWriter`, `MessageStore`, `LineMessage`, `GroupedMessages`
- `MessageStore::new(base_dir: PathBuf) -> Arc<Self>`
- `MessageStore::append(&self, msg: IncomingMessage)`
- `MessageStore::get_recent(&self, messenger_id: &str, group_id: &str, n: u32) -> Result<Vec<GroupedMessages>>`
- `MessageStore::get_before(&self, messenger_id: &str, group_id: &str, key_date: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>>`
- `MessageStore::get_after(&self, messenger_id: &str, group_id: &str, key_date: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>>`
- `MessageStore::get_range(&self, messenger_id: &str, group_id: &str, start: &str, end: &str) -> Result<Vec<GroupedMessages>>`

- [ ] **Step 1: 添加导入**

更新文件头部的 `use` 语句。确保包含：

```rust
use std::collections::BTreeSet;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use dashmap::DashMap;
use kai_date;
use kai_file::FileIndexContext;
use kai_file::appender::{FileAppendWriter, FileObjectAppender, NoopErrorHandler};
use kai_file::index::{FilePathGenerator, QueryParser, Record};
use kissbot_api::channel::IncomingMessage;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::Result;
```

- [ ] **Step 2: 添加 DTO 和 MsgKey**

```rust
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MsgKey {
    pub messenger_id: String,
    pub group_id: String,
    pub date: String,
}
```

- [ ] **Step 3: TimeRangeQuery**

```rust
#[derive(Debug, Clone)]
pub struct TimeRangeQuery {
    pub messenger_id: String,
    pub group_id: String,
    pub start: String,
    pub end: String,
}

type GroupIndex = FileIndexContext<TimeRangeQuery, MsgKey, IncomingMessage, MessageParser>;
```

- [ ] **Step 4: MessageParser**

```rust
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
            .join(&key.messenger_id)
            .join(&key.group_id)
            .join(format!("{}.jsonl", key.date)))
    }
}

impl QueryParser<TimeRangeQuery, MsgKey> for MessageParser {
    fn parse_query(&self, query: TimeRangeQuery) -> Vec<(MsgKey, (String, String))> {
        let start_date = kai_date::as_date(&query.start);
        let end_date = kai_date::as_date(&query.end);
        let mut keys = Vec::new();

        fn make_key(mid: &str, gid: &str, date: &str) -> MsgKey {
            MsgKey { messenger_id: mid.to_string(), group_id: gid.to_string(), date: date.to_string() }
        }

        if start_date == end_date {
            keys.push((make_key(&query.messenger_id, &query.group_id, start_date), (query.start, query.end)));
        } else {
            keys.push((make_key(&query.messenger_id, &query.group_id, start_date), (query.start, format!("{} 23:59:59", start_date))));
            let internal = kai_date::get_internal_dates(start_date, end_date).unwrap_or_default();
            for d in &internal {
                keys.push((make_key(&query.messenger_id, &query.group_id, d), (format!("{} 00:00:00", d), format!("{} 23:59:59", d))));
            }
            keys.push((make_key(&query.messenger_id, &query.group_id, end_date), (format!("{} 00:00:00", end_date), query.end)));
        }
        keys
    }
}
```

- [ ] **Step 5: MessageFileWriter**

```rust
struct MessageFileWriter {
    base_dir: PathBuf,
    index: Weak<GroupIndex>,
    date_sets: Weak<DashMap<(String, String), BTreeSet<String>>>,
}

impl MessageFileWriter {
    fn new(
        base_dir: PathBuf,
        index: Weak<GroupIndex>,
        date_sets: Weak<DashMap<(String, String), BTreeSet<String>>>,
    ) -> Self {
        Self { base_dir, index, date_sets }
    }
}

#[async_trait::async_trait]
impl FileAppendWriter<MsgKey, IncomingMessage> for MessageFileWriter {
    async fn write(&self, key: &MsgKey, records: Vec<IncomingMessage>) -> std::result::Result<(), kai_file::Error> {
        let path = self.base_dir
            .join(&key.messenger_id)
            .join(&key.group_id)
            .join(format!("{}.jsonl", key.date));

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(kai_file::Error::IoError)?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path).await
            .map_err(kai_file::Error::IoError)?;

        for record in &records {
            let line = serde_json::to_string(record).map_err(kai_file::Error::Json)?;
            file.write_all(line.as_bytes()).await.map_err(kai_file::Error::IoError)?;
            file.write_all(b"\n").await.map_err(kai_file::Error::IoError)?;
        }

        if let Some(index) = self.index.upgrade() {
            index.mark_obsolete(key);
        }

        if let Some(sets) = self.date_sets.upgrade() {
            sets.entry((key.messenger_id.clone(), key.group_id.clone()))
                .or_insert_with(BTreeSet::new)
                .insert(key.date.clone());
        }

        Ok(())
    }
}
```

- [ ] **Step 6: MessageStore 结构体与构造**

```rust
pub struct MessageStore {
    appender: FileObjectAppender<MsgKey, IncomingMessage, MessageFileWriter>,
    index: Arc<GroupIndex>,
    date_sets: Arc<DashMap<(String, String), BTreeSet<String>>>,
}

impl MessageStore {
    pub fn new(base_dir: PathBuf) -> Arc<Self> {
        let parser = MessageParser::new(base_dir.clone());
        let index: Arc<GroupIndex> = Arc::new(FileIndexContext::new(parser));
        let date_sets: Arc<DashMap<(String, String), BTreeSet<String>>> = Arc::new(DashMap::new());
        let writer = MessageFileWriter::new(
            base_dir,
            Arc::downgrade(&index),
            Arc::downgrade(&date_sets),
        );
        let appender = FileObjectAppender::new(
            writer,
            NoopErrorHandler,
            Duration::from_secs(5),
            100,
        );
        Arc::new(Self { appender, index, date_sets })
    }

    pub async fn append(&self, msg: IncomingMessage) {
        let date = kai_date::as_date(&msg.time).to_string();
        let key = MsgKey {
            messenger_id: msg.messenger_id.to_string(),
            group_id: msg.group_id.to_string(),
            date,
        };
        let _ = self.appender.append(key, vec![msg]).await;
    }
```

- [ ] **Step 7: 查询方法**

在 `impl MessageStore` 块内添加：

```rust
    pub async fn get_recent(&self, messenger_id: &str, group_id: &str, n: u32) -> Result<Vec<GroupedMessages>> {
        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        let group_key = (messenger_id.to_string(), group_id.to_string());
        if let Some(dates) = self.date_sets.get(&group_key) {
            for date in dates.iter().rev() {
                if remaining == 0 { break; }
                let key = MsgKey {
                    messenger_id: messenger_id.to_string(),
                    group_id: group_id.to_string(),
                    date: date.clone(),
                };
                let msgs = self.index.query_last(&key, remaining).await?;
                if !msgs.is_empty() {
                    let count = msgs.len() as u32;
                    let messages: Vec<LineMessage> = msgs.into_iter()
                        .map(|(line, msg)| LineMessage { line, message: msg })
                        .collect();
                    results.push(GroupedMessages { key: date.clone(), messages });
                    remaining = remaining.saturating_sub(count);
                }
            }
        }

        results.reverse();
        Ok(results)
    }

    pub async fn get_before(&self, messenger_id: &str, group_id: &str, key_date: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        let key = MsgKey {
            messenger_id: messenger_id.to_string(),
            group_id: group_id.to_string(),
            date: key_date.to_string(),
        };

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
                results.push(GroupedMessages { key: key_date.to_string(), messages });
                remaining = remaining.saturating_sub(count);
            }
        }

        if remaining > 0 {
            let group_key = (messenger_id.to_string(), group_id.to_string());
            if let Some(dates) = self.date_sets.get(&group_key) {
                let mut cursor = key_date.to_string();
                loop {
                    if remaining == 0 { break; }
                    let prev = dates.range::<str, _>((Bound::Unbounded, Bound::Excluded(cursor.as_str()))).next_back();
                    match prev {
                        Some(prev_date) => {
                            let prev_key = MsgKey {
                                messenger_id: messenger_id.to_string(),
                                group_id: group_id.to_string(),
                                date: prev_date.clone(),
                            };
                            let msgs = self.index.query_last(&prev_key, remaining).await?;
                            if !msgs.is_empty() {
                                let count = msgs.len() as u32;
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages { key: prev_date.clone(), messages });
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

    pub async fn get_after(&self, messenger_id: &str, group_id: &str, key_date: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        let key = MsgKey {
            messenger_id: messenger_id.to_string(),
            group_id: group_id.to_string(),
            date: key_date.to_string(),
        };

        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        let msgs = self.index.query_after(&key, line + 1, remaining).await?;
        let count = msgs.len() as u32;
        if count > 0 {
            let messages: Vec<LineMessage> = msgs.into_iter()
                .map(|(l, msg)| LineMessage { line: l, message: msg })
                .collect();
            results.push(GroupedMessages { key: key_date.to_string(), messages });
            remaining = remaining.saturating_sub(count);
        }

        if remaining > 0 {
            let group_key = (messenger_id.to_string(), group_id.to_string());
            if let Some(dates) = self.date_sets.get(&group_key) {
                let mut cursor = key_date.to_string();
                loop {
                    if remaining == 0 { break; }
                    let next = dates.range::<str, _>((Bound::Excluded(cursor.as_str()), Bound::Unbounded)).next();
                    match next {
                        Some(next_date) => {
                            let next_key = MsgKey {
                                messenger_id: messenger_id.to_string(),
                                group_id: group_id.to_string(),
                                date: next_date.clone(),
                            };
                            let msgs = self.index.query_first(&next_key, remaining).await?;
                            if !msgs.is_empty() {
                                let count = msgs.len() as u32;
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages { key: next_date.clone(), messages });
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

    pub async fn get_range(&self, messenger_id: &str, group_id: &str, start: &str, end: &str) -> Result<Vec<GroupedMessages>> {
        let query = TimeRangeQuery {
            messenger_id: messenger_id.to_string(),
            group_id: group_id.to_string(),
            start: start.to_string(),
            end: end.to_string(),
        };

        let results: Vec<(MsgKey, Vec<(u32, IncomingMessage)>)> = self.index.query_all(query).await?;

        let grouped: Vec<GroupedMessages> = results.into_iter()
            .filter(|(_, msgs)| !msgs.is_empty())
            .map(|(key, msgs)| {
                let messages: Vec<LineMessage> = msgs.into_iter()
                    .map(|(line, msg)| LineMessage { line, message: msg })
                    .collect();
                GroupedMessages { key: key.date, messages }
            })
            .collect();

        Ok(grouped)
    }
}
```

- [ ] **Step 8: 编译验证**

Run: `cargo build -p kissbot-channel-web` (from `kissbot-channel-web/` directory)
Expected: 编译成功

- [ ] **Step 9: 提交**

```bash
git add kissbot-channel-web/src/message_store.rs
git commit -m "refactor: rewrite MessageStore with single FileIndexContext and FileObjectAppender"
```

---

### Task 2: HTTP handler + WebMessengerCreator 适配

**Files:**
- Modify: `kissbot-channel-web/src/http.rs`
- Modify: `kissbot-channel-web/src/messenger.rs`
- Verify: cargo build, cargo test

**Interfaces:**
- Consumes: `MessageStore::get_recent(messenger_id, group_id, n)` (now requires messenger_id)
- `MessageStore::new(base_dir)` (no longer takes messenger_id param)

- [ ] **Step 1: 更新 HTTP handlers**

在 `kissbot-channel-web/src/http.rs` 中，为每个消息查询 handler 添加 `messenger_id` 参数：

对 `handle_messages_recent`：
```rust
async fn handle_messages_recent(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ApiResponse<Vec<GroupedMessages>>> {
    let group_id = match params.get("group_id") {
        Some(id) => id,
        None => return Json(ApiResponse::error("Missing group_id".to_string())),
    };
    let n: u32 = params.get("n").and_then(|v| v.parse().ok()).unwrap_or(20);
    match messenger.message_store.get_recent(&messenger.messenger_id, group_id, n).await {
        Ok(msgs) => Json(ApiResponse::success(msgs)),
        Err(e) => Json(ApiResponse::error(e.to_string())),
    }
}
```

对 `handle_messages_before`：
```rust
async fn handle_messages_before(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ApiResponse<Vec<GroupedMessages>>> {
    let group_id = match params.get("group_id") {
        Some(id) => id,
        None => return Json(ApiResponse::error("Missing group_id".to_string())),
    };
    let key = match params.get("key") {
        Some(k) => k,
        None => return Json(ApiResponse::error("Missing key".to_string())),
    };
    let line: u32 = match params.get("line").and_then(|v| v.parse().ok()) {
        Some(l) => l,
        None => return Json(ApiResponse::error("Missing or invalid line".to_string())),
    };
    let n: u32 = params.get("n").and_then(|v| v.parse().ok()).unwrap_or(10);
    match messenger.message_store.get_before(&messenger.messenger_id, group_id, key, line, n).await {
        Ok(msgs) => Json(ApiResponse::success(msgs)),
        Err(e) => Json(ApiResponse::error(e.to_string())),
    }
}
```

对 `handle_messages_after`：
```rust
async fn handle_messages_after(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ApiResponse<Vec<GroupedMessages>>> {
    // 同 before，调用 messenger.message_store.get_after(&messenger.messenger_id, group_id, key, line, n)
}
```

对 `handle_messages_range`：
```rust
async fn handle_messages_range(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ApiResponse<Vec<GroupedMessages>>> {
    // 调用 messenger.message_store.get_range(&messenger.messenger_id, group_id, start, end)
}
```

- [ ] **Step 2: 更新 WebMessengerCreator**

在 `kissbot-channel-web/src/messenger.rs` 的 `WebMessengerCreator::create()` 中：

原来：
```rust
let message_store = MessageStore::new(messages_base, mid.to_string());
```

改为：
```rust
let message_store = MessageStore::new(messages_base);
```

- [ ] **Step 3: 编译验证**

Run: `cargo build -p kissbot-channel-web`
Expected: 编译成功，无 warning

- [ ] **Step 4: 测试验证**

Run: `cargo test -p kissbot-api` (从 `kissbot-api/` 目录)
Expected: 71 passed, 0 failed

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "refactor: update HTTP handlers and messenger for new MessageStore API"
```
