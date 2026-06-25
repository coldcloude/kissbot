# memory-store 单元测试实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 kissbot-memory-store 模块编写 config.rs 和 record.rs 的单元测试，使用临时目录集成测试风格。

**Architecture:** Config 测试覆盖正常加载和加载失败路径。Record 测试覆盖 load_existing_file_state 的三种状态、append_record 的正常写入/时序检查/force 重排序/多 key 隔离路径，使用 NoopFileHook 避免依赖 MemoryIndexer。

**Tech Stack:** Rust, tokio, tempfile

---

## 文件结构

| 文件 | 操作 | 说明 |
|------|------|------|
| `kissbot-memory-store/Cargo.toml` | 修改 | 增加 tempfile dev-dependency |
| `kissbot-memory-store/src/config.rs` | 修改 | 追加测试模块 |
| `kissbot-memory-store/src/record.rs` | 修改 | 追加测试模块 |

record.rs 的测试需要：
- 一个 `NoopFileHook`（不调用 MemoryIndexer 的空 hook），用于测试时替代 ChannelFileIndexHook 等
- `load_existing_file_state` 函数现在是 `pub(crate)` — 确认可见性；如果不满足则需要调整为 `pub(crate)`

---

### Task 1: 添加 tempfile 依赖 + 确认函数可见性

**Files:**
- Modify: `kissbot-memory-store/Cargo.toml`
- Modify: `kissbot-memory-store/src/record.rs`

- [ ] **Step 1: 修改 Cargo.toml，添加 tempfile**

在 `kissbot-memory-store/Cargo.toml` 末尾添加：

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 检查 `load_existing_file_state` 可见性**

`load_existing_file_state` 函数在第 40 行定义，没有 `pub` 修饰，默认是模块私有。需要在测试模块中访问它，所以改为 `pub(crate)`：

```rust
// 第 40 行修改
pub(crate) async fn load_existing_file_state(file_path: &PathBuf) -> Result<FileState> {
```

**注意：** `FileState` 也是在 record.rs 内部定义的（第 16-19 行），测试模块在同一文件中，可以直接访问私有成员。

- [ ] **Step 3: 确认编译通过**

```bash
cd /home/admin/project/kissbot/kissbot-memory-store && cargo check 2>&1
```
Expected: 编译成功，没有 warning。

- [ ] **Step 4: Commit**

```bash
cd /home/admin/project/kissbot && git add kissbot-memory-store/Cargo.toml kissbot-memory-store/src/record.rs
git commit -m "test: 添加 tempfile 依赖，调整 load_existing_file_state 为 pub(crate)"
```

---

### Task 2: config.rs 测试 — 配置加载

**Files:**
- Modify: `kissbot-memory-store/src/config.rs`

在文件末尾追加测试模块：

- [ ] **Step 1: 在 config.rs 末尾添加测试模块**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_load_ok() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test-config.json");
        let json_content = format!(
            r#"{{"listen_addr": "127.0.0.1", "listen_port": 8082, "api_key": "test-key-123"}}"#
        );
        std::fs::write(&config_path, json_content).unwrap();

        // SAFETY: test environment, single-threaded, no concurrent env access
        unsafe { std::env::set_var("KISSBOT_MEMORY_STORE_CONFIG", config_path.to_str().unwrap()); }
        let config = Config::load().unwrap();
        assert_eq!(config.listen_addr, "127.0.0.1");
        assert_eq!(config.listen_port, 8082);
        assert_eq!(config.api_key, "test-key-123");
        unsafe { std::env::remove_var("KISSBOT_MEMORY_STORE_CONFIG"); }
    }

    #[test]
    fn test_config_load_error() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("nonexistent-config.json");

        unsafe { std::env::set_var("KISSBOT_MEMORY_STORE_CONFIG", config_path.to_str().unwrap()); }
        let result = Config::load();
        assert!(result.is_err());
        unsafe { std::env::remove_var("KISSBOT_MEMORY_STORE_CONFIG"); }
    }
}
```

- [ ] **Step 2: 运行测试验证**

```bash
cd /home/admin/project/kissbot && cargo test -p kissbot-memory-store -- config 2>&1
```

Expected: 2 个测试全部 PASS。

- [ ] **Step 3: Commit**

```bash
cd /home/admin/project/kissbot && git add kissbot-memory-store/src/config.rs
git commit -m "test: config.rs 添加配置加载测试（正常路径 + 错误路径）"
```

---

### Task 3: record.rs 测试— 辅助函数和 load_existing_file_state 测试

**Files:**
- Modify: `kissbot-memory-store/src/record.rs`

- [ ] **Step 1: 在 record.rs 末尾追加测试模块和 NoopFileHook**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 空操作 FileHook，不调用 MemoryIndexer
    struct NoopFileHook;

    impl<K> FileHook<K> for NoopFileHook {
        fn on_append(&self, _key: &K) {}
        fn on_force_append(&self, _key: &K) {}
    }
}
```

**注意：** `FileHook` 在 `kissbot_memory::data` 中定义，测试模块中通过 `use super::*;` 已经能访问到（record.rs 第 10 行有 `use kissbot_memory::data::...`）。

- [ ] **Step 2: 添加 load_existing_file_state 的三个测试**

在 `mod tests` 内部追加：

```rust
    #[tokio::test]
    async fn test_load_state_file_not_exists() {
        let state = load_existing_file_state(&PathBuf::from("/tmp/nonexistent_file_xyz.jsonl")).await.unwrap();
        assert_eq!(state.sn, 0);
        assert_eq!(*state.time, "2000-01-01 00:00:00");
    }

    #[tokio::test]
    async fn test_load_state_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.jsonl");
        tokio::fs::write(&file_path, "").await.unwrap();

        let state = load_existing_file_state(&file_path).await.unwrap();
        assert_eq!(state.sn, 0);
        assert_eq!(*state.time, "2000-01-01 00:00:00");
    }

    #[tokio::test]
    async fn test_load_state_with_records() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("records.jsonl");

        // 写入 3 条 JSONL 记录
        let lines = vec![
            r#"{"sn":1,"time":"2026-06-25 10:00:00","content":"msg1"}"#,
            r#"{"sn":2,"time":"2026-06-25 10:01:00","content":"msg2"}"#,
            r#"{"sn":3,"time":"2026-06-25 10:02:00","content":"msg3"}"#,
        ];
        let content = lines.join("\n") + "\n";
        tokio::fs::write(&file_path, content).await.unwrap();

        let state = load_existing_file_state(&file_path).await.unwrap();
        assert_eq!(state.sn, 3);
        assert_eq!(*state.time, "2026-06-25 10:02:00");
    }
```

- [ ] **Step 3: 运行测试验证**

```bash
cd /home/admin/project/kissbot && cargo test -p kissbot-memory-store -- record::tests::test_load_state 2>&1
```

Expected: 3 个测试全部 PASS。

- [ ] **Step 4: Commit**

```bash
cd /home/admin/project/kissbot && git add kissbot-memory-store/src/record.rs
git commit -m "test: record.rs 添加 NoopFileHook 和 load_existing_file_state 测试"
```

---

### Task 4: record.rs 测试— append_record 正常写入 + 四种记录类型

**Files:**
- Modify: `kissbot-memory-store/src/record.rs`

- [ ] **Step 1: 添加 init_test_config 辅助函数**

在 `mod tests` 中追加：

```rust
    use std::sync::Once;
    use kissbot_memory::Config as MemoryConfig;

    static INIT: Once = Once::new();

    fn init_test_config(root_dir: &std::path::Path) {
        INIT.call_once(|| {
            unsafe { std::env::set_var(
                "KISSBOT_MEMORY_CONFIG",
                root_dir.join("memory-config.json").to_str().unwrap()
            ); }
            let json_content = format!(r#"{{"root_dir": "{}"}}"#, root_dir.display().to_string());
            std::fs::write(root_dir.join("memory-config.json"), &json_content).unwrap();
            // 提前初始化 MemoryConfig 的 OnceLock
            let _ = MemoryConfig::get();
        });
    }
```

**注意：** 这里 `root_dir` 是独立的临时目录路径。`INIT.call_once` 保证只初始化一次。

- [ ] **Step 2: 添加 append_record 正常写入测试**

在 `mod tests` 中追加（`init_test_config` 之后）：

```rust
    #[tokio::test]
    async fn test_append_new_file() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("hello".to_string()),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
        ];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        // 验证文件被创建
        let expected_path = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        // 读取文件内容验证
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ChannelRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
        assert_eq!(*record.content(), "hello");
    }

    #[tokio::test]
    async fn test_append_multiple_records() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("msg1".to_string()),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("msg2".to_string()),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("msg3".to_string()),
                time: Arc::new("2026-06-25 10:02:00".to_string()),
            },
        ];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        let expected_path = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 3);

        // 验证 sn 顺序
        for (i, line) in lines.iter().enumerate() {
            let record: ChannelRecord = serde_json::from_str(line).unwrap();
            assert_eq!(record.sn(), (i + 1) as u64);
        }
    }

    #[tokio::test]
    async fn test_append_sequential() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 第一次写入
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("first".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        ctx.append_record(req1, false, NoopFileHook).await.unwrap();

        // 第二次写入
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("second".to_string()),
            time: Arc::new("2026-06-25 10:01:00".to_string()),
        }];
        ctx.append_record(req2, false, NoopFileHook).await.unwrap();

        let expected_path = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);

        // 第一条 sn=1，第二条 sn=2
        let r1: ChannelRecord = serde_json::from_str(lines[0]).unwrap();
        let r2: ChannelRecord = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r1.sn(), 1);
        assert_eq!(*r1.content(), "first");
        assert_eq!(r2.sn(), 2);
        assert_eq!(*r2.content(), "second");
    }
```

- [ ] **Step 3: 添加四种记录类型的写入测试**

```rust
    #[tokio::test]
    async fn test_append_think_record() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ThinkRequest, RecordKey, ThinkRecord, ThinkParser> =
            RecordContext::new(ThinkParser {});

        let requests = vec![ThinkRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("".to_string()),
            content: Arc::new("I think...".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        let expected_path = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026")
            .join("think-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ThinkRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
        assert_eq!(*record.content(), "I think...");
    }

    #[tokio::test]
    async fn test_append_tool_call_record() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ToolCallRequest, RecordKey, ToolCallRecord, ToolCallParser> =
            RecordContext::new(ToolCallParser {});

        let requests = vec![ToolCallRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("".to_string()),
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        let expected_path = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026")
            .join("tool-call-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ToolCallRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
    }

    #[tokio::test]
    async fn test_append_tool_result_record() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ToolResultRequest, RecordKey, ToolResultRecord, ToolResultParser> =
            RecordContext::new(ToolResultParser {});

        let requests = vec![ToolResultRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("".to_string()),
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        let expected_path = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026")
            .join("tool-result-records-2026-06-25.jsonl");
        assert!(expected_path.exists(), "file should exist: {:?}", expected_path);

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let record: ToolResultRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.sn(), 1);
    }
```

- [ ] **Step 4: 运行测试验证**

```bash
cd /home/admin/project/kissbot && cargo test -p kissbot-memory-store -- record::tests::test_append 2>&1
```

Expected: 6 个测试全部 PASS。

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot && git add kissbot-memory-store/src/record.rs
git commit -m "test: record.rs 添加 append_record 正常写入测试（6 个）"
```

---

### Task 5: record.rs 测试— 时序检查 + force 模式 + 多 key 隔离

**Files:**
- Modify: `kissbot-memory-store/src/record.rs`

- [ ] **Step 1: 添加时序检查测试**

```rust
    #[tokio::test]
    async fn test_append_out_of_order_rejected() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 先写入一条 time=10:02:00
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("later".to_string()),
            time: Arc::new("2026-06-25 10:02:00".to_string()),
        }];
        ctx.append_record(req1, false, NoopFileHook).await.unwrap();

        // 再写入一条 time=10:00:00 — 早于已有记录，应该被拒绝
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("earlier".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        let result = ctx.append_record(req2, false, NoopFileHook).await;
        assert!(result.is_err());
        match result {
            Err(Error::RecordNotInOrder(latest, new)) => {
                assert_eq!(latest, "2026-06-25 10:02:00");
                assert_eq!(new, "2026-06-25 10:00:00");
            }
            _ => panic!("expected RecordNotInOrder error"),
        }
    }

    #[tokio::test]
    async fn test_append_force_out_of_order() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 先写入一条 time=10:02:00
        let req1 = vec![ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("later".to_string()),
            time: Arc::new("2026-06-25 10:02:00".to_string()),
        }];
        ctx.append_record(req1, false, NoopFileHook).await.unwrap();

        // force=true 写入一条 time=10:00:00 — 强制重排序
        let req2 = vec![ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("earlier".to_string()),
            time: Arc::new("2026-06-25 10:00:00".to_string()),
        }];
        ctx.append_record(req2, true, NoopFileHook).await.unwrap();

        // 验证文件有 2 条记录，按 time 排序，sn 重编号
        let expected_path = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);

        // 第一条 sn=1, time=10:00:00, content=earlier
        let r1: ChannelRecord = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(r1.sn(), 1);
        assert_eq!(*r1.content(), "earlier");

        // 第二条 sn=2, time=10:02:00, content=later
        let r2: ChannelRecord = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r2.sn(), 2);
        assert_eq!(*r2.content(), "later");
    }
```

- [ ] **Step 2: 添加 force 模式对有已有数据的文件进行重排序测试**

```rust
    #[tokio::test]
    async fn test_append_force_with_existing_data() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 先写入 3 条，time 分别是 10:01, 10:02, 10:03
        let req1 = vec![
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("second".to_string()),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("third".to_string()),
                time: Arc::new("2026-06-25 10:02:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("fourth".to_string()),
                time: Arc::new("2026-06-25 10:03:00".to_string()),
            },
        ];
        ctx.append_record(req1, false, NoopFileHook).await.unwrap();

        // force 写入 2 条更早的记录：09:59, 10:00
        let req2 = vec![
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("first".to_string()),
                time: Arc::new("2026-06-25 09:59:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("first-half".to_string()),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
        ];
        ctx.append_record(req2, true, NoopFileHook).await.unwrap();

        // 验证文件有 5 条记录，按 time 排序，sn 重编号
        let expected_path = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 5);

        let expected_order = ["first", "first-half", "second", "third", "fourth"];
        for (i, line) in lines.iter().enumerate() {
            let record: ChannelRecord = serde_json::from_str(line).unwrap();
            assert_eq!(record.sn(), (i + 1) as u64, "sn mismatch at line {}", i);
            assert_eq!(*record.content(), expected_order[i], "content mismatch at line {}", i);
        }
    }
```

- [ ] **Step 3: 添加多 key 隔离测试**

```rust
    #[tokio::test]
    async fn test_append_multiple_keys() {
        let dir = tempfile::tempdir().unwrap();
        init_test_config(dir.path());

        let ctx: RecordContext<ChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser> =
            RecordContext::new(ChannelParser {});

        // 两个不同 key 的请求（不同 agent_id）
        let requests = vec![
            ChannelRequest {
                agent_id: Arc::new("agent1".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u1".to_string()),
                group_id: Arc::new("g1".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("agent1-msg".to_string()),
                time: Arc::new("2026-06-25 10:00:00".to_string()),
            },
            ChannelRequest {
                agent_id: Arc::new("agent2".to_string()),
                role_name: Arc::new("default".to_string()),
                messenger_id: Arc::new("telegram".to_string()),
                user_id: Arc::new("u2".to_string()),
                group_id: Arc::new("g2".to_string()),
                is_self: 0,
                msg_type: Arc::new("text".to_string()),
                content: Arc::new("agent2-msg".to_string()),
                time: Arc::new("2026-06-25 10:01:00".to_string()),
            },
        ];

        ctx.append_record(requests, false, NoopFileHook).await.unwrap();

        // 验证两个文件都存在，sn 各自从 1 开始
        let path1 = dir.path()
            .join("agent1")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u1=g1-records-2026-06-25.jsonl");
        let path2 = dir.path()
            .join("agent2")
            .join("memory-store")
            .join("2026-default")
            .join("channel-telegram=u2=g2-records-2026-06-25.jsonl");

        assert!(path1.exists(), "file for agent1 should exist");
        assert!(path2.exists(), "file for agent2 should exist");

        let r1: ChannelRecord = serde_json::from_str(tokio::fs::read_to_string(&path1).await.unwrap().trim()).unwrap();
        let r2: ChannelRecord = serde_json::from_str(tokio::fs::read_to_string(&path2).await.unwrap().trim()).unwrap();
        assert_eq!(r1.sn(), 1);
        assert_eq!(r2.sn(), 1);
        assert_eq!(*r1.content(), "agent1-msg");
        assert_eq!(*r2.content(), "agent2-msg");
    }
```

- [ ] **Step 4: 运行所有测试验证**

```bash
cd /home/admin/project/kissbot && cargo test -p kissbot-memory-store 2>&1
```

Expected: 14 个测试全部 PASS（2 config + 3 load_state + 6 append 正常 + 2 时序 + 1 force 合并 + 1 多 key）。

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot && git add kissbot-memory-store/src/record.rs
git commit -m "test: record.rs 添加时序检查/force模式/多key隔离测试（4 个）"
```

---

### Self-Review 检查清单

1. **Spec coverage:** Config 测试覆盖了 load_ok 和 load_error。Record 测试覆盖了 load_existing_file_state（3 case）、正常写入（6 case）、时序检查（2 case）、force 合并（1 case）、多 key 隔离（1 case）。所有 spec 中的测试点都有对应 task。
2. **Placeholder scan:** 无 TBD/TODO/占位符。每段代码完整可编译。
3. **Type consistency:** 所有类型名 (`ChannelRequest`, `ChannelRecordKey`, `RecordContext`, `ChannelParser` 等) 与 record.rs 第 10 行 `use` 的导入一致。`NoopFileHook` 通过泛型 `FileHook<K>` 实现，与 record.rs 中 `append_record` 的 `H: FileHook<K>` 约束匹配。`RoleName = ""` 时目录路径不带 `-` 后缀，与 `ensure_year_role_dir` 逻辑一致。
