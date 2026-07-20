# channel-web 消息存储与 SSE 重构设计

## 概述

在 channel-web 中实现消息本地文件存储，替换 SSE 和消息查询中的 DTO 类型为统一的 `IncomingMessage`，并提供基于 `FileIndexContext` 的消息查询 API。

## 动机

1. **统一的类型模型**：删除 `SseMessage`、`MessageResponse`、`AttachmentRefResponse`，前后端一致使用 `IncomingMessage`
2. **本地消息历史**：当前 `GET /api/messages` 返回空，消息查询完全依赖外部 memory store；改为本地 JSONL 文件存储，提供最近消息、翻页、时间范围查询
3. **基于 kai-file 索引**：利用 `FileIndexContext` 实现高性能行级定位和时间范围查询
4. **简化 SSE 推送**：去掉按 group 分发的逻辑，改为全局 broadcast

## 影响范围

- `kissbot-api` — 新增 `Record for IncomingMessage` 实现
- `kissbot-channel-web` — 主要改动：SSE 层重写、新增 MessageStore、HTTP API 变更
- `kai-file` — 无改动（仅新增外部依赖引用）

---

## 一、SSE 重构（kissbot-channel-web）

### 1.1 删除类型

删除 `messenger.rs` 中的以下类型：
- `SsePayload<'a>`
- `SseMessage`

### 1.2 SseDispatcher 改造

```rust
pub struct SseDispatcher {
    senders: Arc<Mutex<HashMap<Uuid, flume::Sender<String>>>>,
}
```

- `register() -> flume::Receiver<String>`：生成新 UUID，创建 `flume::unbounded` channel，sender 存入 `senders`，返回 receiver
- `push(data: &str)`：遍历所有 sender，逐个 `try_send()`，失败（disconnected）的 sender 从 map 中移除
- 不再需要 group_id 参数

### 1.3 SSE 路由

`GET /api/events`：不再按 group_id 注册多个 receiver，改为：

```rust
async fn handle_sse_events(
    State(messenger): State<Arc<WebMessenger>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = messenger.sse.register();
    let stream = rx.into_stream().map(|data| Ok(Event::default().data(data)));
    Sse::new(stream).keep_alive(...)
}
```

### 1.4 send() 中的 SSE 推送

原有的 SSE 推送位置不变（在循环中为每组成员创建 `IncomingMessage` 后），但改为推送 `IncomingMessage` 的 JSON 序列化，而非 `SseMessage`。

同时构造一份 admin 视角的 `IncomingMessage`（`is_self=1`），推送到 MessageStore 的写入队列。

---

## 二、Record 实现（kissbot-api）

### 2.1 新增依赖

`kissbot-api/Cargo.toml` 新增：
```toml
kai-file = { path = "../kai-rs/kai-file" }
```

### 2.2 Record 实现

在 `kissbot-api/src/channel.rs` 中：

```rust
impl Record for IncomingMessage {
    fn time(&self) -> &str {
        self.time.as_str()
    }
}
```

---

## 三、MessageStore（kissbot-channel-web）

### 3.1 目录结构

```
data/{messenger_id}/messages/{group_id}/{date}.jsonl
```

每行一条 `IncomingMessage` JSON。

### 3.2 返回类型

```rust
/// 单条消息及其位置
pub struct LineMessage {
    pub line: u32,
    pub message: IncomingMessage,
}

/// 同一日期文件中的消息组
pub struct GroupedMessages {
    pub key: String,
    pub messages: Vec<LineMessage>,
}
```

### 3.3 写入队列

- `flume::unbounded<IncomingMessage>` 队列
- `send()` 中 SSE 推送后将 admin 视角的 `IncomingMessage` 推入队列（非阻塞）
- 队列消费者由 `WebMessenger::start()` 启动为单 tokio task，串行处理

写入流程：
```
1. 从 msg.time 解析日期 date_key
2. 以 append 模式打开 {group_dir}/{date_key}.jsonl
3. 写入一行 JSON
4. 更新 date_set: DashMap<String, BTreeSet<String>> (group_id -> sorted_dates)
5. index.mark_obsolete(date_key)
```

### 3.4 FileIndexContext 配置

```rust
type DateKey = String;  // "2026-07-20"
type GroupIndex = FileIndexContext<TimeRangeQuery, DateKey, IncomingMessage, GroupParser>;

struct GroupParser {
    base_dir: PathBuf,
    group_id: String,
}
```

- `FilePathGenerator`：`get_path(date_key) -> {base_dir}/{group_id}/{date_key}.jsonl`
- `QueryParser` 类型：`TimeRangeQuery { start: String, end: String }`，将时间范围展开为 `Vec<(date_key, start_time, end_time)>`
- `message_id` 索引：不需要，SSE 推送的消息不用于查询

### 3.5 索引初始化

延迟加载 — 首次查询某 group 时：

```rust
async fn ensure_index(group_id) {
    indices.entry(group_id).or_insert_with(|| {
        let index = GroupIndex::new(GroupParser::new(base_dir, group_id));
        // 遍历 date_set 中所有已知 date_key，标记重建
        if let Some(dates) = date_sets.get(group_id) {
            for date in dates.iter() {
                index.mark_all_obsolete(date.clone());
            }
        }
        index
    });
}
```

### 3.6 查询 API

```rust
async fn get_recent(group_id, n: u32) -> Result<Vec<GroupedMessages>>
```

1. 取 date_set 中该 group 的最大 date_key
2. `index.query_last(date_key, n)` 得 `Vec<(line, IncomingMessage)>`
3. 不足 n 条 → 前一个 date_key，`query_last` 补足，递归

```rust
async fn get_before(group_id, key, line, n: u32) -> Result<Vec<GroupedMessages>>
```

1. `index.query_before(key, line, n)` 得按行号降序的结果
2. 不足 n 条 → 前一个 date_key，`query_last` 补足
3. 最终各组内部按行号升序排列

```rust
async fn get_after(group_id, key, line, n: u32) -> Result<Vec<GroupedMessages>>
```

1. `index.query_after(key, line, n)` 得按行号升序的结果
2. 不足 n 条 → 后一个 date_key，`query_first` 补足

```rust
async fn get_range(group_id, start, end) -> Result<Vec<GroupedMessages>>
```

1. `index.query_all(TimeRangeQuery { start, end })` — 自动计算覆盖的 date_key
2. 各组内按行号升序排列

---

## 四、HTTP API（kissbot-channel-web）

### 4.1 新增路由

| 方法 | 路径 | 参数 | 功能 |
|------|------|------|------|
| GET | `/api/messages/recent` | `group_id, n` | 最近 n 条消息 |
| GET | `/api/messages/before` | `group_id, key, line, n` | 从 key+line 向前取 n 条 |
| GET | `/api/messages/after` | `group_id, key, line, n` | 从 key+line 向后取 n 条 |
| GET | `/api/messages/range` | `group_id, start, end` | 时间范围查询 |

### 4.2 删除内容

- 删除 `MessageResponse`、`AttachmentRefResponse`、`InitAttachmentRequest` 结构体
- 删除 `GET /api/messages` 路由及 handler
- 删除 `POST /api/attachment/init` 路由及 handler（功能已由 `handle_send_message` 覆盖）

### 4.3 返回格式

所有查询接口统一返回 `Json<ApiResponse<Vec<GroupedMessages>>>`。

---

## 五、跨 key 查询规则

1. 按 `date_set` 中排序的日期确定前/后 key：`date_set.iter().rev()` 从最新开始
2. 跨 key 时，新组中的消息从文件开头/末尾取起：
   - `get_recent`、`get_before` 跨 key：上一个 key，取 `query_last`
   - `get_after` 跨 key：下一个 key，取 `query_first`
3. 不跨 key 的临界处理：`query_before` 本身结果反向，需反转回升序
4. `get_range` 由 `FileIndexContext.query_all` 内部跨 key
