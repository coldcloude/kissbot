# 使用 FileObjectAppender 重写 record.rs 设计文档

## 背景

`kissbot-memory-store/src/record.rs` 当前包含 844 行代码，核心是 `RecordContext<Q,K,R,P>` 结构体，负责 4 种记录类型（channel、think、tool_call、tool_result）的 JSONL 文件写入。它自行管理 per-key 锁、文件状态、SN 分配、顺序检查和 force 重写逻辑。

`kai-file` 的 `FileObjectAppender` 提供了通用的异步缓冲写入管道 + per-key 锁机制。通过将 record.rs 的写入逻辑重构为基于 `FileObjectAppender` 的架构，可以消除重复的锁管理和缓冲逻辑。

## 架构变化

### 旧架构

```
RecordManager
  └─ RecordContext<Q, K, R, P>
       ├─ DashMap<K, FileLock>          ← 自实现的 per-key 锁
       ├─ append_record(Vec<Q>, force, hook)
       │    ├─ 解析请求 → 分组 → 排序
       │    ├─ 获取锁 → 加载文件状态
       │    ├─ 分配 SN → 检查顺序
       │    ├─ 追加或 force 重写
       │    └─ 触发 FileHook
       └─ parser: P
```

### 新架构

```
RecordManager
  ├─ RecordAppendWriter<K,R,P,H>        ← 实现 FileAppendWriter
  │    └─ DashMap<K, Arc<Mutex<Context>>>
  │         └─ RecordWriterContext<K,R,P,H>
  │              ├─ state: Option<FileState>
  │              ├─ parser: P
  │              └─ hook: H
  │
  └─ FileObjectAppender<K,R,W,C,E>      ← 缓冲 + flush
       └─ 调用 W.get_lock() → lock → C.write()
```

## 核心类型

### `FileState`（保留）

```rust
pub(crate) struct FileState {
    pub sn: u64,
    pub time: Arc<String>,
}
```

### `RecordWriterContext<K, R, P, H>`

实现 `FileAppendWriterContext<K, R>`，是每个 key 的写入上下文：

- `state: Option<FileState>` — 文件状态缓存（sn + latest time），首次写入时从磁盘恢复
- `parser: P` — 实现 `FilePathGenerator<K>`，用于获取文件路径
- `hook: H` — 实现 `FileHook<K>`，写入完成后触发索引更新

`write(&mut self, key, records)` 执行实际文件 I/O：

1. 通过 `self.parser.get_path(key)` 获取文件路径
2. 加载文件状态（首次或缓存丢失时通过 `ReverseLineReader` 恢复）
3. 为记录分配连续 SN：`records[i].set_sn(state.sn + 1 + i)`
4. 按 time 排序
5. 如果 `state.time > records[0].time()`：
   - 全量重写：读取文件中所有记录 → 合并 → 排序 → 截断重写
   - 调用 `self.hook.on_force_append(key)`
6. 否则：
   - 追加到文件末尾
   - 调用 `self.hook.on_append(key)`
7. 更新 `self.state`

### `RecordAppendWriter<K, R, P, H>`

实现 `FileAppendWriter<K, R, RecordWriterContext<K, R, P, H>>`：

- `map: DashMap<K, Arc<Mutex<RecordWriterContext<K, R, P, H>>>>` — per-key 锁 + 上下文
- `get_lock(key)` — 返回 `Arc<Mutex<Context>>`，不存在时创建（state=None）
- `remove_lock(key)` — 清理 map 条目

## 写入流程

### Stage 1：同步预检（在 `RecordManager.append_*` 中）

```
输入: Vec<Q>, force: bool
  │
  ├─ 解析请求: Q → (K, R) 分组 → HashMap<K, Vec<R>>
  ├─ 每组按 time 排序
  ├─ 对每个 key:
  │   ├─ writer.get_lock(key) → Arc<Mutex<Context>>
  │   ├─ lock().await → 读取 ctx.state
  │   ├─ 若 state.time > records[0].time() && !force
  │   │   └─ return Err(RecordNotInOrder)   ← 同步返回给 HTTP
  │   └─ 释放锁
  └─ 对每个 key: appender.append(key, records)  ← 进入异步管道
     └─ return Ok(())
```

### Stage 2：异步写入（在 `RecordWriterContext::write` 中）

```
flush task → get_lock(key) → lock().await → ctx.write(key, batch)
                                               │
  ctx.write:                                   │
    ├─ parser.get_path(key) → file_path
    ├─ state.take() ?? load_existing_file_state(file_path)
    ├─ 分配 SN + 排序
    ├─ if state.time > records[0].time():
    │   ├─ 全量重写（读 + 合并 + 排序 + 写回）
    │   └─ hook.on_force_append(key)
    ├─ else:
    │   ├─ 追加文件
    │   └─ hook.on_append(key)
    └─ state = Some(updated_state)
```

## 并发安全

- `get_lock(key)` 对同一个 key 返回同一把 `Arc<Mutex<Context>>`
- Stage 1 预检和 Stage 2 flush 通过同一把锁串行化
- Stage 1 预检是"尽力而为"的快速路径，竞态下（预检通过后 flush 前状态变化）由 Stage 2 的 `write` 自动降级为全量重写，结果正确

## RecordManager 结构

```rust
pub struct RecordManager {
    channel_writer: Arc<RecordAppendWriter<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>>,
    think_writer: Arc<RecordAppendWriter<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>>,
    tool_call_writer: Arc<RecordAppendWriter<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>>,
    tool_result_writer: Arc<RecordAppendWriter<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>>,

    channel_appender: FileObjectAppender<ChannelRecordKey, ChannelRecord, ...>,
    think_appender: FileObjectAppender<RecordKey, ThinkRecord, ...>,
    tool_call_appender: FileObjectAppender<RecordKey, ToolCallRecord, ...>,
    tool_result_appender: FileObjectAppender<RecordKey, ToolResultRecord, ...>,
}
```

构造方式：每个 `RecordAppendWriter` 先 `Arc::new(...)`，再 `.clone()` 传入 `FileObjectAppender::new(writer.clone(), error_handler, timeout, batch_size)`。

### 公开 API（保持不变）

```rust
pub fn new() -> Self
pub fn get() -> &'static Self
pub async fn append_channel_record(&self, requests: Vec<ChannelRequest>, force: bool) -> Result<()>
pub async fn append_think_record(&self, requests: Vec<ThinkRequest>, force: bool) -> Result<()>
pub async fn append_tool_call_record(&self, requests: Vec<ToolCallRequest>, force: bool) -> Result<()>
pub async fn append_tool_result_record(&self, requests: Vec<ToolResultRequest>, force: bool) -> Result<()>
```

## 保留的辅助函数

- `FileState` — 原样保留
- `load_existing_file_state` — 原样保留，通过 `ReverseLineReader` 恢复文件状态
- `write_records_to_file` — 精简为仅做序列化 + state 更新，文件打开交给调用方

## 错误处理

### Stage 1 错误
- `RecordNotInOrder` — 同步返回，HTTP 收到错误响应
- 其他解析错误 — 同步返回

### Stage 2 错误（通过 `ErrorHandler`）
```rust
pub(crate) struct LogErrorHandler;
impl<K: Debug, R> ErrorHandler<K, R> for LogErrorHandler {
    async fn on_write_error(&self, key: &K, _batch: Vec<R>, error: &Error) {
        log::error!("[memory-store] write error for key={:?}: {}", key, error);
    }
}
```

## 删除的旧代码

- `RecordContext<Q, K, R, P>` 结构体及 impl
- `FileLock` 类型别名 (`Arc<Mutex<Option<FileState>>>`)
- `get_lock` 辅助函数
- `_marker: PhantomData<(Q,R)>` 字段（不再需要 Q 泛型）

## 测试影响

现有测试覆盖：
- 文件状态加载（存在/不存在/空文件）
- 顺序追加（单条/多条/跨调用）
- 4 种记录类型各自的写入
- 乱序拒绝（`RecordNotInOrder` 错误）
- force 重写（插入更早记录 + 全量重排序）
- 多 key 隔离

需要适配：
- `RecordContext` 改 `RecordAppendWriter` + `FileObjectAppender`
- `NoopFileHook` 保留不变
- `append_record` 调用改为通过 appender
- 异步 flush 需要 `tokio::time::sleep` 等待写入完成

测试充分性：覆盖所有原有场景，新增 LogErrorHandler 的异步特性不影响测试逻辑。
