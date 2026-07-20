# channel-web 消息存储与 SSE 重构 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现基于 JSONL 文件的本地消息存储，替换 SSE/消息查询 DTO 为 `IncomingMessage`，提供最近消息/翻页/时间范围查询 API

**Architecture:** MessageStore 核心结构 + 写入队列 + 基于 kai-file FileIndexContext 的查询索引；SSE 改为全局 broadcast；WebMessenger 内嵌 MessageStore

**Tech Stack:** Rust, kai-file (FileIndexContext), kai-date, flume, tokio, serde, chrono

## Global Constraints

- 所有消息文件存为 JSONL（每行一条 `IncomingMessage` JSON）
- 文件路径: `data/messages/{messenger_id}/{group_id}/{date}.jsonl`
- SSE 推送全部 `IncomingMessage` 而非 `SseMessage`
- `Record for IncomingMessage` 实现在 `kissbot-api` 中
- 所有查询返回 `Vec<GroupedMessages>`，各组内消息按行号升序排列
- 跨 key 查询：不足时自动查找前/后 key

---

### Task 1: kissbot-api — Record impl 与依赖

**Files:**
- Modify: `kissbot-api/Cargo.toml`
- Modify: `kissbot-api/src/channel.rs:121`
- Verify: cargo build

- [ ] **Step 1: 添加 kai-file 依赖**

编辑 `kissbot-api/Cargo.toml`，在 `[dependencies]` 中新增：

```toml
kai-file = { path = "../kai-rs/kai-file" }
```

- [ ] **Step 2: 实现 Record for IncomingMessage**

在 `kissbot-api/src/channel.rs` 末尾（`IncomingMessage` 定义之后，最后一个 `}` 之后）添加：

```rust
use kai_file::Record;

impl Record for IncomingMessage {
    fn time(&self) -> &str {
        self.time.as_str()
    }
}
```

注意：在同一个文件中 `use` 放在文件顶部已有 `use` 块之后；确认 `IncomingMessage` 定义在 `channel.rs` 的位置，放在它定义的下方即可。

- [ ] **Step 3: 编译验证**

Run: `cargo build -p kissbot-api`
Expected: 编译成功

- [ ] **Step 4: 提交**

```bash
git add kissbot-api/Cargo.toml kissbot-api/src/channel.rs
git commit -m "feat(kissbot-api): implement Record for IncomingMessage"
```

---

### Task 2: MessageStore — 数据结构、写入队列与文件追加

**Files:**
- Create: `kissbot-channel-web/src/message_store.rs`
- Modify: `kissbot-channel-web/src/error.rs`
- Modify: `kissbot-channel-web/Cargo.toml`
- Verify: cargo build

**Interfaces:**
- Consumes: `IncomingMessage` (kissbot-api), `kai_file::*`, `kai_date::as_date`
- Produces: `LineMessage`, `GroupedMessages`, `MessageStore::new()`, `MessageStore::append()`

- [ ] **Step 1: 添加依赖**

编辑 `kissbot-channel-web/Cargo.toml`，在 `[dependencies]` 中新增：

```toml
kai-file = { path = "../kai-rs/kai-file" }
kai-date = { path = "../kai-rs/kai-date" }
```

- [ ] **Step 2: 添加 KaiFileError 变体**

编辑 `kissbot-channel-web/src/error.rs`，在 `Error` enum 中新增变体：

```rust
    #[error("KaiFile error: {0}")]
    KaiFileError(#[from] kai_file::Error),
```

- [ ] **Step 3: 创建 message_store.rs — 数据结构和构造函数**

```rust
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;
use dashmap::DashMap;
use kai_date;
use kai_file::{FileIndexContext, FilePathGenerator, QueryParser, Record};
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
```

- [ ] **Step 4: 编译验证**

Run: `cargo build -p kissbot-channel-web`
Expected: 编译成功（有预存 warning 无视）

- [ ] **Step 5: 提交**

```bash
git add kissbot-channel-web/Cargo.toml kissbot-channel-web/src/error.rs kissbot-channel-web/src/message_store.rs
git commit -m "feat(channel-web): add MessageStore with queue writer"
```

---

### Task 3: SSE 重构 — 全局 broadcast

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs`
- Modify: `kissbot-channel-web/src/http.rs`
- Verify: cargo build

**Interfaces:**
- Consumes: `SseDispatcher` 改造为全局模式
- Produces: SSE handler 不再使用 group_id，`send()` 推送 admin 视角 `IncomingMessage`

- [ ] **Step 1: 重写 SseDispatcher**

在 `messenger.rs` 中替换 SseDispatcher 实现（替换第 27-47 行）：

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

pub struct SseDispatcher {
    senders: Arc<Mutex<HashMap<Uuid, flume::Sender<String>>>>,
}

impl SseDispatcher {
    pub fn new() -> Self {
        Self { senders: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn register(&self) -> flume::Receiver<String> {
        let (tx, rx) = flume::unbounded();
        let id = Uuid::new_v4();
        self.senders.lock().unwrap().insert(id, tx);
        rx
    }

    pub fn push(&self, data: &str) {
        let mut senders = self.senders.lock().unwrap();
        senders.retain(|_, tx| tx.try_send(data.to_string()).is_ok());
    }
}
```

同时删除 `use dashmap::DashMap;` 的导入（如果不再使用——检查 `messenger.rs` 中 `DashMap` 仍被其他类型使用则保留）。

Uuid 需要添加 use: `use uuid::Uuid;` — 检查是否有此 use，没有则加。

- [ ] **Step 2: 删除 SseMessage 和 SsePayload**

删除 `messenger.rs` 中的以下代码段（第 83-101 行）：

```rust
// ========== SSE 消息结构（编译检查的 JSON 序列化） ==========

#[derive(Debug, Serialize)]
struct SsePayload<'a> {
    r#type: &'a str,
    data: SseMessage,
}

#[derive(Debug, Serialize)]
struct SseMessage {
    msg_id: Arc<String>,
    messenger_id: Arc<String>,
    user_id: Arc<String>,
    group_id: Arc<String>,
    is_self: usize,
    msg_type: Arc<String>,
    content: Arc<String>,
    time: Arc<String>,
}
```

检查 `use serde::Serialize;` 是否仍被其他代码使用；如果仅被上述类型使用，可移除导入。

- [ ] **Step 3: 修改 send() 中的 SSE 推送**

在 `messenger.rs` 的 `send()` 方法中（第 430-446 行附近），替换 SSE 推送代码：

将原来的：

```rust
        // 推 SSE
        let group_id = outgoing.group_id.clone();
        let response_content = new_content;
        let sse_event = SseMessage {
            msg_id: msg_id.clone(),
            messenger_id,
            user_id: outgoing.user_id.clone(),
            group_id: outgoing.group_id.clone(),
            is_self: 1,
            msg_type: outgoing.msg_type.clone(),
            content: Arc::new(serde_json::to_string(&response_content).unwrap_or_default()),
            time: time.clone(),
        };
        let sse_payload = SsePayload { r#type: "message", data: sse_event };
        if let Ok(json) = serde_json::to_string(&sse_payload) {
            self.sse.push(group_id.as_str(), &json);
        }
```

替换为：

```rust
        // 构建 admin 视角的 IncomingMessage，推 SSE + 写入存储
        let admin_msg = IncomingMessage {
            msg_id: msg_id.clone(),
            messenger_id: messenger_id.clone(),
            user_id: ADMIN_USER_ID.clone(),
            group_id: outgoing.group_id.clone(),
            is_self: 1,
            msg_type: outgoing.msg_type.clone(),
            content: new_content.clone(),
            time: time.clone(),
        };
        if let Ok(json) = serde_json::to_string(&admin_msg) {
            self.sse.push(&json);
        }
        self.message_store.append(admin_msg);
```

注意：`messenger_id` 在原有代码中消耗了所有权（`messenger_id` 被移入 `SseMessage`），现在需要改为克隆：`messenger_id.clone()`（已使用 `clone()` 在 `admin_msg` 构造中）。原有 `let messenger_id = self.messenger_id.clone();` 已在第 405 行定义，此处直接使用即可。

- [ ] **Step 4: 添加 message_store 字段到 WebMessenger**

在 `WebMessenger` 结构体（第 105-116 行）新增字段：

```rust
    pub message_store: Arc<MessageStore>,
```

并在文件顶部添加 `use crate::message_store::MessageStore;`

- [ ] **Step 5: 更新 WebMessenger::new() 构造函数**

将 `message_store` 参数添加到 new() 中：

```rust
    pub fn new(
        messenger_id: Arc<String>,
        repo_path: PathBuf,
        config: Arc<RwLock<WebMessengerRepo>>,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
        attachment_dir: &str,
        message_store: Arc<MessageStore>,
    ) -> Self {
        Self {
            messenger_id,
            repo_path,
            config,
            msg_id_seq: AtomicU32::new(0),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            on_user_remove,
            sse: Arc::new(SseDispatcher::new()),
            attachment_store: Arc::new(AttachmentStore::new(attachment_dir)),
            message_store,
        }
    }
```

- [ ] **Step 6: 更新 SSE handler（http.rs）**

在 `http.rs` 中修改 `handle_sse_events`：

原来（按 group 注册多个 receiver）：

```rust
async fn handle_sse_events(
    State(messenger): State<Arc<WebMessenger>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let groups = messenger.config_groups().await;
    let sse = messenger.sse.clone();

    let mut receivers = Vec::new();
    for group in groups.iter() {
        let rx = sse.register(&group.group_id);
        receivers.push(rx);
    }

    let streams: Vec<_> = receivers.into_iter().map(|rx| {
        rx.into_stream().map(|data| Ok(Event::default().data(data)))
    }).collect();

    let merged = futures::stream::select_all(streams);

    Sse::new(merged).keep_alive(KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("keep-alive"))
}
```

改为：

```rust
async fn handle_sse_events(
    State(messenger): State<Arc<WebMessenger>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = messenger.sse.register();
    let stream = rx.into_stream().map(|data| Ok(Event::default().data(data)));
    Sse::new(stream).keep_alive(KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("keep-alive"))
}
```

- [ ] **Step 7: 编译验证**

Run: `cargo build -p kissbot-channel-web`
Expected: 编译成功

- [ ] **Step 8: 提交**

```bash
git add kissbot-channel-web/src/messenger.rs kissbot-channel-web/src/http.rs
git commit -m "refactor(channel-web): SSE global broadcast, replace SseMessage with IncomingMessage"
```

---

### Task 4: MessageStore — 查询方法与索引集成

**Files:**
- Modify: `kissbot-channel-web/src/message_store.rs`
- Verify: cargo build

**Interfaces:**
- Consumes: FileIndexContext lazy init, date_set navigation
- Produces: `get_recent()`, `get_before()`, `get_after()`, `get_range()`, `ensure_index()`

- [ ] **Step 1: 实现索引初始化**

在 `MessageStore` impl 中添加：

```rust
    async fn ensure_index(&self, group_id: &str) -> Result<()> {
        if self.indices.contains_key(group_id) {
            return Ok(());
        }
        let group_dir = self.base_dir.join(group_id);
        let parser = GroupParser::new(&self.base_dir, group_id);
        let index = GroupIndex::new(parser);
        // Mark all existing date keys for initial indexing
        if let Some(dates) = self.date_sets.get(group_id) {
            for date in dates.iter() {
                index.mark_all_obsolete(date.clone());
            }
        }
        self.indices.insert(group_id.to_string(), index);
        Ok(())
    }
```

- [ ] **Step 2: 实现 get_recent**

```rust
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
                        .rev() // query_last returns descending; reverse to ascending
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

        // Reverse to oldest-first chronological order
        results.reverse();
        Ok(results)
    }
```

注意：`FileIndexContext` 的 `query_last` 返回 `Result<Vec<(u32, R)>, kai_file::Error>`，但我们定义了 `use crate::error::Result`（即 `std::result::Result<T, crate::Error>`）。直接在 `query_last` 返回上调用 `.await` 后需要用 `map_err` 转换错误类型。上面已经用了 `map_err` 把 `kai_file::Error` 转为 `crate::Error::KaiFileError`，但可以通过 `?` 操作符配合 `#[from]` 自动转换。更好的写法：

```rust
let msgs = index.query_last(date_key, remaining).await?;
```

由于我们在 `error.rs` 中新增了 `KaiFileError(#[from] kai_file::Error)`，`?` 可以自动转换 `kai_file::Error` 为 `crate::Error`。所以上面不需要显式 `map_err`。

- [ ] **Step 3: 实现 get_before**

```rust
    pub async fn get_before(&self, group_id: &str, key: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        self.ensure_index(group_id).await?;
        let index = self.indices.get(group_id).unwrap();

        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        // Current key: query_before returns lines before `line`, descending line order
        let msgs = index.query_before(key, line, remaining).await?;
        let count = msgs.len() as u32;
        if count > 0 {
            let messages: Vec<LineMessage> = msgs.into_iter()
                .rev() // reverse to ascending line order
                .map(|(l, msg)| LineMessage { line: l, message: msg })
                .collect();
            results.push(GroupedMessages {
                key: key.to_string(),
                messages,
            });
            remaining = remaining.saturating_sub(count);
        }

        // If insufficient, go to previous keys
        if remaining > 0 {
            if let Some(dates) = self.date_sets.get(group_id) {
                let mut cursor = key.to_string();
                loop {
                    if remaining == 0 { break; }
                    let prev = dates.range::<str, _>((std::ops::Bound::Unbounded, std::ops::Bound::Excluded(&cursor))).next_back();
                    match prev {
                        Some(prev_key) => {
                            let msgs = index.query_last(prev_key, remaining).await?;
                            if !msgs.is_empty() {
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .rev()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                let count = msgs.len() as u32;
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

        results.reverse(); // oldest first
        Ok(results)
    }
```

- [ ] **Step 4: 实现 get_after**

```rust
    pub async fn get_after(&self, group_id: &str, key: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        self.ensure_index(group_id).await?;
        let index = self.indices.get(group_id).unwrap();

        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        // query_after(key, line, n) returns records starting from line number (inclusive).
        // We want messages AFTER the given line, so use line+1
        let msgs = index.query_after(key, line + 1, remaining).await?;
        let count = msgs.len() as u32;
        if count > 0 {
            let messages: Vec<LineMessage> = msgs.into_iter()
                .map(|(l, msg)| LineMessage { line: l, message: msg })
                .collect();
            results.push(GroupedMessages { key: key.to_string(), messages });
            remaining = remaining.saturating_sub(count);
        }

        // Cross-key: if insufficient, go to next date keys
        if remaining > 0 {
            if let Some(dates) = self.date_sets.get(group_id) {
                let mut cursor = key.to_string();
                loop {
                    if remaining == 0 { break; }
                    let next = dates.range::<str, _>((std::ops::Bound::Excluded(&cursor), std::ops::Bound::Unbounded)).next();
                    match next {
                        Some(next_key) => {
                            let msgs = index.query_first(next_key, remaining).await?;
                            if !msgs.is_empty() {
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages { key: next_key.clone(), messages });
                                remaining = remaining.saturating_sub(msgs.len() as u32);
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
```

Wait, I made a mistake above — I wrote two versions of `get_after` in the same step. Let me clean this up in the actual plan. The correct version:

```rust
    pub async fn get_after(&self, group_id: &str, key: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>> {
        self.ensure_index(group_id).await?;
        let index = self.indices.get(group_id).unwrap();

        let mut remaining = n;
        let mut results: Vec<GroupedMessages> = Vec::new();

        // query_after(key, line, n) returns records starting from line (inclusive?)
        // Per FileIndexContext: "Get N records starting from line number"
        // We want messages AFTER the given line, so use line+1
        let msgs = index.query_after(key, line + 1, remaining).await?;
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
                    let next = dates.range::<str, _>((std::ops::Bound::Excluded(&cursor), std::ops::Bound::Unbounded)).next();
                    match next {
                        Some(next_key) => {
                            let msgs = index.query_first(next_key, remaining).await?;
                            if !msgs.is_empty() {
                                let messages: Vec<LineMessage> = msgs.into_iter()
                                    .map(|(l, msg)| LineMessage { line: l, message: msg })
                                    .collect();
                                results.push(GroupedMessages { key: next_key.clone(), messages });
                                remaining = remaining.saturating_sub(msgs.len() as u32);
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
```

- [ ] **Step 5: 实现 get_range**

```rust
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
```

- [ ] **Step 6: 编译验证**

Run: `cargo build -p kissbot-channel-web`
Expected: 编译成功

- [ ] **Step 7: 提交**

```bash
git add kissbot-channel-web/src/message_store.rs
git commit -m "feat(channel-web): MessageStore query methods with cross-key support"
```

---

### Task 5: HTTP API — 新路由与 handler

**Files:**
- Modify: `kissbot-channel-web/src/http.rs`
- Verify: cargo build

- [ ] **Step 1: 添加新路由**

在 `create_router()` 中，在路由链中添加：

```rust
        .route("/api/messages/recent", get(handle_messages_recent))
        .route("/api/messages/before", get(handle_messages_before))
        .route("/api/messages/after", get(handle_messages_after))
        .route("/api/messages/range", get(handle_messages_range))
```

同时删除不再使用的路由：
- 删除 `.route("/api/messages", get(handle_get_messages))`（注意：可能已被之前的工作移除）
- 确认没有任何旧的 `/api/messages` 路由残留

- [ ] **Step 2: 添加 handler：get_recent**

```rust
/// GET /api/messages/recent?group_id=xxx&n=20
async fn handle_messages_recent(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ApiResponse<Vec<GroupedMessages>>> {
    let group_id = match params.get("group_id") {
        Some(id) => id,
        None => return Json(ApiResponse::error("Missing group_id".to_string())),
    };
    let n: u32 = params.get("n").and_then(|v| v.parse().ok()).unwrap_or(20);
    match messenger.message_store.get_recent(group_id, n).await {
        Ok(msgs) => Json(ApiResponse::success(msgs)),
        Err(e) => Json(ApiResponse::error(e.to_string())),
    }
}
```

- [ ] **Step 3: 添加 handler：get_before**

```rust
/// GET /api/messages/before?group_id=xxx&key=2026-07-20&line=42&n=10
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
    match messenger.message_store.get_before(group_id, key, line, n).await {
        Ok(msgs) => Json(ApiResponse::success(msgs)),
        Err(e) => Json(ApiResponse::error(e.to_string())),
    }
}
```

- [ ] **Step 4: 添加 handler：get_after**

```rust
/// GET /api/messages/after?group_id=xxx&key=2026-07-20&line=42&n=10
async fn handle_messages_after(
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
    match messenger.message_store.get_after(group_id, key, line, n).await {
        Ok(msgs) => Json(ApiResponse::success(msgs)),
        Err(e) => Json(ApiResponse::error(e.to_string())),
    }
}
```

- [ ] **Step 5: 添加 handler：get_range**

```rust
/// GET /api/messages/range?group_id=xxx&start=2026-07-20T00:00:00Z&end=2026-07-20T23:59:59Z
async fn handle_messages_range(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ApiResponse<Vec<GroupedMessages>>> {
    let group_id = match params.get("group_id") {
        Some(id) => id,
        None => return Json(ApiResponse::error("Missing group_id".to_string())),
    };
    let start = match params.get("start") {
        Some(s) => s,
        None => return Json(ApiResponse::error("Missing start".to_string())),
    };
    let end = match params.get("end") {
        Some(e) => e,
        None => return Json(ApiResponse::error("Missing end".to_string())),
    };
    match messenger.message_store.get_range(group_id, start, end).await {
        Ok(msgs) => Json(ApiResponse::success(msgs)),
        Err(e) => Json(ApiResponse::error(e.to_string())),
    }
}
```

- [ ] **Step 6: 清理旧的 handler 和类型**

删除或确认以下已不存在：
- `MessageResponse` 结构体定义
- `handle_get_messages` handler
- 删除 `#[derive(Debug, Deserialize)] pub struct InitAttachmentRequest { ... }`（可能已被之前工作移除）
- 删除 `// ========== DTOs ==========` 和其后的空行（如果所有旧 DTO 已删除）

添加必要的 import：

```rust
use crate::message_store::{GroupedMessages, MessageStore};
```

- [ ] **Step 7: 编译验证**

Run: `cargo build -p kissbot-channel-web`
Expected: 编译成功

- [ ] **Step 8: 提交**

```bash
git add kissbot-channel-web/src/http.rs
git commit -m "feat(channel-web): add message query API endpoints"
```

---

### Task 6: 集成 — MessageStore 创建与 WebMessenger 装配

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs` (WebMessengerCreator)
- Modify: `kissbot-channel-web/src/main.rs`
- Verify: cargo build

- [ ] **Step 1: 更新 WebMessengerCreator::create()**

在 `messenger.rs` 的 `WebMessengerCreator::create()` 方法中（第 541-563 行），修改为创建 MessageStore 并传入 WebMessenger：

```rust
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
    ) -> std::result::Result<Arc<WebMessenger>, kissbot_channel::Error> {
        let mid = self.config.read().await.messenger_id.clone();

        // MessageStore base dir: parent of attachment_dir / "messages"
        let messages_base = Path::new(&self.attachment_dir)
            .parent()
            .map(|p| p.join("messages"))
            .unwrap_or_else(|| PathBuf::from("data/messages"));
        let message_store = MessageStore::new(messages_base, mid.to_string());

        let messenger = Arc::new(WebMessenger::new(
            mid,
            self.repo_path.clone(),
            self.config.clone(),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            on_user_remove,
            &self.attachment_dir,
            message_store,
        ));

        Ok(messenger)
    }
```

需要添加 use：`use std::path::Path;`（如果没有）和 `use crate::message_store::MessageStore;`

- [ ] **Step 2: 编译验证**

Run: `cargo build -p kissbot-channel-web`
Expected: 编译成功

- [ ] **Step 3: 提交**

```bash
git add kissbot-channel-web/src/messenger.rs
git commit -m "feat(channel-web): integrate MessageStore into WebMessenger"
```

---

### Task 7: 最终清理与全面编译

**Files:**
- Modify: `kissbot-channel-web/src/http.rs`
- Modify: `kissbot-channel-web/src/messenger.rs`
- Verify: cargo build (full workspace)

- [ ] **Step 1: 检查并清理所有无用的 import**

在 `http.rs` 中：
- 确认 `MessageResponse`、`AttachmentRefResponse`、`handle_get_messages`、`handle_init_attachment` 已删除
- 检查 import 中是否有 `Serialize`、`Deserialize`、`dashmap::DashMap`、`flume` 等仍然在使用的
- 确认 `use crate::messenger::ADMIN_USER_ID` — 是否仍被 `http.rs` 中其他 handler 使用（如 attachment 相关 handler），如有保留，否则移除

在 `messenger.rs` 中：
- 确认 `SsePayload`、`SseMessage` 已删除
- 确认 `use serde::Serialize;` 仍被其他代码使用（如 `WebMessengerRepo` 等），否则移除
- 检查 `use dashmap::DashMap;` 是否仍被其他代码使用

- [ ] **Step 2: 全 workspace 编译**

Run: `cargo build -p kissbot-channel-web` — 无 error
Run: `cargo build` — 整个 workspace 编译通过

- [ ] **Step 3: 最终提交**

```bash
git add -A
git commit -m "chore(channel-web): cleanup unused imports and types"
```
