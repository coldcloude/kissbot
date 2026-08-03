# MemoryStoreClient 合并 MemoryWriter 设计

## 目标

将 `MemoryWriter`（kissbot-agent/src/memory_writer.rs）的 think / tool-call / tool-result 推送能力并入 `MemoryStoreClient`（kissbot-agent/src/memory_store_client.rs），删除 MemoryWriter，由 MemoryStoreClient 承载全部 `/store/*` 推送。合并遵循方案 B：每记录类型一个 `FileObjectAppender`，但"生成 requests 实体之后"的 HTTP 发送段只写一遍（共享函数）。

## 结构设计（memory_store_client.rs）

### MemoryStoreClient 结构

```rust
pub struct MemoryStoreClient {
    channel_appender: FileObjectAppender<String, ChannelRecord, MemoryStoreSender, MemoryStoreContext>,
    think_appender: FileObjectAppender<String, ThinkRecord, MemoryStoreSender, ThinkStoreContext>,
    tool_call_appender: FileObjectAppender<String, ToolCallRecord, MemoryStoreSender, ToolCallStoreContext>,
    tool_result_appender: FileObjectAppender<String, ToolResultRecord, MemoryStoreSender, ToolResultStoreContext>,
}
```

- 各 appender 独立 buffer / 独立保序，参数统一：timeout 1s、batch 100（沿用现有常量 RECORD_MAX_DELAY / RECORD_QUEUE_SIZE）
- buffer key 固定常量：channel 沿用 `FILE_KEY = "0"`；think / tool-call / tool-result 分别用 `"think"` / `"tool-call"` / `"tool-result"`
- 现有 `push_channel_record(&self, record: ChannelRecord)` 不变

### 记录类型（agent 侧新增，与 WriteTask 变体字段同构）

```rust
pub struct ThinkRecord {
    pub agent_id: Arc<String>,
    pub role_name: Option<Arc<String>>,   // None → 空串（与 memory-store 无 role 语义一致）
    pub content: Arc<String>,
    pub key: Arc<String>,                  // 固定 ""
    pub time: Arc<String>,
}
pub struct ToolCallRecord { /* agent_id, role_name, tool_name, tool_params(Arc<serde_json::Value>), key, time */ }
pub struct ToolResultRecord { /* agent_id, role_name, tool_result(Arc<serde_json::Value>), key, time */ }
```

### 共享发送段（只写一遍）

```rust
/// POST {base_url}{path}，带 X-Api-Key 鉴权头；base_url 空则跳过（Ok）；
/// 非 2xx 返回 Err（错误含状态码与返回体）
async fn send_store_request(
    client: &reqwest::Client, base_url: &str, api_key: &str,
    path: &str, body: &impl Serialize,
) -> std::result::Result<(), kai_file::Error>
```

统一承载：URL 拼接（`trim_end_matches('/')`）、`X-Api-Key` 头、非 2xx 检查、base_url 空跳过。channel 现有逻辑（memory_store_client.rs 现 write 方法内）挪入此函数，think/tool 的 base_url 空边界一并补齐。

### 各类型 context::write（仅构造 requests 实体，各写一遍）

每类型一个 `XxxStoreContext { client, base_url, api_key }`，实现 `FileAppendWriterContext<String, XxxRecord>`：

- channel：`ChannelRequests { requests, force: 1 }` → `/store/channel`（现有字段映射不变）
- think：`ThinkRequests { requests, force: 1 }` → `/store/think`
- tool-call：`ToolCallRequests { requests, force: 1 }` → `/store/tool-call`
- tool-result：`ToolResultRequests { requests, force: 1 }` → `/store/tool-result`

**force 统一为 1**（用户决定）：跳过 RecordNotInOrder 时间乱序检查，避免秒级时间戳碰撞导致写入被拒。

字段映射遵循 kissbot_api::memory 的 `ChannelRequest` / `ThinkRequest` / `ToolCallRequest` / `ToolResultRequest` 定义（全部 Arc 字段；agent 侧 Option role_name → Arc<String>，None → 空串）。

### context 字段初始化共享

4 个 context 的 `{ client, base_url, api_key }` 初始化相同（ApiConfig / SecurityConfig 来源）——共享一个 factory 函数返回 `(Client, String, String)`。

### 错误处理统一（用户决定）

新增泛型日志 handler（一次实现，各 appender 复用）：

```rust
struct LoggingErrorHandler<K, R>;
#[async_trait]
impl<K, R> ErrorHandler<K, R> for LoggingErrorHandler<K, R> {
    async fn on_write_error(&self, key: &K, _batch: Vec<R>, error: &kai_file::Error) {
        error!("记忆写入失败 key={:?}: {}", key, error);
    }
}
```

channel 的 `NoopErrorHandler` 一并替换——修复 channel 写失败静默丢弃的缺陷。

## coordinator 变化（coordinator.rs）

- 删除 `memory_writer` import、`memory_writer: Arc<MemoryWriter>` 字段、初始化（`MemoryWriter::start()` 与 Arc 包裹）——初始化处改为仅保留 `memory_store_client`
- 步骤 4 改为：

```rust
// 4. 推送 think 到 memory-store（事件模式编码；取记忆用会话保存的 agent_id）
// Think 记忆只存思考内容（方案 A）：有思考内容才写，无则跳过
if let Some(reasoning) = &model_resp.reasoning_content {
    let role_name = memory_role(&session.key);
    self.memory_store_client.push_think(
        session.agent_id.to_string(),
        Some(role_name),
        reasoning.clone(),
        now,
    );
}
```

### 公开方法签名（MemoryStoreClient 新增，返回 ()）

```rust
pub async fn push_think(&self, agent_id: String, role_name: Option<String>, content: String, time: String)
pub async fn push_tool_call(&self, agent_id: String, role_name: Option<String>, tool_name: String, tool_params: serde_json::Value, time: String)  // 本期无调用方
pub async fn push_tool_result(&self, agent_id: String, role_name: Option<String>, tool_result: serde_json::Value, time: String)  // 本期无调用方
```

`push_tool_call` / `push_tool_result` 无调用方，按项目惯例加 `#[allow(dead_code)]`（station 工具调用功能落地时直接调用）。

## types.rs 变化

删除 `WriteTask` 枚举（含 `#[allow(dead_code)]` 的 ToolCall / ToolResult 变体）及其"MemoryWriter 写入队列"注释块。

## 文件删除

- 删除 `kissbot-agent/src/memory_writer.rs`

## 测试

1. 集成：`nexus-ego-chat-store.spec.ts` 的 `assertThinkRecords` 继续端到端验证（think 经新 appender 写入 memory-store，4 场景）
2. 单测（memory_store_client.rs 新增）：
   - 各 `write` 的 requests 实体构造：字段映射正确、force 均为 1
   - `send_store_request` 纯逻辑：base_url 空跳过、path 拼接（HTTP 行为由集成测试覆盖）
3. 现有测试影响：types.rs 测试不涉及 WriteTask；coordinator 测试不涉及 memory_writer——无存量破坏

## 文档

- 本设计文档（docs/superpowers/specs/）
- docs/spec **不修改**（沿用用户约定；其中 WriteTask 定义会与实际代码不一致，如需同步另行指示）

## 不修改

- kai-file 基础库（队列保持无界，用户决定）
- kissbot-api 的请求结构定义
- memory-store 服务端
