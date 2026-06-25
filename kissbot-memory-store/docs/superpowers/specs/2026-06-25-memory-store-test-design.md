# memory-store 单元测试设计

## 概述

为 `kissbot-memory-store` 模块编写单元测试，覆盖 `config.rs` 和 `record.rs` 两个核心文件。采用集成测试风格，使用临时目录操作真实文件系统，与 `kissbot-memory` 的测试风格保持一致。

## 测试范围

| 文件 | 测试内容 |
|------|----------|
| config.rs | Config::load() 从 JSON 文件正确加载配置 |
| record.rs | load_existing_file_state、RecordContext::append_record 各分支 |
| Cargo.toml | 添加 tempfile 作为 dev-dependency |

## 测试结构

遵循项目现有规范，在每个源文件末尾内联测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // ...
}
```

## Config 测试

### load_ok
- 在临时目录创建 config.json，写入有效 JSON
- 调用 Config::load()，验证返回的字段值正确

### load_error
- 调用 Config::load() 指向不存在的文件路径
- 验证返回 Err

## Record 测试

### 辅助函数

一个 `Once` 保护的 `init_test_config()`，在临时目录写一个最小 config.json 并初始化全局 `Config`，确保文件操作有正确的根目录（虽然 `RecordContext` 不直接依赖 Config，但后续如果涉及 `ensure_file_path` 可能间接需要）。

为每个测试创建独立的临时目录，避免测试间状态污染。

### 测试用例

#### 1. `load_existing_file_state` — 文件状态加载

| 测试 | 输入 | 期望 |
|------|------|------|
| `test_load_state_file_not_exists` | 不存在的路径 | sn=0, time="2000-01-01 00:00:00" |
| `test_load_state_empty_file` | 空文件 | sn=0, time="2000-01-01 00:00:00" |
| `test_load_state_with_records` | 含 3 条 ChannelRecord 的文件 | sn=3, time=最后一条的 time |

#### 2. `append_record` — 正常写入

| 测试 | 输入 | 期望 |
|------|------|------|
| `test_append_new_file` | 1 条 ChannelRequest，force=false | 文件被创建，sn=1 |
| `test_append_multiple_records` | 3 条 ChannelRequest | 分配到 sn=1,2,3，文件有 3 行 |
| `test_append_sequential` | 第一次写入，第二次再写入 | 第二次的 sn 从 2 开始递增 |
| `test_append_think_record` | ThinkRequest | 文件被创建，数据正确 |
| `test_append_tool_call_record` | ToolCallRequest | 文件被创建，数据正确 |
| `test_append_tool_result_record` | ToolResultRequest | 文件被创建，数据正确 |

#### 3. `append_record` — 时序检查

| 测试 | 输入 | 期望 |
|------|------|------|
| `test_append_out_of_order_rejected` | 第一条 time="10:00:00"，第二条 time="09:00:00" | 返回 `RecordNotInOrder` |
| `test_append_force_out_of_order` | 同上，force=true | 正常写入，所有记录按 time 重排序 |

#### 4. `append_record` — 多 key 隔离

| 测试 | 输入 | 期望 |
|------|------|------|
| `test_append_multiple_keys` | 2 条不同 agent_id/date 的请求 | 创建 2 个独立文件，sn 各自从 1 开始 |

#### 5. `append_record` — 有已有数据的文件追加 force

| 测试 | 输入 | 期望 |
|------|------|------|
| `test_append_force_with_existing_data` | 先写入 3 条，然后写入 2 条 time 更早的记录 + force | 文件中 5 条记录按 time 排序，sn 按排序后重新编号 |

## 测试依赖

在 Cargo.toml 中添加：

```toml
[dev-dependencies]
tempfile = "3"
```

## 不覆盖的范围

- 文件权限错误（测试环境不可控）
- `MemoryIndexer` 的 `FileHook` 行为（已在 `kissbot-memory` 中覆盖）
- `api.rs` 的 HTTP handler（属于集成测试，后续可单独计划）
- `error.rs`（简单 enum，无逻辑）
- `main.rs`（入口点，无业务逻辑）
