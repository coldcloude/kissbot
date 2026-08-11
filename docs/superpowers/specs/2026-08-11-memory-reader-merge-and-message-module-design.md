# MemoryReader 并入 MemoryStoreClient + message 模块提取设计

日期：2026-08-11

## 背景与目标

### 现状问题

1. **MemoryReader 与 StoreHttpConfig 功能重复**：`MemoryReader { client: reqwest::Client, store_url: String }` 与 `StoreHttpConfig { client, base_url, api_key }` 是两套并存的 HTTP 配置（前者读全局 ApiConfig，后者读 ApiConfig + SecurityConfig）；读取功能（`read_recent_for_context`）与写入功能（MemoryStoreClient 的 push 系列）分散在两个结构体。
2. **消息打包代码散落两处**：`pack_memory_messages`（MemoryMsg 列表 → 交替 User/Assistant 序列）在 memory_reader.rs；batch → 单条 User content 的拼接（逐行 `name: text`）内联在 session_manager 的 BatchConsumer.try_flush，用的是 coordinator 的 `extract_text`。两者 is_self=0 的格式化逻辑相同却各写一份。
3. **read_memory_struct_index 是 dead code**：`#[allow(dead_code)]` 的 memory-struct 占位，调用方静默忽略结果。

### 本次目标

1. 删除 MemoryReader：读取功能并入 MemoryStoreClient（共享同一 `Arc<StoreHttpConfig>`），HTTP 细节归 StoreHttpConfig。
2. 新建 message.rs 模块：MessageContent（原 MemoryMsg 改名）+ 文本提取 + 打包（记忆交替序列 + batch 单条 User）集中一处，BatchConsumer 复用。
3. `read_memory_struct_index` 删除（memory-struct 未实现，roadmap 实现时再加）。

## 一、StoreHttpConfig 读方法 + MemoryStoreClient 合并（kissbot-agent/src/memory_store_client.rs）

- `StoreHttpConfig` 新增读方法（与写方法 `send_store_request` 共用发送内核：base_url 空跳过、X-Api-Key 头、非 2xx 报错）：

```rust
/// 查询请求：POST {base_url}{path}，返回反序列化后的响应体（读路径要 data，写方法丢弃 body）
async fn send_store_query<T: serde::de::DeserializeOwned>(
    &self, path: &str, body: &impl Serialize,
) -> std::result::Result<T, kai_file::Error>
```

- `MemoryStoreClient` 增加字段 `config: Arc<StoreHttpConfig>`（与 4 个 context 共享同一 Arc；`new()` 构造时 clone 一份存 self）。
- `read_recent_for_context` 移入 `MemoryStoreClient`（签名不变，两次查询的 HTTP 经 `config.send_store_query` 发，错误仍映射为 `crate::types::Error::MemoryStoreError`）：

```rust
pub async fn read_recent_for_context(
    &self,
    agent_id: Arc<String>,
    role_name: Arc<String>,
    cfg: &EffectiveContextConfig,
) -> Result<Vec<MessageContent>>
```

- `MemoryReader` 结构体、`read_memory_struct_index`、测试缝 `reader_at` 全部删除。
- 测试：read 相关 11 个测试迁入 memory_store_client.rs 的 tests 模块（同文件可字面量构造 `StoreHttpConfig`/`MemoryStoreClient` 注入 mock store 地址，避开全局配置——现有 `send_store_request_skips_empty_base_url` 已用此方式）。

## 二、message.rs 模块（kissbot-agent/src/message.rs，新建）

```rust
/// 消息内容（channel record 的最小视图：name + 文本段 + is_self；id 类不保留；时间由排序键承担，不保留）
pub struct MessageContent {
    pub user_name: Arc<String>,
    /// 文本段列表：每个元素为 Content::Text 的 Arc 克隆（Multi 拆多个元素；打包时以 \n 拼接）
    pub content: Vec<Arc<String>>,
    /// 是否 agent 自身消息（is_self：1=self，0=他人）
    pub is_self: bool,
}

/// parse_channel_groups 与 pack_batch 共用的 MessageContent 构造：
/// user_name 克隆 Arc；content 递归提取 Text 段（collect_text_parts）；is_self 按 >0 转 bool
pub fn extract_content(user_name: &Arc<String>, is_self: u32, content: &Content) -> MessageContent
```

- `collect_text_parts`（递归提取，从 memory_reader 移入）降为**私有**（只被 extract_content 调用）。
- `user_line` 私有辅助：is_self=0 单行格式（name 空只留 content，否则 `"name: content"`）——pack_memory_messages 的 is_self=0 分支与 pack_batch 共用：

```rust
fn user_line(m: &MessageContent) -> String
```

- `pack_memory_messages(&[MessageContent]) -> Vec<Message>`：从 memory_reader 移入，行为不变（空 content 跳过、前导 self 丢弃、同 is_self 合并、User 结尾补空 Assistant）；is_self=0 分支改用 `user_line`。
- `pack_batch`：batch → 单条 User Message（替换 BatchConsumer 内联）：

```rust
/// 将一批 IncomingMessageEvent 打包为一条 User Message：
/// 每 event 经 extract_content 构造（is_self=0），空 content（非文本）跳过，逐行 user_line 拼接；
/// 全部跳过时返回空 content 的 User（try_flush 已在 items 为空时提前返回，此处输入必非空）
pub fn pack_batch(events: &[Arc<IncomingMessageEvent>]) -> Message
```

- 不做 `IncomingMessageEvent → ChannelRecord` 转换（用户否决）：事件直接构造 MessageContent，仅复用 extract_content / collect_text_parts / user_line / 空 content 跳过这些可复用部分。

## 三、接线

### session_manager.rs（BatchConsumer.try_flush）

- 删除 `use crate::coordinator::{AgentCoordinator, extract_text}` 中的 extract_text、内联拼接循环；
- 改调 `crate::message::pack_batch(&items)`，取 `Message::User` 的 content 喂给 `session.accept_batch(content)`。

### coordinator.rs

- 删除 `memory_reader: Arc<MemoryReader>` 字段、`MemoryReader::new()` 构造、`use crate::memory_reader::{MemoryReader, pack_memory_messages}` 中 MemoryReader；
- `build_role_context` 改调 `self.memory_store_client.read_recent_for_context(...)`（map_or_else 逻辑不变）；
- 删除 `ensure_session` 中 `read_memory_struct_index` 调用块（含"顶层记忆索引（memory-struct 未实现时静默跳过）"注释）；
- `extract_text` **保留**（`handle_incoming` 仍在使用）。

### main.rs

- `mod memory_reader;` 删除；新增 `mod message;`。

## 四、测试

- memory_reader.rs 的 11 个 read 测试 → memory_store_client.rs tests；
- 4 个 pack 测试 → message.rs tests；
- 新增：`pack_batch` 拼接 / 空 name / 非文本跳过 / 全跳过空 content 测试；`extract_content` 构造测试（is_self>0、Multi 提取）；
- 全量回归（4 个 crate），clippy 不新增 warning。

## 五、文档

- `docs/spec/kissbot-agent-modules.md`：memory_reader 行/组件图/时序图/表格改为 memory_store_client 承担读取；`docs/spec/kissbot-agent-nexus.md`："读取（MemoryReader）"、"记忆索引读取（MemoryReader → Memory-Struct）"节更新。
