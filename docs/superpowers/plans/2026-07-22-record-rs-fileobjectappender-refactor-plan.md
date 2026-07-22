# record.rs 使用 FileObjectAppender 重构 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `kissbot-memory-store/src/record.rs` 的写入逻辑重构为基于 `kai-file` 的 `FileObjectAppender` + `FileAppendWriter` + `FileAppendWriterContext` 架构。

**Architecture:** 两段式写入——Stage 1（同步预检）解析请求、分组排序、检查顺序；Stage 2（异步写入）通过 FileObjectAppender 缓冲后调用 FileAppendWriterContext::write 执行实际文件 I/O 和 FileHook。

**Tech Stack:** Rust, tokio, kai-file (FileObjectAppender), serde_json

## Global Constraints

- 不删除代码中的注释
- 保留 `FileState`、`load_existing_file_state`、`write_records_to_file` 等辅助函数
- 4 种记录类型使用 4 个独立的 FileObjectAppender
- Stage 1 预检使用 writer.get_lock() 获取锁保证并发安全
- 新增 `LogErrorHandler` 使用 `eprintln!` 记录（不引入新依赖）
- 公开 API 签名不变

---

### Task 1: 更新 kai-file 导出

**Files:**
- Modify: `kai-rs/kai-file/src/lib.rs:11`

**Interfaces:**
- Consumes: 已有 `FileAppendWriterContext` trait（在 `appender.rs` 中定义）
- Produces: 将 `FileAppendWriterContext` 加入 pub use 导出

- [ ] **Step 1: 添加 FileAppendWriterContext 到导出列表**

```rust
// 第 11 行修改
pub use appender::{FileAppendWriter, FileAppendWriterContext, ErrorHandler, NoopErrorHandler, FileObjectAppender};
```

- [ ] **Step 2: 编译验证**

```bash
cd /home/admin/project/kissbot/kai-rs/kai-file && cargo check 2>&1
```
Expected: 编译成功，无警告。

- [ ] **Step 3: 提交**

```bash
git add kai-rs/kai-file/src/lib.rs
git commit -m "feat(kai-file): 导出 FileAppendWriterContext trait

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: 实现 RecordWriterContext 和 RecordAppendWriter

**Files:**
- Modify: `kissbot-memory-store/src/record.rs`（主体重写）

**Interfaces:**
- Consumes（从现有代码保留）:
  - `FileState` struct
  - `load_existing_file_state` 函数
  - `write_records_to_file` 函数（精简：去掉 file.open，只做序列化 + state 更新）
  - `FileHook<K>` trait（来自 `kissbot_memory::data`）
  - `FilePathGenerator<K>` trait（来自 `kai_file::index`）
  - 4 个 Hook 实现（`ChannelFileIndexHook` 等）
- Produces:
  - `RecordWriterContext<K, R, P, H>` — 实现 `FileAppendWriterContext<K, R>`
  - `RecordAppendWriter<K, R, P, H>` — 实现 `FileAppendWriter<K, R, RecordWriterContext<K, R, P, H>>`

- [ ] **Step 1: 确认现有 record.rs 结构**

读取当前 `kissbot-memory-store/src/record.rs` 文件，确认要保留的代码段：
- `FileState`（第 17-20 行）
- `write_records_to_file`（第 22-31 行）
- `load_existing_file_state`（第 41-70 行）
- 4 个 `FileHook` 实现（第 190-236 行）
- `RecordManager` 结构 + impl（第 238-276 行）
- 测试模块（第 278-844 行）— 这些会在后续任务中适配

需要删除：
- `FileLock` 类型别名（第 33 行）
- `get_lock` 函数（第 35-39 行）
- `RecordContext<Q, K, R, P>` 整个结构及 impl（第 72-188 行）

需要新增：
- `RecordWriterContext<K, R, P, H>` 结构 + `FileAppendWriterContext` impl
- `RecordAppendWriter<K, R, P, H>` 结构 + `FileAppendWriter` impl
- `LogErrorHandler`

- [ ] **Step 2: 删除旧 RecordContext 相关代码**

从 record.rs 中删除：
1. `use std::collections::HashMap;` — 移到 RecordManager 使用处
2. `FileLock` 类型别名
3. `get_lock` 函数
4. `RecordContext<Q,K,R,P>` 结构体（第 72-76 行）
5. `impl<Q,K,R,P> RecordContext<Q,K,R,P>` 块（第 78-188 行）

保留：
- 所有 `use` 导入（后面可能需要调整）
- `FileState`、`write_records_to_file`、`load_existing_file_state`
- 4 个 Hook 结构体 + impl

- [ ] **Step 3: 添加 RecordWriterContext**

在删除 `RecordContext` 的位置（原第 72 行附近）添加：

```rust
use kai_file::{FileAppendWriter, FileAppendWriterContext, ErrorHandler, FileObjectAppender};

pub(crate) struct RecordWriterContext<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
{
    state: Option<FileState>,
    parser: P,
    hook: H,
    _phantom: PhantomData<(K, R)>,
}

impl<K, R, P, H> RecordWriterContext<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: MemoryRecord,
    P: FilePathGenerator<K>,
    H: FileHook<K>,
{
    pub fn new(parser: P, hook: H) -> Self {
        Self {
            state: None,
            parser,
            hook,
            _phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<K, R, P, H> FileAppendWriterContext<K, R> for RecordWriterContext<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    R: MemoryRecord + 'static,
    P: FilePathGenerator<K> + Send + Sync + 'static,
    H: FileHook<K> + Send + Sync + 'static,
{
    async fn write(&mut self, key: &K, records: Vec<R>) -> kai_file::Result<()> {
        let file_path = self.parser.get_path(key).await?;

        let file_state = self.state.get_or_insert(
            load_existing_file_state(&file_path).await
                .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?
        );

        // 分配 SN
        let mut records = records;
        for (i, record) in records.iter_mut().enumerate() {
            record.set_sn(file_state.sn + 1 + i as u64);
        }

        // 按 time 排序（相同 time 按 sn）
        records.sort_by(|a, b| a.cmp(b));

        if file_state.time.as_str() > records[0].time() {
            // 乱序 → 全量重写
            let mut all_records: Vec<R> = Vec::new();
            let file = tokio::fs::File::open(&file_path).await?;
            let reader = tokio::io::BufReader::new(file);
            let mut lines = reader.lines();
            while let Some(line) = lines.next_line().await? {
                let record: R = serde_json::from_str(&line)?;
                all_records.push(record);
            }
            all_records.extend(records);
            all_records.sort_by(|a, b| a.cmp(b));

            let mut file = tokio::fs::OpenOptions::new()
                .create(true).write(true).open(&file_path).await?;

            file_state.sn = 0;
            for record in &mut all_records {
                file_state.sn += 1;
                record.set_sn(file_state.sn);
                let line = serde_json::to_string(record)? + "\n";
                tokio::io::AsyncWriteExt::write_all(&mut file, line.as_bytes()).await?;
            }
            if let Some(last) = all_records.last() {
                file_state.time = last.time_string();
            }

            self.hook.on_force_append(key);
        } else {
            // 有序 → 追加
            let mut file = tokio::fs::OpenOptions::new()
                .create(true).append(true).open(&file_path).await?;

            for record in &mut records {
                file_state.sn += 1;
                record.set_sn(file_state.sn);
                let line = serde_json::to_string(record)? + "\n";
                tokio::io::AsyncWriteExt::write_all(&mut file, line.as_bytes()).await?;
                file_state.time = record.time_string();
            }

            self.hook.on_append(key);
        }

        Ok(())
    }
}
```

注意：需要在文件顶部添加 `use std::sync::Arc;`（如果还没有）和 `use kai_file::{...}`。

- [ ] **Step 4: 添加 RecordAppendWriter**

在 `RecordWriterContext` 之后添加：

```rust
pub(crate) struct RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
{
    map: DashMap<K, Arc<Mutex<RecordWriterContext<K, R, P, H>>>>,
    _phantom: PhantomData<(R,)>,
}

impl<K, R, P, H> RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
    R: MemoryRecord,
    P: FilePathGenerator<K>,
    H: FileHook<K>,
{
    pub fn new(parser: P, hook: H) -> Self {
        Self {
            map: DashMap::new(),
            _phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<K, R, P, H> FileAppendWriter<K, R, RecordWriterContext<K, R, P, H>> for RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    R: MemoryRecord + 'static,
    P: FilePathGenerator<K> + Send + Sync + 'static,
    H: FileHook<K> + Send + Sync + 'static,
{
    async fn get_lock(&self, key: &K) -> Arc<Mutex<RecordWriterContext<K, R, P, H>>> {
        self.map.entry(key.clone()).or_insert_with(|| {
            Arc::new(Mutex::new(RecordWriterContext::new(
                P: ??? // 问题：这里需要 parser，但 parser 不在 get_lock 的参数中
            )))
        }).clone()
    }

    async fn remove_lock(&self, key: &K) {
        // 只有当 Arc 的强引用数为 1（仅 map 持有）时才清理
        if let Some(entry) = self.map.get(key) {
            if Arc::strong_count(entry.value()) == 1 {
                drop(entry);
                self.map.remove(key);
            }
        }
    }
}
```

问题：`get_lock` 需要创建新的 `RecordWriterContext`，但 `parser` 和 `hook` 需要从外部传入。当前的 `FileAppendWriter` trait 签名是 `get_lock(&self, key: &K)`，没有额外的参数。

**解决方案**：`RecordAppendWriter` 自身持有 `parser` 和 `hook` 的副本：

```rust
pub(crate) struct RecordAppendWriter<K, R, P, H>
where
    K: Eq + Hash + Clone + Send + Sync,
{
    map: DashMap<K, Arc<Mutex<RecordWriterContext<K, R, P, H>>>>,
    parser: P,
    hook: H,
    _phantom: PhantomData<(R,)>,
}
```

并在 `get_lock` 中使用 `self.parser` 和 `self.hook` 创建新 context：

```rust
async fn get_lock(&self, key: &K) -> Arc<Mutex<RecordWriterContext<K, R, P, H>>> {
    self.map.entry(key.clone()).or_insert_with(|| {
        Arc::new(Mutex::new(RecordWriterContext::new(
            ... // 但或怎么办？parser 和 hook 需要 move 进闭包
        )))
    }).clone()
}
```

问题：`or_insert_with` 闭包不能 borrow `self.parser`（因为 `&self` 的 borrow 和 map 的 entry borrow 冲突）。

**最终方案**：使用 `entry()` + `or_insert_with` 但先 clone parser/hook：

```rust
async fn get_lock(&self, key: &K) -> Arc<Mutex<RecordWriterContext<K, R, P, H>>>
where
    P: Clone,
    H: Clone,
{
    match self.map.entry(key.clone()) {
        dashmap::Entry::Occupied(entry) => entry.get().clone(),
        dashmap::Entry::Vacant(entry) => {
            let ctx = Arc::new(Mutex::new(
                RecordWriterContext::new(self.parser.clone(), self.hook.clone())
            ));
            entry.insert(ctx.clone());
            ctx
        }
    }
}
```

这要求 `P: Clone` 和 `H: Clone`。所有 parser（`ChannelParser`、`ThinkParser` 等）都是空结构体（自动 Clone）。Hook 也是空结构体（可以实现 Clone）。

为所有 Hook 添加 Clone：

```rust
#[derive(Clone)]
struct ChannelFileIndexHook;
#[derive(Clone)]
struct ThinkFileIndexHook;
#[derive(Clone)]
struct ToolCallFileIndexHook;
#[derive(Clone)]
struct ToolResultFileIndexHook;
```

- [ ] **Step 5: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-store && cargo check 2>&1
```
Expected: 编译成功（测试除外）。

- [ ] **Step 6: 提交**

```bash
git add kissbot-memory-store/src/record.rs
git commit -m "refactor(memory-store): 添加 RecordWriterContext 和 RecordAppendWriter

- 删除旧 RecordContext<Q,K,R,P>
- 新增 RecordWriterContext<K,R,P,H> 实现 FileAppendWriterContext
- 新增 RecordAppendWriter<K,R,P,H> 实现 FileAppendWriter
- 保留 FileState、load_existing_file_state、write_records_to_file
- 为 FileHook 实现添加 Clone derive

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: 实现 LogErrorHandler 并重写 RecordManager

**Files:**
- Modify: `kissbot-memory-store/src/record.rs`

**Interfaces:**
- Consumes: `RecordWriterContext`, `RecordAppendWriter`（来自 Task 2）, `FileObjectAppender`
- Produces: 重写的 `RecordManager`（4 个 writer + 4 个 appender），Stage 1 预检逻辑

- [ ] **Step 1: 添加 LogErrorHandler**

在 record.rs 中（Hook 实现之后）添加：

```rust
pub(crate) struct LogErrorHandler;

#[async_trait]
impl<K: std::fmt::Debug + Send + Sync + 'static, R: Send + Sync + 'static>
    ErrorHandler<K, R> for LogErrorHandler
{
    async fn on_write_error(&self, key: &K, _batch: Vec<R>, error: &kai_file::Error) {
        eprintln!("[memory-store] write error for key={:?}: {}", key, error);
    }
}

// NoopFileHook 保留给测试使用
#[cfg(test)]
pub(crate) struct NoopFileHook;
#[cfg(test)]
impl<K> FileHook<K> for NoopFileHook {
    fn on_append(&self, _key: &K) {}
    fn on_force_append(&self, _key: &K) {}
}
```

- [ ] **Step 2: 重写 RecordManager 结构体和 impl**

替换现有的 `RecordManager` 定义（原第 238 行起），保留 `RECORD_MANAGER` static 和 `get()` 方法：

```rust
use std::time::Duration;

pub struct RecordManager {
    channel_writer: Arc<RecordAppendWriter<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>>,
    channel_appender: FileObjectAppender<ChannelRecordKey, ChannelRecord,
        RecordAppendWriter<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>,
        RecordWriterContext<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>,
        LogErrorHandler>,

    think_writer: Arc<RecordAppendWriter<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>>,
    think_appender: FileObjectAppender<RecordKey, ThinkRecord,
        RecordAppendWriter<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>,
        RecordWriterContext<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>,
        LogErrorHandler>,

    tool_call_writer: Arc<RecordAppendWriter<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>>,
    tool_call_appender: FileObjectAppender<RecordKey, ToolCallRecord,
        RecordAppendWriter<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>,
        RecordWriterContext<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>,
        LogErrorHandler>,

    tool_result_writer: Arc<RecordAppendWriter<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>>,
    tool_result_appender: FileObjectAppender<RecordKey, ToolResultRecord,
        RecordAppendWriter<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>,
        RecordWriterContext<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>,
        LogErrorHandler>,
}
```

为简化类型标注，使用类型别名：

```rust
// 在文件顶部或 RecordManager 之前
type ChannelWriter = RecordAppendWriter<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>;
type ChannelContext = RecordWriterContext<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>;
type ChannelAppender = FileObjectAppender<ChannelRecordKey, ChannelRecord, ChannelWriter, ChannelContext, LogErrorHandler>;

type ThinkWriter = RecordAppendWriter<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>;
type ThinkContext = RecordWriterContext<RecordKey, ThinkRecord, ThinkParser, ThinkFileIndexHook>;
type ThinkAppender = FileObjectAppender<RecordKey, ThinkRecord, ThinkWriter, ThinkContext, LogErrorHandler>;

type ToolCallWriter = RecordAppendWriter<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>;
type ToolCallContext = RecordWriterContext<RecordKey, ToolCallRecord, ToolCallParser, ToolCallFileIndexHook>;
type ToolCallAppender = FileObjectAppender<RecordKey, ToolCallRecord, ToolCallWriter, ToolCallContext, LogErrorHandler>;

type ToolResultWriter = RecordAppendWriter<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>;
type ToolResultContext = RecordWriterContext<RecordKey, ToolResultRecord, ToolResultParser, ToolResultFileIndexHook>;
type ToolResultAppender = FileObjectAppender<RecordKey, ToolResultRecord, ToolResultWriter, ToolResultContext, LogErrorHandler>;
```

- [ ] **Step 3: 实现 RecordManager::new**

```rust
impl RecordManager {
    pub fn new() -> Self {
        let channel_writer = Arc::new(RecordAppendWriter::new(ChannelParser {}, ChannelFileIndexHook {}));
        let channel_appender = FileObjectAppender::new(
            channel_writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        );

        let think_writer = Arc::new(RecordAppendWriter::new(ThinkParser {}, ThinkFileIndexHook {}));
        let think_appender = FileObjectAppender::new(
            think_writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        );

        let tool_call_writer = Arc::new(RecordAppendWriter::new(ToolCallParser {}, ToolCallFileIndexHook {}));
        let tool_call_appender = FileObjectAppender::new(
            tool_call_writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        );

        let tool_result_writer = Arc::new(RecordAppendWriter::new(ToolResultParser {}, ToolResultFileIndexHook {}));
        let tool_result_appender = FileObjectAppender::new(
            tool_result_writer.clone(),
            Arc::new(LogErrorHandler),
            Duration::from_millis(100),
            10,
        );

        Self {
            channel_writer,
            channel_appender,
            think_writer,
            think_appender,
            tool_call_writer,
            tool_call_appender,
            tool_result_writer,
            tool_result_appender,
        }
    }

    pub fn get() -> &'static Self {
        RECORD_MANAGER.get_or_init(|| RecordManager::new())
    }
}
```

- [ ] **Step 4: 实现 append 方法（Stage 1 预检 + Stage 2 投递）**

```rust
impl RecordManager {
    pub async fn append_channel_record(&self, requests: Vec<ChannelRequest>, force: bool) -> Result<()> {
        // Stage 1: 解析 → 分组 → 排序 → 预检
        let mut records_map: HashMap<ChannelRecordKey, Vec<ChannelRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ChannelParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }

        for (key, records) in &records_map {
            let mut sorted = records.clone();
            sorted.sort_by(|a, b| a.cmp(b));

            let lock = self.channel_writer.get_lock(key).await;
            let ctx = lock.lock().await;
            if let Some(ref state) = ctx.state {
                if state.time.as_str() > sorted[0].time() {
                    if !force {
                        return Err(Error::RecordNotInOrder(
                            state.time.to_string(),
                            sorted[0].time().to_string(),
                        ));
                    }
                }
            }
        }

        // Stage 2: 排序后投递到 FileObjectAppender
        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            self.channel_appender.append(key, records).await;
        }

        Ok(())
    }

    pub async fn append_think_record(&self, requests: Vec<ThinkRequest>, force: bool) -> Result<()> {
        let mut records_map: HashMap<RecordKey, Vec<ThinkRecord>> = HashMap::new();
        for request in requests {
            let (key, record) = ThinkParser.parse_request(request);
            records_map.entry(key).or_default().push(record);
        }

        for (key, records) in &records_map {
            let mut sorted = records.clone();
            sorted.sort_by(|a, b| a.cmp(b));

            let lock = self.think_writer.get_lock(key).await;
            let ctx = lock.lock().await;
            if let Some(ref state) = ctx.state {
                if state.time.as_str() > sorted[0].time() {
                    if !force {
                        return Err(Error::RecordNotInOrder(
                            state.time.to_string(),
                            sorted[0].time().to_string(),
                        ));
                    }
                }
            }
        }

        for (key, mut records) in records_map {
            records.sort_by(|a, b| a.cmp(b));
            self.think_appender.append(key, records).await;
        }

        Ok(())
    }

    // tool_call 和 tool_result 类似...
}
```

为减少重复，提取 Stage 1 公共逻辑到辅助方法：

```rust
impl RecordManager {
    /// Stage 1 预检：对单个 key 检查顺序，返回错误如果乱序且非 force
    async fn check_order<K, R>(
        writer: &RecordAppendWriter<K, R, impl FilePathGenerator<K>, impl FileHook<K>>,
        records_map: &HashMap<K, Vec<R>>,
        force: bool,
    ) -> Result<()>
    where
        K: Eq + Hash + Clone + Send + Sync,
        R: MemoryRecord + Clone,
    {
        for (key, records) in records_map {
            let mut sorted = records.clone();
            sorted.sort_by(|a, b| a.cmp(b));

            let lock = writer.get_lock(key).await;
            let ctx = lock.lock().await;
            if let Some(ref state) = ctx.state {
                if state.time.as_str() > sorted[0].time() && !force {
                    return Err(Error::RecordNotInOrder(
                        state.time.to_string(),
                        sorted[0].time().to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}
```

问题：由于 `RecordAppendWriter` 的泛型参数较多，且 `FileAppendWriter` trait 的 `get_lock` 需要具体类型，提取通用辅助方法可能不可行或过于复杂。

**更实际的方案**：在每个 append 方法中内联重复 Stage 1 逻辑（4 个方法各约 15 行重复），不做抽象提取。保持代码清晰可读。

- [ ] **Step 5: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-store && cargo check 2>&1
```
Expected: 编译成功（测试除外）。

- [ ] **Step 6: 提交**

```bash
git add kissbot-memory-store/src/record.rs
git commit -m "refactor(memory-store): 重写 RecordManager 使用 FileObjectAppender

- 添加 LogErrorHandler（日志记录）
- 4 个 writer + 4 个 appender 的 RecordManager 结构
- Stage 1 同步预检：解析→分组→排序→检查顺序
- Stage 2 异步投递到 FileObjectAppender
- 公开 API 签名不变

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 4: 适配测试

**Files:**
- Modify: `kissbot-memory-store/src/record.rs`（测试模块）

**Interfaces:**
- Consumes: 新的 `RecordAppendWriter`、`FileObjectAppender`、`NoopFileHook`
- Produces: 适配后的测试，覆盖所有原有场景

- [ ] **Step 1: 更新 NoopFileHook**

移动到测试模块内（如已添加则跳过）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct NoopFileHook;
    impl<K> FileHook<K> for NoopFileHook {
        fn on_append(&self, _key: &K) {}
        fn on_force_append(&self, _key: &K) {}
    }
    // ...
}
```

- [ ] **Step 2: 添加测试辅助函数**

```rust
/// 创建测试用的 writer + appender 对
fn create_test_appender<K, R, P, H>(
    parser: P, hook: H,
) -> (Arc<RecordAppendWriter<K, R, P, H>>, FileObjectAppender<K, R, RecordAppendWriter<K, R, P, H>, RecordWriterContext<K, R, P, H>, LogErrorHandler>)
where
    // 各种 bound...
{
    let writer = Arc::new(RecordAppendWriter::new(parser, hook));
    let appender = FileObjectAppender::new(
        writer.clone(),
        Arc::new(LogErrorHandler),
        Duration::from_millis(50),  // 短 timeout 便于测试
        100,
    );
    (writer, appender)
}
```

由于泛型约束复杂，更好的方案是在每个测试中直接构造：

```rust
let writer = Arc::new(RecordAppendWriter::new(ChannelParser {}, NoopFileHook));
let appender = FileObjectAppender::new(
    writer.clone(),
    Arc::new(LogErrorHandler),
    Duration::from_millis(50),
    100,
);
```

- [ ] **Step 3: 重写 test_load_state_* 测试**

这些测试只测试 `load_existing_file_state` 函数，不依赖 `RecordContext`，所以不需要改变。

- [ ] **Step 4: 重写 test_append_new_file 测试**

```rust
#[tokio::test]
async fn test_append_new_file() {
    init_test_config();
    let root = &MemoryConfig::get().root_dir;

    let writer = Arc::new(RecordAppendWriter::new(ChannelParser {}, NoopFileHook));
    let appender = FileObjectAppender::new(
        writer.clone(),
        Arc::new(LogErrorHandler),
        Duration::from_millis(50),
        100,
    );

    // 需要在 RecordManager 之外也能进行 Stage 1 预检
    // 测试中可以直接调用 appender.append（跳过预检）

    let requests = vec![ChannelRequest { /* ... */ }];
    let mut records_map: HashMap<ChannelRecordKey, Vec<ChannelRecord>> = HashMap::new();
    for request in requests {
        let (key, record) = ChannelParser.parse_request(request);
        records_map.entry(key).or_default().push(record);
    }
    for (key, mut records) in records_map {
        records.sort_by(|a, b| a.cmp(b));
        appender.append(key, records).await;
    }

    // 等待 flush
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 验证文件...
}
```

- [ ] **Step 5: 重写所有 append 测试**

对所有测试应用相同的模式：
1. 创建 `RecordAppendWriter` + `FileObjectAppender`
2. 模拟 Stage 1 逻辑（解析、分组、排序）
3. 调用 `appender.append(key, records).await`
4. `tokio::time::sleep(Duration::from_millis(100)).await` 等待 flush
5. 读取文件验证结果

测试列表：
- `test_load_state_file_not_exists` — 不变
- `test_load_state_empty_file` — 不变
- `test_load_state_with_records` — 不变
- `test_append_new_file` — 重写
- `test_append_multiple_records` — 重写
- `test_append_sequential` — 重写
- `test_append_think_record` — 重写
- `test_append_tool_call_record` — 重写
- `test_append_tool_result_record` — 重写
- `test_append_out_of_order_rejected` — 使用 `RecordManager` 的 `append_channel_record`（触发 Stage 1 预检）
- `test_append_force_out_of_order` — 同上，force=true
- `test_append_force_with_existing_data` — 同上
- `test_append_multiple_keys` — 重写

对于乱序测试，需要 `RecordManager` 来触发 Stage 1 预检。但是在测试中创建 `RecordManager` 会创建 4 个 appender。可以提取 `check_order` 逻辑或直接调用 RecordManager 的方法。

由于 `RecordManager::get()` 使用 `OnceLock`，测试中不能调用它（config 冲突）。所以测试应该直接构造 `RecordManager` 或仅测试特定类型。

方案：在测试模块中直接创建 `RecordManager` 实例（或只测试特定类型）。

```rust
#[tokio::test]
async fn test_append_out_of_order_rejected() {
    init_test_config();

    let rm = RecordManager::new();  // 直接构造，不用 OnceLock
    // 但 RecordManager::new() 使用 OnceLock？不，new() 是普通的构造函数
    // RecordManager::get() 使用 OnceLock

    // 先写入 time=10:02:00
    let req1 = vec![ChannelRequest { /* later */ }];
    rm.append_channel_record(req1, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 再写入 time=10:00:00 — 应该被拒绝
    let req2 = vec![ChannelRequest { /* earlier */ }];
    let result = rm.append_channel_record(req2, false).await;
    assert!(result.is_err());
    // ...
}
```

实际上，`RecordManager::new()` 就是普通的构造函数。`RecordManager::get()` 使用 `OnceLock`。测试中可以用 `RecordManager::new()` 直接构造。

但初始化 config 的问题呢？`init_test_config()` 使用 `OnceLock` + `Once` 来确保只初始化一次 config。这没问题。

`RecordManager::new()` 不依赖 config（config 只用于获取 root_dir，但 root_dir 在 init_test_config 中已设置好）。所以直接 new 就行。

但等一下，`RecordManager::new()` 内部使用 `RecordAppendWriter::new()`，这些不依赖 config。好，直接构造没问题。

对于测试，使用 `RecordManager` 的好处是：可以测试完整的 Stage 1 + Stage 2 流程，包括乱序检查。

对于不需要乱序检查的测试（如正常追加），可以直接使用 `appender` + 手动 sleep 等待 flush。

为简化测试，提取辅助函数：

```rust
/// 等待 FileObjectAppender flush 完成
async fn flush() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}
```

- [ ] **Step 6: 运行全部测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory-store && cargo test 2>&1
```
Expected: 全部测试通过。

- [ ] **Step 7: 提交**

```bash
git add kissbot-memory-store/src/record.rs
git commit -m "test(memory-store): 适配测试到 FileObjectAppender 架构

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 5: 最终验证

- [ ] **Step 1: 完整编译检查**

```bash
cd /home/admin/project/kissbot/kissbot-memory-store && cargo check 2>&1
cd /home/admin/project/kissbot/kai-rs/kai-file && cargo check 2>&1
```
Expected: 两者均编译成功。

- [ ] **Step 2: 运行全部测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory-store && cargo test 2>&1
```
Expected: 全部测试通过。

- [ ] **Step 3: 最后一次提交**

```bash
git add -A
git commit -m "refactor(memory-store): 使用 FileObjectAppender 重写 record.rs

- 删除旧 RecordContext<Q,K,R,P>
- 新增 RecordWriterContext<K,R,P,H> 实现 FileAppendWriterContext
- 新增 RecordAppendWriter<K,R,P,H> 实现 FileAppendWriter
- 新增 LogErrorHandler 记录 Stage 2 写入错误
- RecordManager 使用 4 个 writer + 4 个 appender
- Stage 1 同步预检 + Stage 2 异步写入
- 保留 FileState、load_existing_file_state、write_records_to_file
- 适配全部测试用例

Co-Authored-By: deepseek-v4-flash"
```
