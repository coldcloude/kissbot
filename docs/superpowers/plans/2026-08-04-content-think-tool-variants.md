# Content Think/ToolCall/ToolResult 变体 + think 双字段 + key 关联流程 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Content 加 Think/ToolCall/ToolResult 三变体（key=UUID），think 记忆拆 reasoning_content+thinking 双字段，key 关联 ChannelRecord(Think) 与 ThinkRecord，MemoryStoreClient push 直接用 *Request，Session 加 role_name/mode 运行态字段。

**Architecture:** provider 解析拆 reasoning_content（API 字段）+ thinking（`<think>` 标签）两独立字段；coordinator 步骤 4（send_outgoing 前）任一有值则生成 UUID key -> push ChannelRecord(Content::Think(key)，身份来自 out_channel) -> push ThinkRequest(双字段, key)；MemoryStoreClient 删自定义 Record 与映射函数，push 直接用 kissbot-api 的 *Request；Session 加 role_name/mode 字段（与 key 解耦），memory_role 从 Session 读。

**Tech Stack:** Rust（tokio / serde / uuid），Playwright 集成测试。

**依赖：** 子项目 1（out_channel 路由模型）的 out_channel 契约--think 记录身份字段（messenger_id/user_id/self_user_id/group_id）来自 out_channel。实现顺序子项目 1 先。

## Global Constraints

- **不删除代码注释**（项目约定）
- **依赖子项目 1**：Task 4 think 流程消费 `out_channel: &OutChannel`（子项目 1 产出 `OutChannel { channel_id, user: ChannelUser, group_id }` 与 `send_outgoing`）；若子项目 1 未实现，Task 4 无法独立运行
- **ThinkRequest + 存储层 ThinkRecord 都拆双字段**：`content` -> `reasoning_content` + `thinking`；memory-store 服务端 serde 自动适配（实现时验证 `Record` trait 只用 time/sn）
- **Option<String> -> Arc<String>**：None -> 空串（`unwrap_or_default()`），与既有 role_name 模式一致
- **空串视为 None**：provider 解析时 `.filter(|s| !s.is_empty())`；写入条件 `reasoning_content.is_some() || thinking.is_some()`
- **只写入，不实现读取**（think 记忆进上下文/ego 处理后续另出设计）
- **`<think>` 标签总剥离**（strip_think_tag 不变）
- 文本 UTF-8/LF；commit 中文且覆盖全部改动

---

### Task 1: 数据模型（Content 三变体 + ThinkRequest/ThinkRecord 双字段 + Session role_name/mode + memory_role 签名）

**Files:**
- Modify: `kissbot-api/src/message.rs`（Content 加三变体 + 测试）
- Modify: `kissbot-api/src/memory.rs`（ThinkRequest + 存储层 ThinkRecord content 拆双字段 + 测试）
- Modify: `kissbot-agent/src/session_manager.rs`（Session 加 role_name + mode 字段；get_or_create 同步）
- Modify: `kissbot-agent/src/types.rs`（memory_role 签名改 + 测试）

**Interfaces:**
- Consumes: `kissbot_api::Content`（现有）、`kissbot_api::memory::ThinkRequest/ThinkRecord`（现有）、`SessionKey`/`Mode`（现有）
- Produces: `Content::Think(Arc<String>)`/`ToolCall(Arc<String>)`/`ToolResult(Arc<String>)`；`ThinkRequest { agent_id, role_name, reasoning_content, thinking, key, time }`；存储层 `ThinkRecord { reasoning_content, thinking, key, time, sn }`；`Session { key, role_name: String, mode: Mode, context, agent_id, model }`；`memory_role(role_name: &str, mode: &Mode) -> String`

- [ ] **Step 1: 写失败测试（Content 三变体 serde roundtrip）**

`kissbot-api/src/message.rs` 测试模块追加：
```rust
#[test]
fn test_serde_content_think_tool_variants() {
    let think = Content::Think(Arc::new("uuid-1".to_string()));
    let json = serde_json::to_value(&think).unwrap();
    assert_eq!(json, serde_json::json!({"msg_type":"Think","data":"uuid-1"}));
    assert_eq!(serde_json::from_value::<Content>(json).unwrap(), think);

    let call = Content::ToolCall(Arc::new("uuid-2".to_string()));
    let j = serde_json::to_value(&call).unwrap();
    assert_eq!(j["msg_type"], "ToolCall");
    assert_eq!(j["data"], "uuid-2");

    let result = Content::ToolResult(Arc::new("uuid-3".to_string()));
    let j = serde_json::to_value(&result).unwrap();
    assert_eq!(j["msg_type"], "ToolResult");
    assert_eq!(j["data"], "uuid-3");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-api && cargo test test_serde_content_think_tool_variants`
Expected: 编译错误（Content 无 Think/ToolCall/ToolResult 变体）。

- [ ] **Step 3: Content 加三变体**

`kissbot-api/src/message.rs` Content 枚举追加：
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
    // 新增三变体：内容为 key（UUID），关联对应详情记录（agent 直接生成 ChannelRecord，不用于 IncomingMessage/OutgoingMessage）
    Think(Arc<String>),
    ToolCall(Arc<String>),
    ToolResult(Arc<String>),
}
```

- [ ] **Step 4: 写失败测试（ThinkRequest/ThinkRecord 双字段 serde）**

`kissbot-api/src/memory.rs` 测试模块追加：
```rust
#[test]
fn test_serde_think_request_dual_fields() {
    let obj = ThinkRequest {
        agent_id: Arc::new("a1".to_string()),
        role_name: Arc::new("r1".to_string()),
        reasoning_content: Arc::new("推理".to_string()),
        thinking: Arc::new("思考".to_string()),
        key: Arc::new("k1".to_string()),
        time: Arc::new("2026-01-01 00:00:00".to_string()),
    };
    let json = serde_json::to_value(&obj).unwrap();
    let back: ThinkRequest = serde_json::from_value(json).unwrap();
    assert_eq!(*back.reasoning_content, "推理");
    assert_eq!(*back.thinking, "思考");
    assert!(json.get("content").is_none(), "content 字段已拆分");
}

#[test]
fn test_serde_think_record_dual_fields() {
    let obj = ThinkRecord {
        reasoning_content: Arc::new("推理".to_string()),
        thinking: Arc::new("".to_string()),
        key: Arc::new("k1".to_string()),
        time: Arc::new("2026-01-01 00:00:00".to_string()),
        sn: 1,
    };
    let json = serde_json::to_value(&obj).unwrap();
    let back: ThinkRecord = serde_json::from_value(json).unwrap();
    assert_eq!(*back.reasoning_content, "推理");
    assert_eq!(back.sn, 1);
}
```

- [ ] **Step 5: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-api && cargo test test_serde_think_request_dual_fields`
Expected: 编译错误（ThinkRequest/ThinkRecord 无 reasoning_content/thinking 字段，有 content）。

- [ ] **Step 6: ThinkRequest + ThinkRecord 拆双字段**

`kissbot-api/src/memory.rs`：
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub reasoning_content: Arc<String>,   // 原 content 拆分：API 字段解析的内容
    pub thinking: Arc<String>,            // 原 content 拆分：<think> 标签解析的内容（去标签）
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecord {
    pub reasoning_content: Arc<String>,
    pub thinking: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}
```
同步修改 `test_serde_think_request`/`test_serde_think_requests`/`test_serde_think_record`/`test_record_impl`/`test_record_cmp_time` 等既有测试中 ThinkRequest/ThinkRecord 的 `content` 字段为 `reasoning_content` + `thinking`。

- [ ] **Step 7: 写失败测试（memory_role 签名 + Session role_name/mode）**

`kissbot-agent/src/types.rs` 测试模块 `memory_role_encodes_event_only` 改为：
```rust
#[test]
fn memory_role_encodes_event_only() {
    assert_eq!(memory_role("dev", &Mode::Role), "dev");
    assert_eq!(memory_role("dev", &Mode::Event("e1".into())), "dev-e1");
}
```

`kissbot-agent/src/session_manager.rs` 测试模块追加（需 `use crate::types::{Mode, SessionKey};`）：
```rust
#[test]
fn session_copies_role_name_and_mode_from_key() {
    let key = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
    let model = Arc::new(Some(ProviderModel { provider: "p".into(), model: "m".into() }));
    let agent_id = Arc::new("uuid".to_string());
    let session = Session::new(key.clone(), model, agent_id);
    assert_eq!(session.role_name, "r1");
    assert_eq!(session.mode, Mode::Event("e1".into()));
}
```

- [ ] **Step 8: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test memory_role_encodes_event_only`
Expected: 编译错误（memory_role 签名不匹配；Session 无 role_name/mode 字段、无 new 方法或签名不符）。

- [ ] **Step 9: memory_role 签名改**

`kissbot-agent/src/types.rs`：
```rust
// 原：pub fn memory_role(key: &SessionKey) -> String
/// 记忆读写边界的 role 编码：事件模式拼 {role}-{event}（对 memory-store 透明），角色模式原样
/// role_name/mode 从 Session 运行态字段读（SessionKey 只做去重）
pub fn memory_role(role_name: &str, mode: &Mode) -> String {
    match mode {
        Mode::Event(event_id) => format!("{}-{}", role_name, event_id),
        Mode::Role => role_name.to_string(),
    }
}
```

- [ ] **Step 10: Session 加 role_name/mode 字段**

`kissbot-agent/src/session_manager.rs` Session 结构与构造改：
```rust
pub struct Session {
    pub key: SessionKey,           // 只做去重（HashMap key）
    pub role_name: String,         // 运行态：从 key 复制，业务读取源
    pub mode: Mode,                // 运行态：从 key 复制，业务读取源
    pub context: tokio::sync::Mutex<SessionContext>,
    pub agent_id: Arc<String>,
    pub model: arc_swap::ArcSwap<Option<ProviderModel>>,
}

impl Session {
    pub fn new(key: SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> Self {
        let role_name = key.role_name.clone();
        let mode = key.mode.clone();
        Self {
            role_name,
            mode,
            context: tokio::sync::Mutex::new(SessionContext::new()),
            model: arc_swap::ArcSwap::from_pointee(model),
            agent_id,
            key,
        }
    }
}
```
（若现有 get_or_create 直接构造 Session 字面量，改为调 `Session::new(key, model, agent_id)`；保留原有字段初始化逻辑。）

- [ ] **Step 11: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test`
Expected: kissbot-api + kissbot-agent 全部 PASS（新测试 + 既有测试适配后）。注意：coordinator.rs 仍有 3 处 `memory_role(&session.key)` / `memory_role(&key)` 调用编译失败--本步暂用 `memory_role(&session.key.role_name, &session.key.mode)` 临时适配（Task 4 改为从 Session 字段读），使编译通过。

- [ ] **Step 12: 提交**

```bash
cd /home/admin/project/kissbot
git add kissbot-api/src/message.rs kissbot-api/src/memory.rs kissbot-agent/src/session_manager.rs kissbot-agent/src/types.rs kissbot-agent/src/coordinator.rs
git commit -m "feat(api,agent): 数据模型--Content 加 Think/ToolCall/ToolResult 三变体(key=UUID)；ThinkRequest+存储层 ThinkRecord content 拆 reasoning_content+thinking 双字段；Session 加 role_name+mode 运行态字段(与 key 解耦)；memory_role 签名改 (role_name,mode)"
```

---

### Task 2: provider ModelResponse 拆分（reasoning_content + thinking 独立）

**Files:**
- Modify: `kissbot-agent/src/provider.rs`（ModelResponse 加 thinking；parse 函数拆分；测试改）

**Interfaces:**
- Consumes: `strip_think_tag`（现有，不变）
- Produces: `ModelResponse { content, reasoning_content: Option<String>, thinking: Option<String>, tool_calls, finish_reason }`（两字段独立，不合并）

- [ ] **Step 1: 写失败测试（两字段独立）**

`kissbot-agent/src/provider.rs` 测试模块追加/改：
```rust
#[test]
fn parse_openai_response_reasoning_and_thinking_independent() {
    // API 有 reasoning_content + content 有 <think> 标签 -> 两字段都 Some（独立共存）
    let data = serde_json::json!({
        "choices": [{ "message": { "content": "<think>标签思考</think>答案", "reasoning_content": "API推理" }, "finish_reason": "stop" }]
    });
    let resp = parse_openai_response(&data);
    assert_eq!(resp.content, "答案", "<think> 标签应剥离");
    assert_eq!(resp.reasoning_content.as_deref(), Some("API推理"), "reasoning_content 独立取 API 字段");
    assert_eq!(resp.thinking.as_deref(), Some("标签思考"), "thinking 独立取标签内容");
}

#[test]
fn parse_openai_response_only_thinking_when_no_api_field() {
    // 无 API reasoning_content + <think> 标签 -> reasoning_content=None, thinking=Some
    let data = serde_json::json!({
        "choices": [{ "message": { "content": "<think>思考</think>答案" }, "finish_reason": "stop" }]
    });
    let resp = parse_openai_response(&data);
    assert_eq!(resp.reasoning_content, None);
    assert_eq!(resp.thinking.as_deref(), Some("思考"));
}

#[test]
fn parse_anthropic_response_reasoning_and_thinking_independent() {
    let data = serde_json::json!({
        "content": [
            { "type": "thinking", "thinking": "API推理" },
            { "type": "text", "text": "<think>标签思考</think>答复" }
        ],
        "stop_reason": "end_turn"
    });
    let resp = parse_anthropic_response(&data);
    assert_eq!(resp.content, "答复");
    assert_eq!(resp.reasoning_content.as_deref(), Some("API推理"));
    assert_eq!(resp.thinking.as_deref(), Some("标签思考"));
}

#[test]
fn parse_response_no_thinking_when_both_empty() {
    let data = serde_json::json!({
        "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
    });
    let resp = parse_openai_response(&data);
    assert_eq!(resp.reasoning_content, None);
    assert_eq!(resp.thinking, None);
}
```
修改既有 `parse_openai_response_extracts_reasoning_content`：断言 `resp.thinking` 为 None（API 字段无标签时）。修改 `parse_openai_response_falls_back_to_think_tag` 与 `parse_openai_response_empty_api_reasoning_falls_back_to_think_tag`：改为断言 `resp.reasoning_content == None`、`resp.thinking == Some("思考")`（不再合并到 reasoning_content）。anthropic 两个 fallback 测试同理改。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test parse_openai_response_reasoning_and_thinking_independent`
Expected: 编译错误（ModelResponse 无 thinking 字段）。

- [ ] **Step 3: ModelResponse 加 thinking 字段**

`kissbot-agent/src/provider.rs`：
```rust
#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub content: String,
    /// 思考内容：API 字段（DeepSeek reasoning_content / anthropic thinking block）
    pub reasoning_content: Option<String>,
    /// 思考内容：<think> 标签解析（去标签）；与 reasoning_content 独立，不合并
    pub thinking: Option<String>,
    #[allow(dead_code)]
    pub tool_calls: Vec<ToolCall>,
    #[allow(dead_code)]
    pub finish_reason: String,
}
```

- [ ] **Step 4: parse_openai_response 拆分**

```rust
fn parse_openai_response(data: &serde_json::Value) -> ModelResponse {
    let choice = &data["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("").to_string();
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop").to_string();
    // reasoning_content：API 字段，空串视为 None
    let reasoning_content = choice["message"]["reasoning_content"].as_str()
        .map(String::from).filter(|s| !s.is_empty());
    // <think> 标签总剥离；thinking 独立取标签内容，空串视为 None
    let (content, tag_reasoning) = strip_think_tag(&content);
    let thinking = tag_reasoning.filter(|s| !s.is_empty());
    ModelResponse { content, reasoning_content, thinking, tool_calls: Vec::new(), finish_reason }
}
```

- [ ] **Step 5: parse_anthropic_response 拆分**

```rust
fn parse_anthropic_response(data: &serde_json::Value) -> ModelResponse {
    // reasoning_content：thinking block 内容（空串视为 None）
    let mut reasoning_content = None;
    let mut content = String::new();
    if let Some(blocks) = data["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("thinking") if reasoning_content.is_none() => {
                    reasoning_content = block["thinking"].as_str().map(String::from).filter(|s| !s.is_empty());
                }
                Some("text") if content.is_empty() => {
                    content = block["text"].as_str().unwrap_or("").to_string();
                }
                _ => {}
            }
        }
    }
    let finish_reason = data["stop_reason"].as_str().unwrap_or("end_turn").to_string();
    // <think> 标签总剥离；thinking 独立取标签内容
    let (content, tag_reasoning) = strip_think_tag(&content);
    let thinking = tag_reasoning.filter(|s| !s.is_empty());
    ModelResponse { content, reasoning_content, thinking, tool_calls: Vec::new(), finish_reason }
}
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: provider 测试全部 PASS（拆分测试 + 既有适配）。

- [ ] **Step 7: 提交**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/provider.rs
git commit -m "refactor(provider): ModelResponse 拆分--reasoning_content(API 字段)与 thinking(<think> 标签)独立不再合并；parse_openai/anthropic_response 拆分两字段(空串视为 None)；strip_think_tag 不变；测试改+新增两字段共存用例"
```

---

### Task 3: MemoryStoreClient push 用 *Request（删自定义 Record）

**Files:**
- Modify: `kissbot-agent/src/memory_store_client.rs`（删自定义 Record + 映射函数；push 用 *Request；appender R 用 *Request）

**Interfaces:**
- Consumes: `kissbot_api::memory::{ChannelRequest, ChannelRequests, ThinkRequest, ThinkRequests, ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests}`（Task 1 后 ThinkRequest 双字段）
- Produces: `push_channel_record(ChannelRequest)` / `push_think(ThinkRequest)` / `push_tool_call(ToolCallRequest)` / `push_tool_result(ToolResultRequest)`（后两者 `#[allow(dead_code)]`）

- [ ] **Step 1: 写失败测试（push 用 *Request + force=1）**

`kissbot-agent/src/memory_store_client.rs` 测试模块改（替换现有 think_request_maps_fields 等测自定义 Record 的测试）：
```rust
#[test]
fn think_requests_force_is_one() {
    let reqs = think_requests(vec![ThinkRequest {
        agent_id: Arc::new("a1".into()),
        role_name: Arc::new("r1".into()),
        reasoning_content: Arc::new("推理".into()),
        thinking: Arc::new(String::new()),
        key: Arc::new("k1".into()),
        time: Arc::new("2026-08-04 10:00:00".into()),
    }]);
    assert_eq!(reqs.force, 1, "force 统一为 1");
    assert_eq!(reqs.requests[0].reasoning_content.as_str(), "推理");
}

#[tokio::test]
async fn send_store_request_skips_empty_base_url() {
    let client = Client::new();
    let rst = send_store_request(&client, "", "k", "/store/think", &serde_json::json!({})).await;
    assert!(rst.is_ok(), "base_url 空应跳过发送");
}
```
（channel_requests/tool_call_requests/tool_result_requests force 测试同理改用 *Request 构造。）

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test think_requests_force_is_one`
Expected: 编译错误（think_requests 签名不匹配：当前接收 Vec<ThinkRecord>，测试传 Vec<ThinkRequest>）。

- [ ] **Step 3: 删自定义 Record + 映射函数**

`kissbot-agent/src/memory_store_client.rs`：
- 删除 `struct ChannelRecord`（agent 侧自定义）、`ThinkRecord`、`ToolCallRecord`、`ToolResultRecord`（4 个）
- 删除 `channel_request`/`think_request`/`tool_call_request`/`tool_result_request` 映射函数
- import 改：`use kissbot_api::memory::{ChannelRequest, ChannelRequests, ThinkRequest, ThinkRequests, ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests};`（已有），删除 `use kissbot_api::Content;`（若仅 ChannelRecord 测试用）

- [ ] **Step 4: push 方法改用 *Request**

```rust
pub async fn push_channel_record(&self, record: ChannelRequest) {
    self.channel_appender.append(CHANNEL_KEY.to_string(), vec![record]).await;
}

/// 推送 think 记录（reasoning_content + thinking 双字段，key 关联 ChannelRecord(Think)）
/// 调用方：coordinator 步骤 4（思考记忆推送）
pub async fn push_think(&self, record: ThinkRequest) {
    self.think_appender.append(THINK_KEY.to_string(), vec![record]).await;
}

/// 推送 tool-call 记录（station 工具调用功能落地时消费）
#[allow(dead_code)]
pub async fn push_tool_call(&self, record: ToolCallRequest) {
    self.tool_call_appender.append(TOOL_CALL_KEY.to_string(), vec![record]).await;
}

/// 推送 tool-result 记录（station 工具调用功能落地时消费）
#[allow(dead_code)]
pub async fn push_tool_result(&self, record: ToolResultRequest) {
    self.tool_result_appender.append(TOOL_RESULT_KEY.to_string(), vec![record]).await;
}
```

- [ ] **Step 5: appender 类型 + write 改用 *Request**

`MemoryStoreClient` 结构字段类型 R 泛型改用 *Request：
```rust
pub struct MemoryStoreClient {
    channel_appender: FileObjectAppender<String, ChannelRequest, StoreSender<MemoryStoreContext>, MemoryStoreContext, LoggingErrorHandler>,
    think_appender: FileObjectAppender<String, ThinkRequest, StoreSender<ThinkStoreContext>, ThinkStoreContext, LoggingErrorHandler>,
    tool_call_appender: FileObjectAppender<String, ToolCallRequest, StoreSender<ToolCallStoreContext>, ToolCallStoreContext, LoggingErrorHandler>,
    tool_result_appender: FileObjectAppender<String, ToolResultRequest, StoreSender<ToolResultStoreContext>, ToolResultStoreContext, LoggingErrorHandler>,
}
```
各 context 的 write 直接构造 XxxRequests（records 已是 Vec<XxxRequest>，无需映射）：
```rust
fn think_requests(records: Vec<ThinkRequest>) -> ThinkRequests {
    ThinkRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ThinkRequest> for ThinkStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ThinkRequest>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/think", &think_requests(records)).await
    }
}
```
（channel_requests/tool_call_requests/tool_result_requests 同理：`XxxRequests { requests: records, force: 1 }`，write 调 send_store_request。）

- [ ] **Step 6: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test`
Expected: 全部 PASS（coordinator.rs 的 push_channel_record/push_think 调用因签名改变编译失败--本步暂在 coordinator 调用处传 *Request 实体适配，Task 4 重构 think 流程；若 coordinator 临时编译困难，可先注释 think 调用，Task 4 恢复）。

- [ ] **Step 7: 提交**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/memory_store_client.rs kissbot-agent/src/coordinator.rs
git commit -m "refactor(memory): MemoryStoreClient push 直接用 *Request--删自定义 ChannelRecord/ThinkRecord/ToolCallRecord/ToolResultRecord 与映射函数；push_channel_record(ChannelRequest)/push_think(ThinkRequest)/push_tool_call(ToolCallRequest)/push_tool_result(ToolResultRequest)；write 直接构造 XxxRequests(force:1)；appender R 用 *Request"
```

---

### Task 4: think 写入流程（coordinator 步骤 4 + memory_role 从 Session 读）

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`（think 流程：key + ChannelRecord(Think) + ThinkRequest 双字段；memory_role 3 处调用从 Session 读）

**依赖：** 子项目 1 的 out_channel 契约（`OutChannel { channel_id, user: ChannelUser, group_id }` + `send_outgoing`）；run_agentic_loop 接收 `out_channel: &OutChannel` 参数。

**Interfaces:**
- Consumes: `ModelResponse { content, reasoning_content, thinking, ... }`（Task 2）；`ThinkRequest`/`ChannelRequest`（Task 1/3）；`Content::Think`（Task 1）；`OutChannel`（子项目 1）；`memory_role(&session.role_name, &session.mode)`（Task 1）；`uuid::Uuid`
- Produces: run_agentic_loop 步骤 4 生成 key + ChannelRecord(Think) + ThinkRequest 双字段

- [ ] **Step 1: 写失败测试（think 写入条件 + 双字段映射）**

`kissbot-agent/src/coordinator.rs` 测试模块追加纯函数测试（若 think 构造逻辑可抽纯函数；否则用集成测试覆盖）：
```rust
#[test]
fn think_write_condition_any_non_empty() {
    // 任一有值则写
    assert!(should_write_think(Some("r".into()), None));
    assert!(should_write_think(None, Some("t".into())));
    assert!(should_write_think(Some("r".into()), Some("t".into())));
    // 都 None 不写
    assert!(!should_write_think(None, None));
}

/// think 写入条件：reasoning_content 或 thinking 任一有值
fn should_write_think(reasoning: Option<&str>, thinking: Option<&str>) -> bool {
    reasoning.is_some() || thinking.is_some()
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test think_write_condition_any_non_empty`
Expected: 编译错误（should_write_think 未定义）。

- [ ] **Step 3: 实现写入条件函数**

在 coordinator.rs 加纯函数（步骤 1 的 `should_write_think`）。

- [ ] **Step 4: run_agentic_loop 步骤 4 重构 think 流程**

`kissbot-agent/src/coordinator.rs` run_agentic_loop 中步骤 4（send_outgoing 前）替换为：
```rust
// 4. 推送 think 到 memory-store（reasoning_content + thinking 双字段，key 关联 ChannelRecord(Think)）
// 身份来自 out_channel；任一有值才写，都 None 跳过
if should_write_think(model_resp.reasoning_content.as_deref(), model_resp.thinking.as_deref()) {
    let key = uuid::Uuid::new_v4().to_string();
    let role_name = memory_role(&session.role_name, &session.mode);
    let agent_id = session.agent_id.clone();

    // 4a. ChannelRecord(Think(key)) 写主时间线（身份来自 out_channel，is_self=1）
    self.memory_store_client.push_channel_record(ChannelRequest {
        agent_id: agent_id.clone(),
        role_name: Arc::new(role_name.clone()),
        messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
        user_id: Arc::new(out_channel.user.user_id.clone()),
        self_user_id: Arc::new(out_channel.user.user_id.clone()),
        group_id: out_channel.group_id.clone(),
        is_self: 1,
        messenger_name: Arc::new(String::new()),   // 占位：详情经 key 关联，name 非关键
        user_name: Arc::new(String::new()),
        group_name: Arc::new(String::new()),
        content: Content::Think(Arc::new(key.clone())),
        time: Arc::new(now.clone()),
    }).await;

    // 4b. ThinkRequest(key, reasoning_content, thinking) 写详情
    self.memory_store_client.push_think(ThinkRequest {
        agent_id,
        role_name: Arc::new(role_name),
        reasoning_content: Arc::new(model_resp.reasoning_content.clone().unwrap_or_default()),
        thinking: Arc::new(model_resp.thinking.clone().unwrap_or_default()),
        key: Arc::new(key),
        time: Arc::new(now.clone()),
    }).await;
}
```
（now 在步骤 3 已定义 `Local::now().format(...)`；若 send_outgoing 在步骤 5 用 now，注意 now 的所有权--步骤 4 用 now.clone()。）

- [ ] **Step 5: memory_role 3 处调用从 Session 读**

coordinator.rs 中所有 `memory_role(&session.key)` / `memory_role(&key)` 改为从 Session 字段读：
- incoming_message 推 channel 记录：`memory_role(&session... )` -- 此处无 session，用来源 channel 的 (agent_name, role_name) + 运行态 mode 构造；按子项目 1 的 session_key_for 逻辑，改为 `memory_role(&key.role_name, &key.mode)`（key 为 SessionKey，仍可访问 role_name/mode；或子项目 1 已改为从 session 读则保持）
- run_agentic_loop（步骤 4 已用 `memory_role(&session.role_name, &session.mode)`）
- send_outgoing 推 channel 记录（子项目 1 的 send_outgoing）：`memory_role(&session.role_name, &session.mode)` 或从 out_channel 所属 channel 的 (agent_name, role_name) + mode

（具体调用点依子项目 1 实现后的代码为准；原则：从 Session 运行态字段读 role_name + mode，不从 key 解析。）

- [ ] **Step 6: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test`
Expected: 全部 PASS（单测 + 编译）。

- [ ] **Step 7: 集成测试适配（assertThinkRecords 双字段）**

`test/tests/nexus-ego-chat-store.spec.ts` 中 `assertThinkRecords` 断言改双字段：
```ts
// 原：断言 content 非空
// 改：断言 reasoning_content 或 thinking 任一非空
const thinkRecs: any[] = resp.data[0][1].map((entry: [number, any]) => entry[1]);
expect(thinkRecs.length).toBeGreaterThanOrEqual(1, 'think 记录应已写入（agent 侧 MemoryStoreClient 推送 /store/think）');
const t = thinkRecs[thinkRecs.length - 1];
const hasReasoning = t.reasoning_content && t.reasoning_content.length > 0;
const hasThinking = t.thinking && t.thinking.length > 0;
expect(hasReasoning || hasThinking).toBe(true, 'reasoning_content 或 thinking 应有内容');
expect(t.key).toBeTruthy('key 应非空（UUID 关联 ChannelRecord(Think)）');
```

- [ ] **Step 8: 运行集成测试确认通过**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/nexus-ego-chat-store.spec.ts --grep "场景1"`
Expected: 1 passed（think 双字段端到端写入，key 非空）。

- [ ] **Step 9: 提交**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/coordinator.rs test/tests/nexus-ego-chat-store.spec.ts
git commit -m "feat(agent): think 写入流程--reasoning_content/thinking 任一有值则生成 UUID key 并写 ChannelRecord(Content::Think(key)) + ThinkRequest(双字段)；身份来自 out_channel；memory_role 从 Session 运行态读；集成测试 assertThinkRecords 适配双字段与 key 非空"
```

---

### Task 5: 全量验证

- [ ] **Step 1: 单测全量**

Run: `cd /home/admin/project/kissbot && cargo test`
Expected: 全部 PASS，无 warning（push_tool_call/push_tool_result 保留 allow(dead_code)；ToolCall/ToolResult Content 变体无消费者但枚举完整不需 allow）。

- [ ] **Step 2: nexus 集成测试全量**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/nexus-chat.spec.ts tests/nexus-ego-chat-store.spec.ts tests/agent-commands.spec.ts tests/agent-config-api.spec.ts`
Expected: 全部 passed（含 assertThinkRecords 4 场景双字段 + key 非空）。

- [ ] **Step 3: 检查未提交改动**

Run: `cd /home/admin/project/kissbot && git status --short`
Expected: 无未提交改动。

---

## 自审

**Spec 覆盖：**
- §2.1 Content 三变体 -> Task 1 Step 3 ✓
- §2.2 ThinkRequest/ThinkRecord 双字段 -> Task 1 Step 6 ✓
- §2.3 Session role_name/mode -> Task 1 Step 10 ✓
- §2.4 memory_role 签名 -> Task 1 Step 9 ✓
- §3 ModelResponse 拆分 -> Task 2 ✓
- §4 push 用 *Request -> Task 3 ✓
- §5 think 写入流程 -> Task 4 ✓
- §6 ToolCall/ToolResult（无生成源）-> Task 1 Content 变体 + Task 3 push 方法 allow(dead_code) ✓
- §7.1 写入条件 -> Task 4 should_write_think ✓
- §7.3 只写入不读取 -> 计划不含读取侧 ✓

**占位符扫描：** 无 TBD/TODO；Task 4 Step 5 的 memory_role 调用点说明"依子项目 1 实现后代码为准"（原则明确，非占位符）。

**类型一致性：**
- ThinkRequest 双字段（Task 1 定义）-> Task 3 push_think(ThinkRequest) + Task 4 构造 ThinkRequest ✓
- ModelResponse.thinking（Task 2 定义）-> Task 4 消费 model_resp.thinking ✓
- Content::Think(Arc<String>)（Task 1）-> Task 4 Content::Think(Arc::new(key)) ✓
- memory_role(role_name, mode)（Task 1）-> Task 4 调用 ✓
- OutChannel（子项目 1）-> Task 4 out_channel.user/group_id ✓

**依赖标注：** Task 4 明确依赖子项目 1 的 out_channel 契约。
