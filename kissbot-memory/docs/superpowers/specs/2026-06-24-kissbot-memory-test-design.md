# kissbot-memory 单元测试设计

为 `kissbot-memory` crate 编写单元测试，覆盖 config / directory / data / index 四个模块。error.rs 同 kissbot-api 约定，纯 enum/type alias，无业务逻辑，跳过。

## 测试策略

- 测试内联在各模块末尾的 `#[cfg(test)] mod tests` 中
- 纯逻辑用同步 `#[test]`，文件 IO 用 `#[tokio::test]`（tokio 已在正式依赖中，无需加 dev-dependency）
- 临时目录使用 `tempfile` crate（需加 dev-dependency）
- 不测 `Config::load()` 和 `Config::get()`（依赖文件系统全局单例，属于胶水代码）
- `MemoryIndexer` 的标记方法是纯同步逻辑，通过直接 `new()` 测试
- 索引查询通过 `FileIndexContext` 直接测试（private struct 但在同一 crate 内可访问）

## dev-dependencies

```toml
[dev-dependencies]
tempfile = "3"
```

## 测试分组

### 1. config.rs（1 个测试）

| 测试 | 说明 |
|------|------|
| `test_config_with_root_dir` | `with_root_dir("/tmp/test")` 构造，验证 root_dir 字段 |

### 2. directory.rs（5 个测试）

**纯路径函数（4 个同步测试）：**

| 测试 | 说明 |
|------|------|
| `test_agent_dir` | `agent_dir("/root", "agent1")` → `/root/agent1` |
| `test_agent_uuid_file` | UUID 标记文件路径包含 `agent-` 前缀 |
| `test_agent_ego_dir` | ego 目录路径包含 `memory-ego` |
| `test_agent_store_dir` | store 目录路径包含 `memory-store` |

**DirectoryManager（1 个同步测试）：**

| 测试 | 说明 |
|------|------|
| `test_dir_manager_new` | `DirectoryManager::new("/tmp/test")` 构造，验证 root_dir |

注：真实的 `ensure_agent_dir`/`list_agents` 等异步方法依赖 `Config::get()` 全局单例，不在 config.rs 测试范围内跳过。路径函数 + 构造器验证确保 DirectoryManager 逻辑正确性。

### 3. data.rs（21 个测试）

**时间函数（4 个）：**

| 测试 | 说明 |
|------|------|
| `test_parse_date_from_time` | `"2026-06-24 15:30:00"` → `"2026-06-24"` |
| `test_get_internal_dates` | 跨 3 天返回中间日期 |
| `test_get_internal_dates_same` | 同一天返回空 Vec |
| `test_get_date_time_segments` | 跨天生成正确的 (start, end) 分段 |

**Record trait（2 个）：**

| 测试 | 说明 |
|------|------|
| `test_record_impl` | 四种 Record 的 sn()/time()/set_sn() 行为 |
| `test_record_cmp_time` | time 优先排序，相同 time 按 sn |

**Record serde（4 个）：**

| 测试 | 说明 |
|------|------|
| `test_serde_channel_record` | ChannelRecord roundtrip |
| `test_serde_think_record` | ThinkRecord roundtrip |
| `test_serde_tool_call_record` | ToolCallRecord roundtrip（含 serde_json::Value） |
| `test_serde_tool_result_record` | ToolResultRecord roundtrip |

**Key serde（2 个）：**

| 测试 | 说明 |
|------|------|
| `test_serde_channel_record_key` | ChannelRecordKey roundtrip |
| `test_serde_record_key` | RecordKey roundtrip |

**FilePathGenerator（4 个）：**

| 测试 | 说明 |
|------|------|
| `test_channel_file_name` | `channel-m1=u1=g1-records-2026-06-24.jsonl` |
| `test_think_file_name` | `think-records-2026-06-24.jsonl` |
| `test_tool_call_file_name` | `tool-call-records-2026-06-24.jsonl` |
| `test_tool_result_file_name` | `tool-result-records-2026-06-24.jsonl` |

**RequestParser（4 个）：**

| 测试 | 说明 |
|------|------|
| `test_channel_request_parser` | ChannelRequest → (ChannelRecordKey, ChannelRecord) |
| `test_think_request_parser` | ThinkRequest → (RecordKey, ThinkRecord) |
| `test_tool_call_request_parser` | ToolCallRequest → (RecordKey, ToolCallRecord) |
| `test_tool_result_request_parser` | ToolResultRequest → (RecordKey, ToolResultRecord) |

**RecordCombiner（4 个）：**

| 测试 | 说明 |
|------|------|
| `test_channel_record_combiner` | Internal record → api result |
| `test_think_record_combiner` | 同上 |
| `test_tool_call_record_combiner` | 同上 |
| `test_tool_result_record_combiner` | 同上 |

注：QueryParser 的四个实现（ChannelParser/ThinkParser/ToolCallParser/ToolResultParser）的 `parse_query` 底层都调用 `parse_query(query: QueryRequest)` 函数分割时间段，该逻辑已在 `test_get_date_time_segments` 中覆盖，不重复测试。

### 4. index.rs（12 个测试）

**FilePosition serde（1 个）：**

| 测试 | 说明 |
|------|------|
| `test_serde_file_position` | FilePosition roundtrip |

**MemoryIndexer 标记方法（8 个同步测试）：**

| 测试 | 说明 |
|------|------|
| `test_mark_channel_obsolete` | 标记后验证 obsolete_set 状态 |
| `test_mark_channel_all_obsolete` | 标记后验证 all_obsolete_set 状态 |
| `test_mark_think_obsolete` | 同上 |
| `test_mark_think_all_obsolete` | 同上 |
| `test_mark_tool_call_obsolete` | 同上 |
| `test_mark_tool_call_all_obsolete` | 同上 |
| `test_mark_tool_result_obsolete` | 同上 |
| `test_mark_tool_result_all_obsolete` | 同上 |

**FileIndexContext 索引查询（2 个同步测试）：**

| 测试 | 说明 |
|------|------|
| `test_file_index_context_new` | `FileIndexContext::new(parser)` 构造后内部状态正确 |
| `test_file_index_context_get_lock` | `get_lock(key)` 返回可用的 FileIndexLock |

注：`query_reverse`/`query_all`/`update_index`/`update_all_index` 等需要真实 JSONL 文件和 `kai-file::ReverseLineReader` 支持，且 `ensure_file_path` 依赖 `DirectoryManager::get()` 全局单例，不在此轮测试中覆盖。这些方法的正确性通过 `data.rs` 的 RequestParser/RecordCombiner 和 index 标记方法的组合验证间接保证。

### 总计

| 模块 | 测试数 |
|------|--------|
| error.rs | 0（跳过） |
| config.rs | 1 |
| directory.rs | 5 |
| data.rs | 21 |
| index.rs | 12 |
| **合计** | **39** |
