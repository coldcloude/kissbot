# MemoryStoreClient 合并 MemoryWriter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 MemoryWriter 的 think / tool-call / tool-result 推送能力并入 MemoryStoreClient（方案 B：每记录类型一个 FileObjectAppender），删除 memory_writer.rs 与 WriteTask，由 MemoryStoreClient 承载全部 /store/* 推送。

**Architecture:** memory_store_client.rs 扩展为 4 个 FileObjectAppender（channel/think/tool-call/tool-result），各类型 context::write 只负责把记录转成 XxxRequests 实体（force 统一 1），生成实体后的 HTTP 发送段由共享函数 send_store_request 承载（URL 拼接 / X-API-Key 鉴权 / 非 2xx 检查 / base_url 空跳过，只写一遍）；sender 泛型化 StoreSender<C>（一次实现四类型复用）；错误处理统一 LoggingErrorHandler（替换 NoopErrorHandler，修复 channel 静默丢弃）。

**Tech Stack:** Rust（tokio / reqwest / serde / kai-file），cargo test 与 Playwright 集成测试验证。

## Global Constraints

- **不删除代码中的注释**（项目约定）
- **不修改**：kai-file 基础库（队列保持无界）、kissbot-api 请求结构定义、memory-store 服务端、docs/spec 文档
- **方案 B**：每记录类型一个 `FileObjectAppender`；HTTP 发送段 `send_store_request` 只写一遍
- **force 统一为 1**（channel/think/tool-call/tool-result 全部；跳过 RecordNotInOrder 时间乱序检查）
- 字段映射遵循 `kissbot_api::memory` 的 `ChannelRequest` / `ThinkRequest` / `ToolCallRequest` / `ToolResultRequest`（全部 Arc 字段）；agent 侧 `role_name: Option<Arc<String>>` → 请求体 `Arc<String>`（None → 空串）
- 错误处理统一：`LoggingErrorHandler`（写失败打 error!，含 key 与错误），替换 channel 的 `NoopErrorHandler`
- coordinator 删除 memory_writer 字段/初始化，步骤 4 直接调 `push_think`；`push_tool_call` / `push_tool_result` 本期无调用方（按项目惯例 `#[allow(dead_code)]`）
- 文本文件 UTF-8、LF；commit comment 用中文，且应包含本次提交所有改动内容

---

### Task 1: MemoryStoreClient 扩展为 4 appender（共享发送段 + 泛型 sender + 记录类型 + push 方法）

**Files:**
- Modify: `kissbot-agent/src/memory_store_client.rs`（全文件重写结构）
- Test: `kissbot-agent/src/memory_store_client.rs`（tests 模块）

**Interfaces:**
- Consumes: `kissbot_api::memory::{ChannelRequest, ChannelRequests, ThinkRequest, ThinkRequests, ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests}`；kai-file `FileObjectAppender / FileAppendWriter / FileAppendWriterContext / ErrorHandler`
- Produces: `MemoryStoreClient { push_channel_record / push_think / push_tool_call / push_tool_result }`；`fn send_store_request(client, base_url, api_key, path, body) -> Result<(), kai_file::Error>`；`struct StoreSender<C>`；`struct LoggingErrorHandler`；记录类型 `ThinkRecord / ToolCallRecord / ToolResultRecord`

- [ ] **Step 1: 写失败测试（requests 构造 + force=1 + base_url 空跳过）**

在 `kissbot-agent/src/memory_store_client.rs` 末尾新增 tests 模块：
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_request_maps_fields_and_empty_role() {
        let r = ThinkRecord {
            agent_id: Arc::new("a1".into()),
            role_name: Some(Arc::new("r1".into())),
            content: Arc::new("思考".into()),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        };
        let req = think_request(r);
        assert_eq!(req.agent_id.as_str(), "a1");
        assert_eq!(req.role_name.as_str(), "r1");
        assert_eq!(req.content.as_str(), "思考");
        assert_eq!(req.time.as_str(), "2026-08-03 10:00:00");

        let r2 = ThinkRecord {
            agent_id: Arc::new("a1".into()),
            role_name: None,   // 无 role → 请求体空串
            content: Arc::new("思考2".into()),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:01".into()),
        };
        assert_eq!(think_request(r2).role_name.as_str(), "", "None role_name 应映射为空串");
    }

    #[test]
    fn think_requests_force_is_one() {
        let reqs = think_requests(vec![ThinkRecord {
            agent_id: Arc::new("a1".into()),
            role_name: None,
            content: Arc::new("思考".into()),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        }]);
        assert_eq!(reqs.force, 1, "force 统一为 1");
        assert_eq!(reqs.requests.len(), 1);
    }

    #[test]
    fn channel_requests_force_is_one() {
        let rec = ChannelRecord {
            agent_id: Arc::new("a1".into()),
            role_name: Arc::new("r1".into()),
            messenger_id: Arc::new("web".into()),
            user_id: Arc::new("u1".into()),
            self_user_id: Arc::new("u1".into()),
            group_id: Arc::new("g1".into()),
            is_self: 0,
            messenger_name: Arc::new("".into()),
            user_name: Arc::new("".into()),
            group_name: Arc::new("".into()),
            content: Content::Text(Arc::new("你好".into())),
            time: Arc::new("2026-08-03 10:00:00".into()),
        };
        let reqs = channel_requests(vec![rec]);
        assert_eq!(reqs.force, 1, "channel force 统一为 1");
        assert_eq!(reqs.requests[0].user_id.as_str(), "u1");
    }

    #[test]
    fn tool_call_and_result_requests_force_is_one() {
        let call = tool_call_requests(vec![ToolCallRecord {
            agent_id: Arc::new("a1".into()),
            role_name: None,
            tool_name: Arc::new("get_date".into()),
            tool_params: Arc::new(serde_json::json!({})),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        }]);
        assert_eq!(call.force, 1);
        assert_eq!(call.requests[0].tool_name.as_str(), "get_date");
        assert_eq!(call.requests[0].role_name.as_str(), "", "None role_name 应映射为空串");

        let result = tool_result_requests(vec![ToolResultRecord {
            agent_id: Arc::new("a1".into()),
            role_name: None,
            tool_result: Arc::new(serde_json::json!({ "ok": true })),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        }]);
        assert_eq!(result.force, 1);
        assert_eq!(result.requests[0].tool_result["ok"], true);
    }

    #[tokio::test]
    async fn send_store_request_skips_empty_base_url() {
        let client = Client::new();
        // base_url 空 → 直接 Ok（不联网）
        let rst = send_store_request(&client, "", "k", "/store/think", &serde_json::json!({})).await;
        assert!(rst.is_ok(), "base_url 空应跳过发送");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 编译错误——`think_request` / `think_requests` / `channel_requests` / `tool_call_requests` / `tool_result_requests` / `send_store_request` 未定义；`ThinkRecord` 等类型未定义。

- [ ] **Step 3: 重写 memory_store_client.rs 结构**

文件头部 import 区替换为：
```rust
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileAppendWriterContext, FileObjectAppender, ErrorHandler};
use kissbot_api::memory::{
    ChannelRequest, ChannelRequests, ThinkRequest, ThinkRequests,
    ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests,
};
use kissbot_security::HEADER_API_KEY;
use kissbot_api::Content;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::error;

// 批处理参数
const RECORD_QUEUE_SIZE: usize = 100;
const RECORD_MAX_DELAY: Duration = Duration::from_secs(1);
const CHANNEL_KEY: &str = "0";
const THINK_KEY: &str = "think";
const TOOL_CALL_KEY: &str = "tool-call";
const TOOL_RESULT_KEY: &str = "tool-result";
```

`ChannelRecord` 结构保持不变（原有定义与注释不动）。

删除原 `MemoryStoreSender` 结构与其 impl，替换为泛型 sender + 共享件（放在 `MemoryStoreClient` 定义之前）：
```rust
// ========== 共享发送段（各记录类型复用，只写一遍） ==========

/// 构造共享的 HTTP 配置（client / base_url / api_key，各类型 context 同源）
fn store_http_config() -> (Client, String, Arc<String>) {
    let api_config = kissbot_api::ApiConfig::get();
    let security = kissbot_security::SecurityConfig::get();
    (Client::new(), api_config.memory_store_url.clone(), security.api_key.clone())
}

/// POST {base_url}{path}，带 X-Api-Key 鉴权头；base_url 空则跳过（Ok）；
/// 非 2xx 返回 Err（错误含状态码与返回体）
async fn send_store_request(
    client: &Client,
    base_url: &str,
    api_key: &str,
    path: &str,
    body: &impl Serialize,
) -> std::result::Result<(), kai_file::Error> {
    if base_url.is_empty() {
        return Ok(());
    }
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = client.post(&url)
        .header(HEADER_API_KEY, api_key)
        .json(body)
        .send()
        .await
        .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?;
    if !response.status().is_success() {
        let status = response.status();
        let msg = response.text().await.unwrap_or_default();
        return Err(kai_file::Error::WriteError(format!("[{}] {}", status, msg)));
    }
    Ok(())
}

/// 写失败统一日志 handler（替换 NoopErrorHandler，修复写失败静默丢弃）
struct LoggingErrorHandler;
#[async_trait]
impl<K, R> ErrorHandler<K, R> for LoggingErrorHandler
where
    K: std::fmt::Debug + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    async fn on_write_error(&self, key: &K, _batch: Vec<R>, error: &kai_file::Error) {
        error!("记忆写入失败 key={:?}: {}", key, error);
    }
}

// ========== 泛型 sender（各记录类型复用 FileAppendWriter，只写一遍） ==========

pub struct StoreSender<C> {
    context: Arc<Mutex<C>>,
}

impl<C> StoreSender<C> {
    pub fn new(context: C) -> Self {
        Self { context: Arc::new(Mutex::new(context)) }
    }
}

#[async_trait]
impl<K, R, C> FileAppendWriter<K, R, C> for StoreSender<C>
where
    C: FileAppendWriterContext<K, R>,
{
    async fn get_lock(&self, _key: &K) -> Arc<Mutex<C>> {
        self.context.clone()
    }
    async fn remove_lock(&self, _key: &K) {}
}

// ========== 记录类型（与 WriteTask 变体字段同构） ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecord {
    pub agent_id: Arc<String>,
    pub role_name: Option<Arc<String>>,   // None → 空串（与 memory-store 无 role 语义一致）
    pub content: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub agent_id: Arc<String>,
    pub role_name: Option<Arc<String>>,
    pub tool_name: Arc<String>,
    pub tool_params: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub agent_id: Arc<String>,
    pub role_name: Option<Arc<String>>,
    pub tool_result: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}
```

`MemoryStoreClient` 结构与其 impl 替换为：
```rust
pub struct MemoryStoreClient {
    channel_appender: FileObjectAppender<String, ChannelRecord, StoreSender<MemoryStoreContext>, MemoryStoreContext>,
    think_appender: FileObjectAppender<String, ThinkRecord, StoreSender<ThinkStoreContext>, ThinkStoreContext>,
    tool_call_appender: FileObjectAppender<String, ToolCallRecord, StoreSender<ToolCallStoreContext>, ToolCallStoreContext>,
    tool_result_appender: FileObjectAppender<String, ToolResultRecord, StoreSender<ToolResultStoreContext>, ToolResultStoreContext>,
}

impl MemoryStoreClient {
    pub fn new() -> Self {
        let (client, base_url, api_key) = store_http_config();
        Self {
            channel_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(MemoryStoreContext { client: client.clone(), base_url: base_url.clone(), api_key: api_key.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            think_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ThinkStoreContext { client: client.clone(), base_url: base_url.clone(), api_key: api_key.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_call_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolCallStoreContext { client: client.clone(), base_url: base_url.clone(), api_key: api_key.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_result_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolResultStoreContext { client, base_url, api_key })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
        }
    }

    pub async fn push_channel_record(&self, record: ChannelRecord) {
        self.channel_appender.append(CHANNEL_KEY.to_string(), vec![record]).await;
    }

    /// 推送 think 记录（思考内容，方案 A 只存思考内容）
    pub async fn push_think(&self, agent_id: String, role_name: Option<String>, content: String, time: String) {
        self.think_appender.append(THINK_KEY.to_string(), vec![ThinkRecord {
            agent_id: Arc::new(agent_id),
            role_name: role_name.map(Arc::new),
            content: Arc::new(content),
            key: Arc::new(String::new()),
            time: Arc::new(time),
        }]).await;
    }

    /// 推送 tool-call 记录（station 工具调用功能落地时消费）
    #[allow(dead_code)]
    pub async fn push_tool_call(&self, agent_id: String, role_name: Option<String>, tool_name: String, tool_params: serde_json::Value, time: String) {
        self.tool_call_appender.append(TOOL_CALL_KEY.to_string(), vec![ToolCallRecord {
            agent_id: Arc::new(agent_id),
            role_name: role_name.map(Arc::new),
            tool_name: Arc::new(tool_name),
            tool_params: Arc::new(tool_params),
            key: Arc::new(String::new()),
            time: Arc::new(time),
        }]).await;
    }

    /// 推送 tool-result 记录（station 工具调用功能落地时消费）
    #[allow(dead_code)]
    pub async fn push_tool_result(&self, agent_id: String, role_name: Option<String>, tool_result: serde_json::Value, time: String) {
        self.tool_result_appender.append(TOOL_RESULT_KEY.to_string(), vec![ToolResultRecord {
            agent_id: Arc::new(agent_id),
            role_name: role_name.map(Arc::new),
            tool_result: Arc::new(tool_result),
            key: Arc::new(String::new()),
            time: Arc::new(time),
        }]).await;
    }
}
```

`MemoryStoreContext` 及其 `FileAppendWriterContext` impl 替换（write 构造实体 → 共享发送；`base_url.is_empty()` 检查挪入 send_store_request）：
```rust
pub struct MemoryStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn channel_request(r: ChannelRecord) -> ChannelRequest {
    ChannelRequest {
        agent_id: r.agent_id,
        role_name: r.role_name,
        messenger_id: r.messenger_id,
        user_id: r.user_id,
        self_user_id: r.self_user_id,
        group_id: r.group_id,
        is_self: r.is_self,
        messenger_name: r.messenger_name,
        user_name: r.user_name,
        group_name: r.group_name,
        content: r.content,
        time: r.time,
    }
}

fn channel_requests(records: Vec<ChannelRecord>) -> ChannelRequests {
    ChannelRequests { requests: records.into_iter().map(channel_request).collect(), force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ChannelRecord> for MemoryStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ChannelRecord>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/channel", &channel_requests(records)).await
    }
}
```

think / tool-call / tool-result 的 context 与 requests 构造（放在 MemoryStoreContext 之后）：
```rust
pub struct ThinkStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn think_request(r: ThinkRecord) -> ThinkRequest {
    ThinkRequest {
        agent_id: r.agent_id,
        role_name: r.role_name.unwrap_or_default(),  // None → 空串（与 memory-store 无 role 语义一致）
        content: r.content,
        key: r.key,
        time: r.time,
    }
}

fn think_requests(records: Vec<ThinkRecord>) -> ThinkRequests {
    ThinkRequests { requests: records.into_iter().map(think_request).collect(), force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ThinkRecord> for ThinkStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ThinkRecord>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/think", &think_requests(records)).await
    }
}

pub struct ToolCallStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn tool_call_request(r: ToolCallRecord) -> ToolCallRequest {
    ToolCallRequest {
        agent_id: r.agent_id,
        role_name: r.role_name.unwrap_or_default(),
        tool_name: r.tool_name,
        tool_params: r.tool_params,
        key: r.key,
        time: r.time,
    }
}

fn tool_call_requests(records: Vec<ToolCallRecord>) -> ToolCallRequests {
    ToolCallRequests { requests: records.into_iter().map(tool_call_request).collect(), force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ToolCallRecord> for ToolCallStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ToolCallRecord>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/tool-call", &tool_call_requests(records)).await
    }
}

pub struct ToolResultStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn tool_result_request(r: ToolResultRecord) -> ToolResultRequest {
    ToolResultRequest {
        agent_id: r.agent_id,
        role_name: r.role_name.unwrap_or_default(),
        tool_result: r.tool_result,
        key: r.key,
        time: r.time,
    }
}

fn tool_result_requests(records: Vec<ToolResultRecord>) -> ToolResultRequests {
    ToolResultRequests { requests: records.into_iter().map(tool_result_request).collect(), force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ToolResultRecord> for ToolResultStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ToolResultRecord>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/tool-result", &tool_result_requests(records)).await
    }
}
```

注意：`MemoryStoreContext` 原有字段与 `write` 中 channel 字段映射注释保留（不删除注释）；原 `push_channel_record` 行为不变。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 全部 PASS（新增 5 个单测 + 存量 60 个，共 65）；无 warning（push_tool_call/push_tool_result 有 `#[allow(dead_code)]`；`ThinkStoreContext` 等有 push 方法消费）。

- [ ] **Step 5: 提交**

```bash
git add kissbot-agent/src/memory_store_client.rs
git commit -m "refactor(memory): MemoryStoreClient 扩展为 4 appender——新增 think/tool-call/tool-result 的 FileObjectAppender 与 push_think/push_tool_call/push_tool_result；sender 泛型化 StoreSender<C> 一次实现四类型复用；共享发送段 send_store_request 只写一遍（URL 拼接/X-Api-Key/非 2xx/base_url 空跳过）；force 统一为 1；错误处理统一 LoggingErrorHandler（替换 NoopErrorHandler 修复 channel 静默丢弃）；新增 requests 构造与 force/base_url 空跳过单测 5 个"
```

---

### Task 2: coordinator 接入 push_think + 删除 memory_writer/WriteTask

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`（import、字段、初始化、步骤 4）
- Modify: `kissbot-agent/src/types.rs`（删除 WriteTask）
- Delete: `kissbot-agent/src/memory_writer.rs`
- Modify: `kissbot-agent/src/memory_store_client.rs`（移除 push_think 的 `#[allow(dead_code)]`）

**Interfaces:**
- Consumes: `MemoryStoreClient.push_think(agent_id: String, role_name: Option<String>, content: String, time: String)`（Task 1 产出）

- [ ] **Step 1: 修改 coordinator.rs**

删除第 20 行 `use crate::memory_writer::MemoryWriter;`。

删除字段 `memory_writer: Arc<MemoryWriter>,`（原 ~71 行）与其初始化（原 ~88-99 行 `MemoryWriter::start()` 及 Arc 包裹、`memory_writer,` 字段赋值——按实际代码定位删除，保留注释或改为说明性注释）。

步骤 4 原代码：
```rust
                // 4. 推送 think 到 MemoryWriter（事件模式编码；取记忆用会话保存的 agent_id）
                // Think 记忆只存思考内容（方案 A）：有思考内容才写，无则跳过
                if let Some(reasoning) = &model_resp.reasoning_content {
                    let role_name = memory_role(&session.key);
                    let _ = self.memory_writer.push(WriteTask::Think {
                        agent_id: session.agent_id.to_string(),
                        role_name: Some(role_name),
                        content: reasoning.clone(),
                        time: now,
                    });
                }
```
替换为：
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

- [ ] **Step 2: 删除 types.rs 的 WriteTask**

`kissbot-agent/src/types.rs` 删除 `WriteTask` 枚举定义（含 `#[allow(dead_code)]` 的 ToolCall / ToolResult 变体与"MemoryWriter 写入队列"注释块）。

- [ ] **Step 3: 删除 memory_writer.rs**

Run: `cd /home/admin/project/kissbot && git rm kissbot-agent/src/memory_writer.rs`

- [ ] **Step 4: 移除 push_think 的 allow(dead_code)**

`kissbot-agent/src/memory_store_client.rs` 中 `pub async fn push_think` 上方的 `#[allow(dead_code)]` 删除（Task 2 起有 coordinator 调用方）。

- [ ] **Step 5: 构建与单测**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 编译通过（memory_store_client.rs 的 tests 模块引用 `Content` 等 import 无冲突；`WriteTask` 删除后无残留引用）；65 个测试 PASS；无 warning（`push_think` 有调用方、`push_tool_call`/`push_tool_result` 保留 `#[allow(dead_code)]`）。

- [ ] **Step 6: 集成验证（思考记忆端到端）**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/nexus-ego-chat-store.spec.ts --grep "场景1"`
Expected: 1 passed（`assertThinkRecords` 验证 think 经新 appender 写入 memory-store）。

- [ ] **Step 7: 提交**

```bash
git add kissbot-agent/src/coordinator.rs kissbot-agent/src/types.rs kissbot-agent/src/memory_store_client.rs
git commit -m "refactor(agent): coordinator 接入 MemoryStoreClient.push_think，删除 MemoryWriter——移除 memory_writer 字段/初始化与 WriteTask 枚举，删除 memory_writer.rs；步骤 4 直接调 push_think（行为不变：有思考内容才写，无则跳过）"
```

---

### Task 3: 全量验证

- [ ] **Step 1: 单测全量**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 65 passed / 0 failed，无 warning。

- [ ] **Step 2: nexus 集成测试全量**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/nexus-chat.spec.ts tests/nexus-ego-chat-store.spec.ts tests/agent-commands.spec.ts tests/agent-config-api.spec.ts`
Expected: 23 passed（含 assertThinkRecords 4 场景与 channel 记忆断言——channel 改走共享发送段后行为不变）。

- [ ] **Step 3: 检查未提交改动**

Run: `cd /home/admin/project/kissbot && git status --short`
Expected: 无未提交改动。
