# MessageStore 重构设计：统一 FileIndexContext + FileObjectAppender

## 概述

重构 MessageStore，将 per-group 的 `FileIndexContext` 合并为单个实例，key 扩展为 `(messenger_id, group_id, date)` 三元组；写入侧替换 flume 队列为 `kai-file` 的 `FileObjectAppender`，通过 `Weak<FileIndexContext>` 在写入完成后通知索引更新。

## 动机

1. **统一索引管理**：所有 group 共享同一个 `FileIndexContext`，key 自包含全部定位信息
2. **标准化写入**：`FileObjectAppender` 提供缓冲批量写入、超时刷新、错误处理，替代手写 flume 队列
3. **消除冗余字段**：`messenger_id` 无需单独存储在 MessageStore，由 key 自携带

## 影响范围

- `kissbot-channel-web/src/message_store.rs` — 主要重构
- `kissbot-channel-web/src/http.rs` — handler 签名适配
- `kissbot-channel-web/src/messenger.rs` — `WebMessengerCreator` 中 MessageStore 构造调用适配

---

## 一、MsqKey 结构体

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MsgKey {
    pub messenger_id: String,
    pub group_id: String,
    pub date: String,
}
```

- `Ord` 按字段声明顺序比较 `(messenger_id, group_id, date)`，与 tuple 行为一致
- 用于 `FileIndexContext` 的 `K` 类型参数和 `FileObjectAppender` 的 `K` 类型参数

## 二、TimeRangeQuery

```rust
pub struct TimeRangeQuery {
    pub messenger_id: String,
    pub group_id: String,
    pub start: String,
    pub end: String,
}
```

- 用于 `FileIndexContext` 的 `Q` 类型参数
- `QueryParser` 将其展开为 `Vec<(MsgKey, (String, String))>` — 为所有匹配的 `date` key 生成 `(start_time, end_time)` 段

## 三、date_sets 索引

跨 key 查询需要知道某个 `(messenger_id, group_id)` 下有哪些 date 文件存在。用一个共享的 `Arc<DashMap<(String, String), BTreeSet<String>>>` 维护：

- key = `(messenger_id, group_id)` 二元组
- value = 已写入的 date 集合（如 `{"2026-07-19", "2026-07-20"}`）

写入时由 writer 更新，查询时由 `get_recent/get_before/get_after` 读取导航。

## 四、MessageFileWriter

```rust
struct MessageFileWriter {
    base_dir: PathBuf,
    index: Weak<FileIndexContext<TimeRangeQuery, MsgKey, IncomingMessage, MessageParser>>,
    date_sets: Weak<DashMap<(String, String), BTreeSet<String>>>,
}

impl MessageFileWriter {
    fn new(
        base_dir: PathBuf,
        index: Weak<...>,
        date_sets: Weak<DashMap<(String, String), BTreeSet<String>>>,
    ) -> Self
}

#[async_trait]
impl FileAppendWriter<MsgKey, IncomingMessage> for MessageFileWriter {
    async fn write(&self, key: &MsgKey, records: Vec<IncomingMessage>) -> Result<()> {
        // 1. 拼路径: {base_dir}/{messenger_id}/{group_id}/{date}.jsonl
        // 2. 确保目录存在
        // 3. 追加每条记录为 JSONL 行
        // 4. upgrade index → mark_obsolete(key)
        // 5. upgrade date_sets → 插入 date
    }
}
```

- 使用 `Weak` 避免循环引用（`MessageStore` 持有 `Arc<index>`，`writer` 持有 `Weak<index>`）
- `upgrade()` 返回 `None` 时跳过（store 已析构，忽略）

## 五、MessageStore 结构

```rust
pub struct MessageStore {
    appender: FileObjectAppender<MsgKey, IncomingMessage, MessageFileWriter>,
    index: Arc<FileIndexContext<TimeRangeQuery, MsgKey, IncomingMessage, MessageParser>>,
    date_sets: Arc<DashMap<(String, String), BTreeSet<String>>>,
    // key = (messenger_id, group_id)
    // value = sorted dates for that group
}
```

### 构造

```rust
impl MessageStore {
    pub fn new(base_dir: PathBuf) -> Arc<Self> {
        let index = Arc::new(FileIndexContext::new(MessageParser::new(base_dir.clone())));
        let date_sets: Arc<DashMap<(String, String), BTreeSet<String>>> = Arc::new(DashMap::new());
        let writer = MessageFileWriter::new(
            base_dir,
            Arc::downgrade(&index),
            Arc::downgrade(&date_sets),
        );
        let appender = FileObjectAppender::new(
            writer,
            NoopErrorHandler,
            Duration::from_secs(5),   // timeout
            100,                        // batch_size
        );
        Arc::new(Self { appender, index, date_sets })
    }
}
```

`date_sets` 通过 `Arc` 在 `MessageStore` 和 `MessageFileWriter` 之间共享，writer 通过 `Weak` 持有防止循环引用。

### 写入

```rust
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

`append` 返回 `Result`，当前忽略错误（与之前 flume 版本行为一致）。

### 查询

```rust
pub async fn get_recent(&self, messenger_id: &str, group_id: &str, n: u32) -> Result<Vec<GroupedMessages>>
pub async fn get_before(&self, messenger_id: &str, group_id: &str, key_date: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>>
pub async fn get_after(&self, messenger_id: &str, group_id: &str, key_date: &str, line: u32, n: u32) -> Result<Vec<GroupedMessages>>
pub async fn get_range(&self, messenger_id: &str, group_id: &str, start: &str, end: &str) -> Result<Vec<GroupedMessages>>
```

- 构建 `MsgKey` 时 `messenger_id` 和 `group_id` 从参数传入
- 跨 key 查询基于 `date_sets.get(&(messenger_id.to_string(), group_id.to_string()))` 导航

## 六、MessageParser

```rust
struct MessageParser {
    base_dir: PathBuf,
}
```

实现 `FilePathGenerator<MsgKey>` 和 `QueryParser<TimeRangeQuery, MsgKey>`：

- `get_path(key)`: `{base_dir}/{key.messenger_id}/{key.group_id}/{key.date}.jsonl`
- `parse_query(query)`: 根据 `query.messenger_id + query.group_id` 和 `date` 范围展开所有匹配的 `MsgKey`

## 七、HTTP 适配

handler 从 `messenger.messenger_id` 获取值传入 MessageStore 查询方法：

```rust
async fn handle_messages_recent(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ApiResponse<Vec<GroupedMessages>>> {
    let group_id = params.get("group_id")...;
    let n = ...;
    match messenger.message_store.get_recent(&messenger.messenger_id, group_id, n).await {
        ...
    }
}
```

`WebMessengerCreator::create()` 中构造 MessageStore：

```rust
let messages_base = Path::new(&self.attachment_dir)
    .parent()
    .map(|p| p.join("messages"))
    .unwrap_or_else(|| PathBuf::from("data/messages"));
let message_store = MessageStore::new(messages_base);
```

不再传入 `messenger_id`（由 key 自携带）。
