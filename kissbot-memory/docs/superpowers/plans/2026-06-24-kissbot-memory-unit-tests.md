# kissbot-memory 单元测试实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 kissbot-memory crate 编写 39 个单元测试，覆盖 config/directory/data/index 四个模块

**Architecture:** 测试内联在各模块末尾的 `#[cfg(test)] mod tests` 中。纯逻辑同步 `#[test]`，文件 IO 用 `#[tokio::test]`。临时目录使用 `tempfile`。先加 dev-dependency，再按模块逐个添加测试。

**Tech Stack:** Rust, tokio, serde, serde_json, chrono, dashmap, kai-file, kissbot-api, tempfile

**设计文档:** `kissbot-memory/docs/superpowers/specs/2026-06-24-kissbot-memory-test-design.md`

---

## 文件结构

- **Modify:** `Cargo.toml` — 添加 tempfile dev-dependency
- **Modify:** `src/config.rs` — 添加测试
- **Modify:** `src/directory.rs` — 添加测试
- **Modify:** `src/data.rs` — 添加测试
- **Modify:** `src/index.rs` — 添加测试

---

### Task 1: 添加 tempfile dev-dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: 在 Cargo.toml 末尾添加 dev-dependencies**

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo check 2>&1
```

Expected: 编译通过，无错误。

- [ ] **Step 3: 提交**

```bash
git add Cargo.toml
git commit -m "chore: 添加 tempfile dev-dependency

为单元测试的临时目录操作添加 tempfile crate。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: config.rs 测试

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: 在 config.rs 末尾添加测试模块**

在 `impl Config { ... }` 块之后，文件末尾添加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_with_root_dir() {
        let config = Config::with_root_dir("/tmp/test_memory");
        assert_eq!(config.root_dir, std::path::PathBuf::from("/tmp/test_memory"));
    }
}
```

- [ ] **Step 2: 编译并运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_config_with_root_dir -- --nocapture 2>&1
```

Expected: 1 passed

- [ ] **Step 3: 提交**

```bash
git add src/config.rs
git commit -m "test: config.rs 单元测试（1 个）

test_config_with_root_dir 验证 with_root_dir 构造器。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: directory.rs 测试（纯路径函数）

**Files:**
- Modify: `src/directory.rs`

- [ ] **Step 1: 在 directory.rs 末尾添加测试模块**

在 `impl DirectoryManager { ... }` 块之后，文件末尾添加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_agent_dir() {
        let result = agent_dir("/root", "agent1");
        assert_eq!(result, Path::new("/root").join("agent1"));
    }

    #[test]
    fn test_agent_uuid_file() {
        let adir = Path::new("/root").join("agent1");
        let result = agent_uuid_file(&adir, "agent1");
        assert_eq!(result, adir.join("agent-agent1"));
    }

    #[test]
    fn test_agent_ego_dir() {
        let adir = Path::new("/root").join("agent1");
        let result = agent_ego_dir(&adir);
        assert_eq!(result, adir.join("memory-ego"));
    }

    #[test]
    fn test_agent_store_dir() {
        let adir = Path::new("/root").join("agent1");
        let result = agent_store_dir(&adir);
        assert_eq!(result, adir.join("memory-store"));
    }

    #[test]
    fn test_dir_manager_new() {
        let dm = DirectoryManager::new("/tmp/test_memory");
        assert_eq!(dm.root_dir, std::path::PathBuf::from("/tmp/test_memory"));
    }
}
```

- [ ] **Step 2: 编译并运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_agent_dir test_agent_uuid_file test_agent_ego_dir test_agent_store_dir test_dir_manager_new -- --nocapture 2>&1
```

Expected: 5 passed

- [ ] **Step 3: 提交**

```bash
git add src/directory.rs
git commit -m "test: directory.rs 单元测试（5 个）

4 个纯路径函数测试 + 1 个 DirectoryManager 构造测试。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 4: data.rs 测试 — 时间函数 + Record trait

**Files:**
- Modify: `src/data.rs`

- [ ] **Step 1: 在 data.rs 末尾添加测试模块，包含 6 个测试**

在 `impl RecordCombiner<RecordKey, ToolResultRecord, ToolResultRecordResult> for ToolResultParser { ... }` 块之后，文件末尾添加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // ========== Time functions ==========

    #[test]
    fn test_parse_date_from_time() {
        assert_eq!(parse_date_from_time("2026-06-24 15:30:00"), "2026-06-24");
        assert_eq!(parse_date_from_time("2026-01-01 00:00:00"), "2026-01-01");
    }

    #[test]
    fn test_get_internal_dates() {
        let dates = get_internal_dates("2026-06-22", "2026-06-25").unwrap();
        assert_eq!(dates, vec!["2026-06-23", "2026-06-24"]);
    }

    #[test]
    fn test_get_internal_dates_same_day() {
        let dates = get_internal_dates("2026-06-22", "2026-06-22").unwrap();
        assert!(dates.is_empty());
    }

    #[test]
    fn test_get_date_time_segments() {
        let segments = get_date_time_segments("2026-06-22 14:30:00", "2026-06-24 10:00:00").unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].0, "2026-06-22 14:30:00");
        assert_eq!(segments[0].1, "2026-06-22 23:59:59");
        assert_eq!(segments[1].0, "2026-06-23 00:00:00");
        assert_eq!(segments[1].1, "2026-06-23 23:59:59");
        assert_eq!(segments[2].0, "2026-06-24 00:00:00");
        assert_eq!(segments[2].1, "2026-06-24 10:00:00");
    }

    // ========== Record trait ==========

    #[test]
    fn test_record_impl() {
        let mut channel = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 5,
        };
        assert_eq!(channel.sn(), 5);
        assert_eq!(*channel.time(), "2026-06-24 10:00:00");
        channel.set_sn(10);
        assert_eq!(channel.sn(), 10);

        let think = ThinkRecord {
            content: Arc::new("think".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:01".to_string()),
            sn: 1,
        };
        assert_eq!(think.sn(), 1);
        assert_eq!(*think.time(), "2026-06-24 10:00:01");
    }

    #[test]
    fn test_record_cmp_time() {
        let r1 = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let r2 = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("world".to_string()),
            time: Arc::new("2026-06-24 10:00:01".to_string()),
            sn: 1,
        };
        assert_eq!(r1.cmp(&r2), std::cmp::Ordering::Less);

        // same time, different sn
        let r3 = ChannelRecord {
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 2,
            ..r1.clone()
        };
        assert_eq!(r1.cmp(&r3), std::cmp::Ordering::Less);
    }
}
```

第二步：编译验证。

- [ ] **Step 2: 编译**（此时编译应该通过但有未使用的 import — 后续测试会用）

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_get_internal_dates -- --nocapture 2>&1
```

Expected: 6 passed

- [ ] **Step 3: 提交**

```bash
git add src/data.rs
git commit -m "test: data.rs 时间函数 + Record trait 测试（6 个）

test_parse_date_from_time / test_get_internal_dates（2个）/
test_get_date_time_segments / test_record_impl / test_record_cmp_time。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 5: data.rs 测试 — serde roundtrip（6 个）

**Files:**
- Modify: `src/data.rs`

- [ ] **Step 1: 在 data.rs 测试模块中添加 6 个 serde roundtrip 测试**

在已有测试之后添加：

```rust
    // ========== Record serde ==========

    #[test]
    fn test_serde_channel_record() {
        let obj = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.user_id, "u1");
        assert_eq!(*deserialized.content, "hello");
        assert_eq!(deserialized.sn, 1);
    }

    #[test]
    fn test_serde_think_record() {
        let obj = ThinkRecord {
            content: Arc::new("think content".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.content, "think content");
        assert_eq!(*deserialized.key, "k1");
    }

    #[test]
    fn test_serde_tool_call_record() {
        let obj = ToolCallRecord {
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolCallRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.tool_name, "get_weather");
        assert_eq!(deserialized.tool_params["city"], "Beijing");
    }

    #[test]
    fn test_serde_tool_result_record() {
        let obj = ToolResultRecord {
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolResultRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.tool_result["temp"], 25);
    }

    // ========== Key serde ==========

    #[test]
    fn test_serde_channel_record_key() {
        let obj = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRecordKey = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.messenger_id, "telegram");
        assert_eq!(*deserialized.date, "2026-06-24");
    }

    #[test]
    fn test_serde_record_key() {
        let obj = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RecordKey = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.role_name, "default");
    }
```

- [ ] **Step 2: 编译并运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_serde_channel_record test_serde_think_record test_serde_tool_call_record test_serde_tool_result_record test_serde_channel_record_key test_serde_record_key -- --nocapture 2>&1
```

Expected: 12 passed（含 Task 4 的 6 个，共 12）

- [ ] **Step 3: 提交**

```bash
git add src/data.rs
git commit -m "test: data.rs serde roundtrip 测试（6 个）

4 种 Record + 2 种 RecordKey 的序列化/反序列化测试。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 6: data.rs 测试 — FilePathGenerator（4 个）

**Files:**
- Modify: `src/data.rs`

- [ ] **Step 1: 在 data.rs 测试模块中添加 4 个 FilePathGenerator 测试**

```rust
    // ========== FilePathGenerator ==========

    #[test]
    fn test_channel_file_name() {
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ChannelParser;
        assert_eq!(parser.get_file_name(&key), "channel-m1=u1=g1-records-2026-06-24.jsonl");
    }

    #[test]
    fn test_think_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ThinkParser;
        assert_eq!(parser.get_file_name(&key), "think-records-2026-06-24.jsonl");
    }

    #[test]
    fn test_tool_call_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ToolCallParser;
        assert_eq!(parser.get_file_name(&key), "tool-call-records-2026-06-24.jsonl");
    }

    #[test]
    fn test_tool_result_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ToolResultParser;
        assert_eq!(parser.get_file_name(&key), "tool-result-records-2026-06-24.jsonl");
    }
```

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_channel_file_name test_think_file_name test_tool_call_file_name test_tool_result_file_name -- --nocapture 2>&1
```

Expected: 16 passed（累计 16）

- [ ] **Step 3: 提交**

```bash
git add src/data.rs
git commit -m "test: data.rs FilePathGenerator 测试（4 个）

ChannelParser / ThinkParser / ToolCallParser / ToolResultParser 的文件名生成。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 7: data.rs 测试 — RequestParser（4 个）

**Files:**
- Modify: `src/data.rs`

- [ ] **Step 1: 在 data.rs 测试模块中添加 4 个 RequestParser 测试**

```rust
    // ========== RequestParser ==========

    #[test]
    fn test_channel_request_parser() {
        let request = ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 1,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ChannelParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*key.messenger_id, "telegram");
        assert_eq!(*key.date, "2026-06-24");
        assert_eq!(*record.user_id, "u1");
        assert_eq!(*record.content, "hello");
        assert_eq!(record.sn, 0);
    }

    #[test]
    fn test_think_request_parser() {
        let request = ThinkRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            content: Arc::new("thinking...".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ThinkParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*key.date, "2026-06-24");
        assert_eq!(*record.content, "thinking...");
        assert_eq!(*record.key, "k1");
        assert_eq!(record.sn, 0);
    }

    #[test]
    fn test_tool_call_request_parser() {
        let request = ToolCallRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: serde_json::json!({"city": "Beijing"}),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ToolCallParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*record.tool_name, "get_weather");
        assert_eq!(record.tool_params["city"], "Beijing");
        assert_eq!(record.sn, 0);
    }

    #[test]
    fn test_tool_result_request_parser() {
        let request = ToolResultRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            tool_result: serde_json::json!({"temp": 25}),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ToolResultParser;
        let (key, record) = parser.parse_request(request);
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(record.tool_result["temp"], 25);
        assert_eq!(*record.key, "k1");
        assert_eq!(record.sn, 0);
    }
```

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_channel_request_parser test_think_request_parser test_tool_call_request_parser test_tool_result_request_parser -- --nocapture 2>&1
```

Expected: 20 passed（累计 20）

- [ ] **Step 3: 提交**

```bash
git add src/data.rs
git commit -m "test: data.rs RequestParser 测试（4 个）

ChannelParser / ThinkParser / ToolCallParser / ToolResultParser
的请求解析逻辑。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 8: data.rs 测试 — RecordCombiner（4 个）

**Files:**
- Modify: `src/data.rs`

- [ ] **Step 1: 在 data.rs 测试模块中添加 4 个 RecordCombiner 测试**

```rust
    // ========== RecordCombiner ==========

    #[test]
    fn test_channel_record_combiner() {
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let record = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            is_self: 1,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("hello".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 5,
        };
        let result = ChannelParser.combine_record(&key, &record);
        assert_eq!(*result.agent_id, "agent1");
        assert_eq!(*result.messenger_id, "telegram");
        assert_eq!(*result.content, "hello");
        assert_eq!(result.sn, 5);
    }

    #[test]
    fn test_think_record_combiner() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let record = ThinkRecord {
            content: Arc::new("thinking...".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 3,
        };
        let result = ThinkParser.combine_record(&key, &record);
        assert_eq!(*result.agent_id, "agent1");
        assert_eq!(*result.content, "thinking...");
        assert_eq!(result.sn, 3);
    }

    #[test]
    fn test_tool_call_record_combiner() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let record = ToolCallRecord {
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 2,
        };
        let result = ToolCallParser.combine_record(&key, &record);
        assert_eq!(*result.agent_id, "agent1");
        assert_eq!(*result.tool_name, "get_weather");
        assert_eq!(result.sn, 2);
    }

    #[test]
    fn test_tool_result_record_combiner() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let record = ToolResultRecord {
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 7,
        };
        let result = ToolResultParser.combine_record(&key, &record);
        assert_eq!(*result.agent_id, "agent1");
        assert_eq!(result.tool_result["temp"], 25);
        assert_eq!(result.sn, 7);
    }
```

注意：此步骤添加了 kissbot_api::store::* 的一些依赖。ChannelRecord/ThinkRecord/ToolCallRecord/ToolResultRecord 的 api result 类型已通过 `pub type` 别名在 data.rs 顶部引用。确认编译没有问题。

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_channel_record_combiner test_think_record_combiner test_tool_call_record_combiner test_tool_result_record_combiner -- --nocapture 2>&1
```

Expected: 24 passed（累计 24 — data.rs 共 21 个，但需确认总数）

累计统计：Task 4(6) + Task 5(6) + Task 6(4) + Task 7(4) + Task 8(4) = 24 个 data.rs 测试
（实际比 spec 多 3 个是因为 `test_record_impl` 同时覆盖了多个 Record 类型的 sn/time，等效覆盖了两个单独的测试）

- [ ] **Step 3: 提交**

```bash
git add src/data.rs
git commit -m "test: data.rs RecordCombiner 测试（4 个）

ChannelParser / ThinkParser / ToolCallParser / ToolResultParser
的内部记录到外部结果的转换逻辑。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 9: index.rs 测试 — FilePosition serde + MemoryIndexer 标记方法（9 个）

**Files:**
- Modify: `src/index.rs`

- [ ] **Step 1: 在 index.rs 末尾添加测试模块**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // ========== FilePosition serde ==========

    #[test]
    fn test_serde_file_position() {
        let obj = FilePosition { start_pos: 100, end_pos: 200 };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: FilePosition = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.start_pos, 100);
        assert_eq!(deserialized.end_pos, 200);
    }

    // ========== MemoryIndexer mark methods ==========

    #[test]
    fn test_mark_channel_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        assert!(!indexer.channel_indices.obsolete_set.contains(&key));
        indexer.mark_channel_obsolete(&key);
        assert!(indexer.channel_indices.obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_channel_all_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        assert!(!indexer.channel_indices.all_obsolete_set.contains(&key));
        indexer.mark_channel_all_obsolete(&key);
        assert!(indexer.channel_indices.all_obsolete_set.contains(&key));
    }

    fn make_record_key() -> RecordKey {
        RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        }
    }

    #[test]
    fn test_mark_think_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_think_obsolete(&key);
        assert!(indexer.think_indices.obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_think_all_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_think_all_obsolete(&key);
        assert!(indexer.think_indices.all_obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_tool_call_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_tool_call_obsolete(&key);
        assert!(indexer.tool_call_indices.obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_tool_call_all_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_tool_call_all_obsolete(&key);
        assert!(indexer.tool_call_indices.all_obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_tool_result_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_tool_result_obsolete(&key);
        assert!(indexer.tool_result_indices.obsolete_set.contains(&key));
    }

    #[test]
    fn test_mark_tool_result_all_obsolete() {
        let indexer = MemoryIndexer::new();
        let key = make_record_key();
        indexer.mark_tool_result_all_obsolete(&key);
        assert!(indexer.tool_result_indices.all_obsolete_set.contains(&key));
    }
}
```

注意：`MemoryIndexer` 的 `channel_indices`、`think_indices` 等字段不是 `pub` 的。需要在 `MemoryIndexer` 结构体定义中将这些字段改为 `pub(crate)`，以便同 crate 内的测试可以访问。

- [ ] **Step 2: 将 MemoryIndexer 的字段改为 `pub(crate)`**

在 `index.rs` 中找到：

```rust
pub struct MemoryIndexer {
    channel_indices: FileIndexContext<...>,
    think_indices: FileIndexContext<...>,
    tool_call_indices: FileIndexContext<...>,
    tool_result_indices: FileIndexContext<...>,
}
```

改为：

```rust
pub struct MemoryIndexer {
    pub(crate) channel_indices: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRecordResult, ChannelParser>,
    pub(crate) think_indices: FileIndexContext<QueryRequest, RecordKey, ThinkRecord, ThinkRecordResult, ThinkParser>,
    pub(crate) tool_call_indices: FileIndexContext<QueryRequest, RecordKey, ToolCallRecord, ToolCallRecordResult, ToolCallParser>,
    pub(crate) tool_result_indices: FileIndexContext<QueryRequest, RecordKey, ToolResultRecord, ToolResultRecordResult, ToolResultParser>,
}
```

保持完整类型签名。

- [ ] **Step 3: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_serde_file_position test_mark_channel_obsolete test_mark_channel_all_obsolete -- --nocapture 2>&1
```

Expected: 9 passed（累计 33）

- [ ] **Step 4: 提交**

```bash
git add src/index.rs
git commit -m "test: index.rs 测试（9 个）

FilePosition serde + MemoryIndexer 8 个标记方法测试。
将 MemoryIndexer 字段改为 pub(crate) 以支持测试内访问。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 10: index.rs 测试 — FileIndexContext 构造 + get_lock（3 个）

**Files:**
- Modify: `src/index.rs`

- [ ] **Step 1: 在 index.rs 测试模块中添加 FileIndexContext 构造测试**

```rust
    // ========== FileIndexContext ==========

    const TEST_CHANNEL_KEY: ChannelRecordKey = ChannelRecordKey {
        agent_id: Arc::new(String::new()),
        role_name: Arc::new(String::new()),
        messenger_id: Arc::new(String::new()),
        user_id: Arc::new(String::new()),
        group_id: Arc::new(String::new()),
        date: Arc::new(String::new()),
    };

    #[test]
    fn test_file_index_context_new() {
        let ctx: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRecordResult, ChannelParser> = FileIndexContext::new(ChannelParser {});
        assert!(ctx.position_map_map.is_empty());
        assert!(ctx.obsolete_set.is_empty());
        assert!(ctx.all_obsolete_set.is_empty());
    }

    #[test]
    fn test_file_index_context_get_lock() {
        let ctx: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRecordResult, ChannelParser> = FileIndexContext::new(ChannelParser {});
        let key = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let lock = ctx.get_lock(&key);
        assert_eq!(ctx.position_map_map.len(), 1);
        // 验证返回的锁可以获取（但不进行 IO 操作）
        let _guard = lock.try_read();
        // 写入模式下也可获取
        let _wguard = lock.try_write();
    }
```

注意：`FileIndexContext` 的字段 `position_map_map`、`obsolete_set`、`all_obsolete_set` 也是 private 的。需要改为 `pub(crate)`。

- [ ] **Step 2: 将 FileIndexContext 字段改为 `pub(crate)`**

在 `index.rs` 中找到 `struct FileIndexContext`：

```rust
struct FileIndexContext<Q,K,R,RR,P>
where
    ...
{
    _marker: PhantomData<(Q,R,RR)>,
    position_map_map: DashMap<K, FileIndexLock>,
    obsolete_set: DashSet<K>,
    all_obsolete_set: DashSet<K>,
    parser: P,
}
```

改为：

```rust
struct FileIndexContext<Q,K,R,RR,P>
where
    ...
{
    _marker: PhantomData<(Q,R,RR)>,
    pub(crate) position_map_map: DashMap<K, FileIndexLock>,
    pub(crate) obsolete_set: DashSet<K>,
    pub(crate) all_obsolete_set: DashSet<K>,
    parser: P,
}
```

注意 `PHANTOM_DATA` 不需要 pub(crate)，`parser` 保持 private。

- [ ] **Step 3: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_file_index_context_new test_file_index_context_get_lock -- --nocapture 2>&1
```

Expected: 测试通过。但注意 `try_read()` 返回 `TryLockResult<RwLockReadGuard>`，需要处理 `unwrap()`。

检查 Rust edition 2024 中 `const` 初始化 `Arc::new(String::new())` 是否允许。如果 const 中不能用 Arc，改为普通测试函数。

```rust
    #[test]
    fn test_file_index_context_new() {
        let ctx: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelRecordResult, ChannelParser> = FileIndexContext::new(ChannelParser {});
        assert!(ctx.position_map_map.is_empty());
        assert!(ctx.obsolete_set.is_empty());
        assert!(ctx.all_obsolete_set.is_empty());
    }
```

（移除 const 定义，简化）

- [ ] **Step 4: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test test_file_index_context_new test_file_index_context_get_lock -- --nocapture 2>&1
```

Expected: 35 passed（累计 35 — config 1 + directory 5 + data 24 + index 5 暂时）

注意：FileIndexContext 的 `get_lock` 测试在验证 lock 可获取后即可，不要实际调用 `read()` / `write()` 的 await（因为是同步测试）。

- [ ] **Step 5: 提交**

```bash
git add src/index.rs
git commit -m "test: index.rs FileIndexContext 测试（3 个）

FileIndexContext::new() 构造 + get_lock 初始状态验证。
将 FileIndexContext 字段改为 pub(crate) 以支持测试内访问。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 11: 最终验证

- [ ] **Step 1: 运行全部测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test 2>&1
```

Expected: 39 passed

```
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

- [ ] **Step 2: 提交最终改动**

```bash
git add src/data.rs src/index.rs
git commit -m "test: kissbot-memory 全量单元测试完成

config(1) + directory(5) + data(24) + index(9)，合计 39 个测试。

Co-Authored-By: deepseek-v4-flash"
```

（实际所有改动应该已经在各 task 中提交了，这一步仅为确保没有遗漏）
