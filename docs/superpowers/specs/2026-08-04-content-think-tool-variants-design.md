# Content Think/ToolCall/ToolResult 变体 + think 双字段 + key 关联流程 设计文档

> 子项目 2（依赖子项目 1）：在 out_channel 路由模型之上，加 Content 三变体、think 记忆双字段、key 关联流程，并简化 MemoryStoreClient 接口。子项目 1（out_channel 路由模型）见 `2026-08-04-out-channel-routing-design.md`。

## 1. 背景与目标

### 1.1 问题
当前 think 记忆：`push_think` 把 `reasoning_content`（API 字段与 `<think>` 标签合并）作为单 content 写入，key 传空串；think 记忆与 channel 记忆无关联；MemoryStoreClient 自定义 Record 类型与 API Request 之间有无意义映射层。

### 1.2 目标
- Content 加 Think/ToolCall/ToolResult 三变体（`Arc<String>`，内容为 key），agent 直接生成 ChannelRecord 写入主时间线
- think 记忆拆 `reasoning_content`（API 字段）+ `thinking`（`<think>` 标签）双字段，后续分别处理
- key=UUID 关联 ChannelRecord(Think) 与 ThinkRecord
- MemoryStoreClient push 直接用 `*Request` 作参数，删自定义 Record 与映射函数
- Session 加 role_name/mode 运行态字段，memory_role 从 Session 读（SessionKey 只做去重）

### 1.3 依赖
- think 记录身份字段（messenger_id/user_id/self_user_id/group_id）来自子项目 1 的 out_channel 契约
- 实现顺序：子项目 1 先，子项目 2 后

## 2. 数据模型

### 2.1 Content 加三变体（`kissbot-api/src/message.rs`）

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "msg_type", content = "data")]
pub enum Content {
    Text(Arc<String>),
    Multi(Vec<Content>),
    AttachmentInfo(Arc<AttachmentInfo>),
    AttachmentInfoResponse(Arc<AttachmentInfoResponse>),
    GroupJoin(Arc<GroupChangeNotification>),
    GroupLeave(Arc<GroupChangeNotification>),
    UserRemove(Arc<UserRemoveNotification>),
    // 新增三变体：内容为 key（UUID），关联对应详情记录
    Think(Arc<String>),
    ToolCall(Arc<String>),
    ToolResult(Arc<String>),
}
```

序列化：`{"msg_type":"Think","data":"<uuid>"}` / `"ToolCall"` / `"ToolResult"`。

这三变体**不用于 IncomingMessage/OutgoingMessage**（agent 直接生成 ChannelRecord 写入，非 channel 传输）。

### 2.2 ThinkRequest + 存储层 ThinkRecord 拆双字段（`kissbot-api/src/memory.rs`）

传输层 `ThinkRequest`：
```rust
pub struct ThinkRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub reasoning_content: Arc<String>,   // 原 content 拆分：API 字段解析的内容
    pub thinking: Arc<String>,            // 原 content 拆分：<think> 标签解析的内容（去标签）
    pub key: Arc<String>,
    pub time: Arc<String>,
}
```

存储层 `ThinkRecord`（memory-store 服务端序列化用）同步拆：
```rust
pub struct ThinkRecord {
    pub reasoning_content: Arc<String>,
    pub thinking: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}
```

memory-store 服务端靠 serde 自动适配（`Record` trait 只用 `time`/`sn`，业务字段加对服务端透明；实现时验证）。

`Option<String>` -> `Arc<String>` 转换：None -> 空串（`unwrap_or_default()`），与既有 role_name 模式一致。

### 2.3 Session 加 role_name/mode 运行态字段（`kissbot-agent/src/session_manager.rs`）

```rust
pub struct Session {
    pub key: SessionKey,           // 只做去重（HashMap key）
    pub role_name: String,         // 运行态：从 key 复制，业务读取源
    pub mode: Mode,                // 运行态：从 key 复制，业务读取源
    pub context: tokio::sync::Mutex<SessionContext>,
    pub agent_id: Arc<String>,
    pub model: ArcSwap<Option<ProviderModel>>,
    // ...
}
```

get_or_create 时 `session.role_name = key.role_name.clone()`、`session.mode = key.mode.clone()`。

### 2.4 memory_role 签名改（`kissbot-agent/src/types.rs`）

```rust
// 原：pub fn memory_role(key: &SessionKey) -> String
pub fn memory_role(role_name: &str, mode: &Mode) -> String {
    match mode {
        Mode::Event(event_id) => format!("{}-{}", role_name, event_id),
        Mode::Role => role_name.to_string(),
    }
}
```

3 处调用改从 Session 字段读：`memory_role(&session.role_name, &session.mode)`（coordinator incoming 推 channel 记录、think 写入、send_outgoing 推 channel 记录）。

## 3. provider ModelResponse 拆分（`kissbot-agent/src/provider.rs`）

### 3.1 ModelResponse 加 thinking 字段
```rust
pub struct ModelResponse {
    pub content: String,
    pub reasoning_content: Option<String>,   // API 字段（message.reasoning_content / anthropic thinking block）
    pub thinking: Option<String>,            // <think> 标签内容（strip_think_tag 返回）
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
}
```

### 3.2 解析拆分（不合并）
当前 `reasoning_content = api_reasoning.filter(non_empty).or(tag_reasoning)`（合并互斥）。改为两字段独立：

- `parse_openai_response`：
  - `reasoning_content = choice["message"]["reasoning_content"].as_str().map(String::from).filter(|s| !s.is_empty())`
  - `(content, tag_reasoning) = strip_think_tag(&content)`
  - `thinking = tag_reasoning.filter(|s| !s.is_empty())`
- `parse_anthropic_response`：
  - `reasoning_content = thinking_block.thinking.filter(non_empty)`（content blocks 中 type=="thinking"）
  - `(content, tag_reasoning) = strip_think_tag(&content)`
  - `thinking = tag_reasoning.filter(non_empty)`

两字段独立，都可能 Some/None。空串视为 None（filter non_empty）。`strip_think_tag` 仍总剥离 `<think>` 标签。

### 3.3 测试调整
现有测试测合并行为（`.or()`）-> 改测两字段独立：
- `parse_openai_response_extracts_reasoning_content`：reasoning_content=Some，thinking=None
- `parse_openai_response_falls_back_to_think_tag` -> 改名/拆分：API 无 reasoning_content + `<think>` 标签 -> reasoning_content=None，thinking=Some
- 新增：API 有 reasoning_content + content 有 `<think>` 标签 -> 两字段都 Some（独立共存）
- anthropic 同理

## 4. MemoryStoreClient push 用 *Request（`kissbot-agent/src/memory_store_client.rs`）

### 4.1 删自定义 Record
删除 agent 侧 `ChannelRecord`/`ThinkRecord`/`ToolCallRecord`/`ToolResultRecord` 四个结构 + `channel_request`/`think_request`/`tool_call_request`/`tool_result_request` 映射函数。

### 4.2 push 方法直接用 *Request
```rust
pub async fn push_channel_record(&self, record: ChannelRequest) { ... }
pub async fn push_think(&self, record: ThinkRequest) { ... }
#[allow(dead_code)]
pub async fn push_tool_call(&self, record: ToolCallRequest) { ... }
#[allow(dead_code)]
pub async fn push_tool_result(&self, record: ToolResultRequest) { ... }
```

### 4.3 write 直接构造 XxxRequests
```rust
async fn write(&mut self, _key: &String, records: Vec<ThinkRequest>) -> Result<(), kai_file::Error> {
    send_store_request(&self.client, &self.base_url, &self.api_key, "/store/think",
        &ThinkRequests { requests: records, force: 1 }).await
}
```
records 已是 `Vec<XxxRequest>`，无需映射。`FileObjectAppender` 的 R 泛型用 `*Request`。

## 5. think 写入流程（coordinator run_agentic_loop）

### 5.1 流程（步骤 4，send_outgoing 前）
```
let model_resp = ...;  // ModelResponse { content, reasoning_content, thinking, ... }
let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

// think 记忆：reasoning_content 或 thinking 任一有值才写
if model_resp.reasoning_content.is_some() || model_resp.thinking.is_some() {
    let key = uuid::Uuid::new_v4().to_string();
    let role_name = memory_role(&session.role_name, &session.mode);

    // 1. ChannelRecord(Think(key)) 写主时间线（身份来自 out_channel）
    self.memory_store_client.push_channel_record(ChannelRequest {
        agent_id: session.agent_id.clone(),
        role_name: Arc::new(role_name.clone()),
        messenger_id: out_channel.user.messenger_id.clone(),
        user_id: out_channel.user.user_id.clone(),
        self_user_id: out_channel.user.user_id.clone(),
        group_id: out_channel.group_id.clone(),
        is_self: 1,
        messenger_name: ..., user_name: ..., group_name: ...,  // 取自 out_channel 渠道或 response
        content: Content::Think(Arc::new(key.clone())),
        time: now.clone(),
    }).await;

    // 2. ThinkRecord(key, reasoning_content, thinking) 写详情
    self.memory_store_client.push_think(ThinkRequest {
        agent_id: session.agent_id.clone(),
        role_name: Arc::new(role_name),
        reasoning_content: Arc::new(model_resp.reasoning_content.clone().unwrap_or_default()),
        thinking: Arc::new(model_resp.thinking.clone().unwrap_or_default()),
        key: Arc::new(key),
        time: now,
    }).await;
}
// 都 None 则跳过（不写 ChannelRecord 也不写 ThinkRecord）
```

### 5.2 时序与原子性
- think 在 send_outgoing 前（步骤 4 在步骤 5 前）
- ChannelRecord(Think) 与 ThinkRecord 各自经 appender 异步入库，时序不严格（无原子性保证）
- key=UUID 关联两者；time 用同一 `now`（Local）
- 身份字段（messenger_id/user_id/self_user_id/group_id）来自 out_channel（子项目 1 契约）

### 5.3 messenger_name/user_name/group_name 来源
ChannelRecord(Think) 的 name 字段：out_channel 指向的 channel 配置中的名称（messenger_name/user_name/group_name）。若 out_channel 配置未携带名称，用空串（详情经 key 关联，name 非关键）。

## 6. ToolCall/ToolResult（本期无生成源）

流程与 Think 同构（station 工具调用落地时消费）：
```
key = Uuid::new_v4()
push_channel_record(ChannelRequest { content: Content::ToolCall(key), 身份=out_channel, is_self=1, ... })
push_tool_call(ToolCallRequest { tool_name, tool_params, key, ... })
```
本期 `push_tool_call`/`push_tool_result` 无调用方，保留 `#[allow(dead_code)]`。

## 7. 边界处理

### 7.1 写入条件
`reasoning_content.is_some() || thinking.is_some()` 任一有值才写 ChannelRecord(Think) + ThinkRecord；都 None 跳过。空串视为 None（provider 解析时 filter non_empty）。

### 7.2 ThinkRecord 空字段
一个字段有值、另一字段 None 时，ThinkRequest 对应字段为空串（`unwrap_or_default`）。存储层记录空串表示无内容，读取侧后续区分。

### 7.3 读取侧
本期只写入，不实现读取（think 记忆进 agent 上下文、ego 处理）。读取侧后续另出设计。

## 8. 涉及文件

| 文件 | 改动 |
|------|------|
| `kissbot-api/src/message.rs` | Content 加 Think/ToolCall/ToolResult 三变体 |
| `kissbot-api/src/memory.rs` | ThinkRequest + 存储层 ThinkRecord content 拆 reasoning_content + thinking |
| `kissbot-agent/src/session_manager.rs` | Session 加 role_name + mode 运行态字段；get_or_create 同步 |
| `kissbot-agent/src/types.rs` | memory_role 签名改 `(role_name: &str, mode: &Mode)`；测试调整 |
| `kissbot-agent/src/provider.rs` | ModelResponse 加 thinking 字段；parse 函数拆分（不合并）；测试调整 |
| `kissbot-agent/src/memory_store_client.rs` | 删自定义 Record + 映射函数；push 用 *Request；appender R 用 *Request |
| `kissbot-agent/src/coordinator.rs` | think 写入流程（步骤 4）；memory_role 3 处调用改从 Session 读；push_think(ThinkRequest) |

## 9. 测试策略

### 9.1 单测
- **message.rs**：Content::Think/ToolCall/ToolResult 序列化/反序列化 roundtrip
- **memory.rs**：ThinkRequest/ThinkRecord 双字段 serde
- **session_manager**：Session role_name/mode 字段从 key 复制
- **types.rs**：memory_role(role_name, mode) 编码（角色/事件）
- **provider.rs**：
  - reasoning_content（API 字段）独立提取
  - thinking（`<think>` 标签）独立提取
  - 两字段共存（API 有 + 标签有）
  - 都 None（无思考）
  - `<think>` 标签总剥离
- **memory_store_client**：push 用 *Request；write 构造 XxxRequests force=1；base_url 空跳过

### 9.2 集成测试
- assertThinkRecords 适配双字段：查询 think 记录，断言 reasoning_content/thinking 字段（而非单 content）
- think 记忆端到端：模型返回 reasoning_content -> ChannelRecord(Think) + ThinkRecord 写入，key 关联
- 无思考内容（reasoning_content + thinking 都 None）-> 不写 think 记忆

## 10. 范围说明

本 spec 覆盖子项目 2，依赖子项目 1 的 out_channel 契约（身份字段来源）。读取侧（think 记忆进上下文、ego 处理）不在本期，后续另出设计。
