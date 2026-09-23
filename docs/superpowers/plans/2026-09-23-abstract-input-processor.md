# 摘要记忆输入处理器 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 kissbot-agent 中新增一个 pipeline 输入处理器：按 channel（messenger + 接收方用户 + 群组）攒批，每秒定时检测，某 channel 的最后一条消息超时即触发；触发时经摘要记忆（kissbot-memory-struct-abstract）定位事件、取事件摘要与事件结尾原文副本重建会话上下文，再把本批新消息（与历史去重后剩下的部分）交给 pipeline 运行。

**Architecture:** 新增 `AbstractClient`（REST 客户端，仿 `memory_ego_client`）与 `AbstractAgentInputProcessor`（输入处理器，仿 `BatchAgentInputProcessor` 的 per-session 构造与 Drop 退出模式）。处理器在首次 `accept` 时惰性启动一个 1 秒 tick 任务；tick 任务取走已超时 channel 的批次并串行 flush（避免同会话上下文并发写）。flush 顺序：`/abstract/locate` → 逐事件 `/abstract/event/summary` + `/abstract/event/records-tail` → 时间窗过滤 + 与本批新消息去重 → `pack_memory_messages` 组装 → `Session::context_archive_and_clear_cache_and_reset_messages` → `Nexus::run_pipeline`。任何摘要记忆调用失败都降级为「不重置上下文，直接合批进 pipeline」。

**Tech Stack:** Rust 2024 / tokio / reqwest / serde / chrono / dashmap / async-trait；复用 kissbot-api 的 `LocateRequest`/`EventSummaryRequest`/`EventRecordsTailRequest` 与 kissbot-llm 的 `Session`/`Message`。

## Global Constraints

- 所有文本文件 UTF-8 编码、`\n` 换行；注释用中文。
- 装配方式（已确认）：**只注册新处理器，不改 preset**（`preset_role` / `preset_event` 仍指向 `batch_input_processor`）。新处理器经 nexus.json 的 `pipeline.input_processor` 显式启用。
- 服务地址（已确认）：`api` 段新增 `memory_struct_abstract_url`（`#[serde(default)]`，空 = 未配置）。
- 失败降级（已确认）：摘要记忆未配置或任一调用失败 → 记 warn，不重置上下文，照常 `run_pipeline(pack_batch(batch))`。
- 配置复用：不加新配置结构。超时用 `ChannelBatchConfig.channel_batch_interval_secs`；时间窗与条数用 `MemoryRecoverConfig` 的 `memory_time_secs` / `memory_count`。tick 固定 1 秒。
- 去重方向（已确认）：查出的历史记录里去掉落在本批新消息里的条目；本批新消息全部进 pipeline。
- 不改动 `BatchAgentInputProcessor` 的行为；`PP_IN_BATCH` 常量与实现保留。
- 验证命令：`cd kissbot-api && cargo test`、`cd kissbot-agent && cargo test`、`cd kissbot-agent && cargo build`。

---

### Task 1: api 段新增摘要记忆服务地址

**Files:**
- Modify: `kissbot-api/src/config.rs`
- Modify: `kissbot-api/src/config.rs`（文件末尾新增 `#[cfg(test)] mod tests`）
- Modify: `script/template/config.json`
- Modify: `test/workspace-template/config.json`
- Modify: `test/workspace/config.json`

**Interfaces:**
- Consumes: `kissbot_config::Config::get().get_section("api")`（既有）
- Produces: `ApiConfig { memory_store_url: String, memory_ego_url: String, memory_struct_abstract_url: String }`——Task 2 的 `AbstractClient` 读它

- [ ] **Step 1: 写失败测试**

在 `kissbot-api/src/config.rs` 末尾追加（文件当前只有 17 行，无测试模块）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abstract_url_defaults_to_empty_when_absent() {
        // 旧配置没有该字段 → 缺省为空串（表示摘要记忆未部署）
        let cfg: ApiConfig = serde_json::from_value(serde_json::json!({
            "memory_store_url": "http://127.0.0.1:8082",
            "memory_ego_url": "http://127.0.0.1:3001"
        }))
        .unwrap();
        assert_eq!(cfg.memory_store_url, "http://127.0.0.1:8082");
        assert_eq!(cfg.memory_struct_abstract_url, "");
    }

    #[test]
    fn abstract_url_is_read_when_present() {
        let cfg: ApiConfig = serde_json::from_value(serde_json::json!({
            "memory_store_url": "",
            "memory_ego_url": "",
            "memory_struct_abstract_url": "http://127.0.0.1:8083"
        }))
        .unwrap();
        assert_eq!(cfg.memory_struct_abstract_url, "http://127.0.0.1:8083");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd kissbot-api && cargo test abstract_url`
Expected: 编译失败——`ApiConfig` 无 `memory_struct_abstract_url` 字段（error[E0609]）；或解析结果缺少该字段。

- [ ] **Step 3: 实现字段**

`kissbot-api/src/config.rs` 的 `ApiConfig` 改为：

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    pub memory_store_url: String,
    pub memory_ego_url: String,
    /// 摘要记忆（memory-struct-abstract）服务地址；缺省为空 = 未部署（输入处理器降级为普通合批）
    #[serde(default)]
    pub memory_struct_abstract_url: String,
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd kissbot-api && cargo test abstract_url`
Expected: 2 passed。

- [ ] **Step 5: 同步三份配置模板**

三份文件的 `"api"` 段都补上字段（写在 `memory_ego_url` 之后）：

```json
  "api": {
    "memory_store_url": "",
    "memory_ego_url": "",
    "memory_struct_abstract_url": ""
  },
```

- [ ] **Step 6: 提交**

```bash
git add kissbot-api/src/config.rs script/template/config.json test/workspace-template/config.json test/workspace/config.json
git commit -m "kissbot-api 的 api 段新增摘要记忆服务地址 memory_struct_abstract_url，三份配置模板同步补齐"
```

---

### Task 2: agent 错误类型与摘要记忆 REST 客户端

**Files:**
- Modify: `kissbot-agent/src/types/error.rs`
- Create: `kissbot-agent/src/abstract_client.rs`
- Modify: `kissbot-agent/src/main.rs`（`mod` 声明）

**Interfaces:**
- Consumes: Task 1 的 `ApiConfig.memory_struct_abstract_url`；`kissbot_security::{HEADER_API_KEY, SecurityConfig}`（同 `memory_ego_client`）
- Produces:
  - `Error::AbstractError(String)`
  - `AbstractClient::new() -> AbstractClient`
  - `AbstractClient::with_url(base_url: String, api_key: String) -> AbstractClient`
  - `AbstractClient::locate(&self, agent_id: &str, role: &str, messages: Vec<LocateMessage>) -> Result<LocateResponse>`
  - `AbstractClient::event_summary(&self, agent_id: &str, role: &str, event_id: &str) -> Result<EventNode>`
  - `AbstractClient::event_records_tail(&self, agent_id: &str, role: &str, event_id: &str, count: u64) -> Result<EventRecordsResponse>`

- [ ] **Step 1: 写失败测试（客户端文件内的空地址用例）**

创建 `kissbot-agent/src/abstract_client.rs`，先只写文件头与测试模块：

```rust
//! 摘要记忆（memory-struct-abstract）REST 客户端，与 memory_ego_client 同模式：
//! 共享 reqwest client + 组件地址（api.memory_struct_abstract_url）+ api_key（security.api_key）。
//! 接口约定见 [memory-struct-abstract 技术规格](../../docs/spec/memory-struct-abstract.md)。

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_base_url_returns_abstract_error() {
        // 摘要记忆未配置：请求前直接返回 Err（调用方降级为普通合批），不发网络请求
        let client = AbstractClient::with_url(String::new(), String::new());
        let err = client.locate("0", "dev", vec![]).await.unwrap_err();
        assert!(matches!(err, Error::AbstractError(_)), "未配置地址应返回 AbstractError: {:?}", err);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd kissbot-agent && cargo test abstract_client`
Expected: 编译失败——`AbstractClient` / `Error::AbstractError` 未定义。

- [ ] **Step 3: 加错误变体**

`kissbot-agent/src/types/error.rs` 在 `MemoryEgoError` 之后插入：

```rust
    #[error("Abstract memory error: {0}")]
    AbstractError(String),
```

- [ ] **Step 4: 实现客户端**

把 `kissbot-agent/src/abstract_client.rs` 补全为：

```rust
//! 摘要记忆（memory-struct-abstract）REST 客户端，与 memory_ego_client 同模式：
//! 共享 reqwest client + 组件地址（api.memory_struct_abstract_url）+ api_key（security.api_key）。
//! 接口约定见 [memory-struct-abstract 技术规格](../../docs/spec/memory-struct-abstract.md)。

use std::sync::Arc;

use kissbot_api::{
    ApiResponse, EventNode, EventRecordsResponse, EventRecordsTailRequest, EventSummaryRequest,
    EventSummaryResponse, LocateMessage, LocateRequest, LocateResponse,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::types::{Error, Result};

/// 摘要记忆 REST 客户端：请求走 `X-Api-Key`；`memory_struct_abstract_url` 为空时
/// 所有调用在发请求前返回 Err（调用方降级为普通合批）
pub struct AbstractClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl AbstractClient {
    /// 从进程级单例读取配置构造（ApiConfig.memory_struct_abstract_url / SecurityConfig.api_key）
    pub fn new() -> Self {
        Self::with_url(
            kissbot_api::ApiConfig::get().memory_struct_abstract_url.clone(),
            kissbot_security::SecurityConfig::get().api_key.to_string(),
        )
    }

    /// 显式地址构造（单测用；不依赖进程级配置单例）
    pub fn with_url(base_url: String, api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
        }
    }

    /// POST /abstract/locate：定位一批消息涉及的事件（摘要与原文副本由调用方按事件标识另取）
    pub async fn locate(&self, agent_id: &str, role: &str, messages: Vec<LocateMessage>) -> Result<LocateResponse> {
        self.post(
            "/abstract/locate",
            &LocateRequest {
                agent_id: Arc::new(agent_id.to_string()),
                role: Arc::new(role.to_string()),
                messages,
            },
        )
        .await
    }

    /// POST /abstract/event/summary：取事件摘要（返回节点本身：摘要文本 + 层级 + 记录数 + 起止时间）
    pub async fn event_summary(&self, agent_id: &str, role: &str, event_id: &str) -> Result<EventNode> {
        let resp: EventSummaryResponse = self
            .post(
                "/abstract/event/summary",
                &EventSummaryRequest {
                    agent_id: Arc::new(agent_id.to_string()),
                    role: Arc::new(role.to_string()),
                    event_id: Arc::new(event_id.to_string()),
                },
            )
            .await?;
        Ok(resp.event.as_ref().clone())
    }

    /// POST /abstract/event/records-tail：取事件结尾 count 条原文副本（升序）
    pub async fn event_records_tail(&self, agent_id: &str, role: &str, event_id: &str, count: u64) -> Result<EventRecordsResponse> {
        self.post(
            "/abstract/event/records-tail",
            &EventRecordsTailRequest {
                agent_id: Arc::new(agent_id.to_string()),
                role: Arc::new(role.to_string()),
                event_id: Arc::new(event_id.to_string()),
                count,
            },
        )
        .await
    }

    /// 统一 POST + ApiResponse 解包：未配置地址 / 网络失败 / 解析失败 / 业务失败都返回 Err
    async fn post<Req, Resp>(&self, path: &str, request: &Req) -> Result<Resp>
    where
        Req: Serialize + Sync,
        Resp: DeserializeOwned,
    {
        if self.base_url.is_empty() {
            return Err(Error::AbstractError(
                "摘要记忆未配置（api.memory_struct_abstract_url 为空）".to_string(),
            ));
        }
        let resp = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(kissbot_security::HEADER_API_KEY, self.api_key.as_str())
            .json(request)
            .send()
            .await
            .map_err(|e| Error::AbstractError(format!("{} 请求失败: {}", path, e)))?;
        let envelope = resp
            .json::<ApiResponse<Resp>>()
            .await
            .map_err(|e| Error::AbstractError(format!("{} 响应解析失败: {}", path, e)))?;
        envelope.data.ok_or_else(|| {
            Error::AbstractError(format!("{} 返回失败: {}", path, envelope.error.unwrap_or_default()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_base_url_returns_abstract_error() {
        // 摘要记忆未配置：请求前直接返回 Err（调用方降级为普通合批），不发网络请求
        let client = AbstractClient::with_url(String::new(), String::new());
        let err = client.locate("0", "dev", vec![]).await.unwrap_err();
        assert!(matches!(err, Error::AbstractError(_)), "未配置地址应返回 AbstractError: {:?}", err);
    }
}
```

- [ ] **Step 5: 注册模块**

`kissbot-agent/src/main.rs` 在 `mod channel_manager;` 之前插入：

```rust
mod abstract_client;
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cd kissbot-agent && cargo test abstract_client`
Expected: 1 passed。

- [ ] **Step 7: 提交**

```bash
git add kissbot-agent/src/types/error.rs kissbot-agent/src/abstract_client.rs kissbot-agent/src/main.rs
git commit -m "kissbot-agent 新增摘要记忆 REST 客户端 abstract_client（定位/事件摘要/事件原文尾部查询），错误类型新增 AbstractError"
```

---

### Task 3: 输入处理器（纯逻辑与单测）

**Files:**
- Create: `kissbot-agent/src/pipelines/abstract_input_processor.rs`
- Test: 同文件内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::message::{MessageContent, extract_content, pack_batch, pack_memory_messages}`；`kissbot_api::{ChannelKey, ChannelRecord, EventRecord, IncomingMessageEvent, LocateMessage}`；`kissbot_llm::{Message, Session}`
- Produces（Task 4 使用的确切签名）:

```rust
type MessageKey = (String, String, String, bool);
struct PendingBatch { events: Vec<Arc<IncomingMessageEvent>>, last_at: Instant }
pub struct AbstractAgentInputProcessor {
    session_key: Arc<SessionKey>, session: Arc<Session>, client: Arc<AbstractClient>,
    batches: Arc<DashMap<ChannelKey, PendingBatch>>, notify: Arc<Notify>,
}
fn event_channel(event: &IncomingMessageEvent) -> ChannelKey
fn due_channels(batches: &DashMap<ChannelKey, PendingBatch>, timeout: Duration) -> Vec<ChannelKey>
fn locate_message(event: &IncomingMessageEvent) -> LocateMessage
fn message_key(time: &str, user_name: &str, text: &str, is_self: bool) -> MessageKey
fn new_message_key(event: &IncomingMessageEvent) -> MessageKey
fn join_text(mc: &MessageContent) -> String
fn filter_history(records: &[Arc<ChannelRecord>], cutoff: &str, new_keys: &HashSet<MessageKey>) -> Vec<MessageContent>
fn compose_context(summaries: &[String], history: &[MessageContent]) -> Vec<Message>
impl AbstractAgentInputProcessor {
    fn assemble(session_key: Arc<SessionKey>, session: Arc<Session>, client: Arc<AbstractClient>) -> Self
    fn enqueue(&self, event: Arc<IncomingMessageEvent>)
}
```

- [ ] **Step 1: 写失败测试**

创建 `kissbot-agent/src/pipelines/abstract_input_processor.rs`，先写文件头 + 测试模块（实现留空）：

```rust
//! 摘要记忆输入处理器：按 channel（messenger + 接收方用户 + 群组）攒批，每秒定时检测，
//! 某 channel 的最后一条消息超时即触发该批：先经摘要记忆（定位事件 → 事件摘要 + 事件结尾原文副本）
//! 重建会话上下文，再把本批新消息交给 pipeline 运行。摘要记忆未配置或调用失败时降级为普通合批。

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;

    use crate::types::Mode;

    /// 构造测试用上行消息事件（通道固定 web / self / g1 或 g2）
    fn ev_in(messenger: &str, self_user: &str, group: &str, name: &str, text: &str, time: &str) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: Arc::new(self_user.into()),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: Arc::new("m".into()),
                messenger_id: Arc::new(messenger.into()),
                user_id: Arc::new("u".into()),
                group_id: Arc::new(group.into()),
                messenger_name: Arc::new("".into()),
                user_name: Arc::new(name.into()),
                group_name: Arc::new("".into()),
                content: Content::Text(Arc::new(text.into())),
                time: Arc::new(time.into()),
            }),
        })
    }

    fn ev(name: &str, text: &str, time: &str) -> Arc<IncomingMessageEvent> {
        ev_in("web", "self", "g1", name, text, time)
    }

    /// 构造测试用通道原文副本
    fn rec(user: &str, text: &str, is_self: usize, time: &str, sn: u64) -> Arc<ChannelRecord> {
        Arc::new(ChannelRecord {
            user_id: Arc::new(user.into()),
            self_user_id: Arc::new("self".into()),
            messenger_id: Arc::new("web".into()),
            group_id: Arc::new("g1".into()),
            is_self,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(user.into()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new(text.into())),
            time: Arc::new(time.into()),
            sn,
        })
    }

    fn mc(user_name: &str, text: &str, is_self: bool) -> MessageContent {
        MessageContent {
            user_name: Arc::new(user_name.into()),
            content: vec![Arc::new(text.into())],
            is_self,
        }
    }

    #[test]
    fn event_channel_uses_recipient_user_and_group() {
        // 通道三元组 = messenger + 接收方用户（self_user_id）+ 群组；发送者 user_id 不参与
        let e = ev("u1", "a", "2026-09-23 10:00:00");
        assert_eq!(event_channel(&e), ChannelKey::new("web", "self", "g1"));
    }

    #[test]
    fn locate_message_maps_recipient_to_self_user() {
        let e = ev("u1", "a", "2026-09-23 10:00:00");
        let lm = locate_message(&e);
        assert_eq!(lm.self_user_id.as_str(), "self");
        assert_eq!(lm.user_name.as_str(), "u1");
        assert_eq!(lm.is_self, 0, "输入处理器只处理上行消息");
        assert_eq!(lm.channel(), ChannelKey::new("web", "self", "g1"));
    }

    #[test]
    fn new_message_key_matches_record_identity() {
        let e = ev("u1", "hello", "2026-09-23 10:00:00");
        assert_eq!(new_message_key(&e), message_key("2026-09-23 10:00:00", "u1", "hello", false));
    }

    #[test]
    fn due_channels_only_returns_timed_out() {
        let batches = DashMap::new();
        batches.insert(
            ChannelKey::new("web", "self", "g1"),
            PendingBatch { events: vec![], last_at: Instant::now() },
        );
        batches.insert(
            ChannelKey::new("web", "self", "g2"),
            PendingBatch { events: vec![], last_at: Instant::now() - Duration::from_secs(10) },
        );
        let due = due_channels(&batches, Duration::from_secs(3));
        assert_eq!(due, vec![ChannelKey::new("web", "self", "g2")], "只有超时 channel 被触发");
    }

    #[tokio::test]
    async fn enqueue_buckets_by_channel() {
        let processor = test_processor();
        processor.enqueue(ev("u1", "a", "2026-09-23 10:00:00"));
        processor.enqueue(ev("u2", "b", "2026-09-23 10:00:01"));
        processor.enqueue(ev_in("web", "self", "g2", "u3", "c", "2026-09-23 10:00:02"));
        let g1 = ChannelKey::new("web", "self", "g1");
        let g2 = ChannelKey::new("web", "self", "g2");
        assert_eq!(processor.batches.get(&g1).unwrap().events.len(), 2, "同 channel 合入一批");
        assert_eq!(processor.batches.get(&g2).unwrap().events.len(), 1, "不同 channel 分开攒");
    }

    #[test]
    fn filter_history_windows_time_channel_and_dedups_new_messages() {
        let records = vec![
            rec("u1", "old", 0, "2026-09-23 09:00:00", 1),      // 时间窗外
            rec("u1", "dup", 0, "2026-09-23 10:00:01", 2),      // 与本批新消息重复
            rec("agent", "reply", 1, "2026-09-23 10:00:02", 3), // 本人回复：保留
            rec("u2", "new", 0, "2026-09-23 10:00:03", 4),
            rec("u2", "new", 0, "2026-09-23 10:00:03", 4),      // 同 (time, sn)：去重
        ];
        let new_keys = HashSet::from([message_key("2026-09-23 10:00:01", "u1", "dup", false)]);
        let history = filter_history(&records, "2026-09-23 10:00:00", &new_keys);
        assert_eq!(history.len(), 2, "窗外 1 条 + 重复 1 条 + 本批重复 1 条都被剔除");
        assert!(history[0].is_self, "本人回复保留");
        assert_eq!(history[0].content[0].as_str(), "reply");
        assert!(!history[1].is_self);
        assert_eq!(history[1].content[0].as_str(), "new");
    }

    #[test]
    fn compose_context_puts_summary_first_and_keeps_history() {
        let history = vec![mc("u1", "hi", false), mc("agent", "yo", true)];
        let messages = compose_context(&["事件A".to_string(), "事件B".to_string()], &history);
        assert_eq!(messages.len(), 2, "摘要并入首条 User，不新增消息");
        match &messages[0] {
            Message::User { content } => {
                assert!(content.starts_with("【相关事件摘要】"), "摘要块置首: {}", content);
                assert!(content.contains("- 事件A") && content.contains("- 事件B"));
                assert!(content.ends_with("u1: hi"), "原首条 User 内容保留");
            }
            other => panic!("首条应为 User: {:?}", other),
        }
        assert!(matches!(&messages[1], Message::Assistant { .. }));
    }

    #[test]
    fn compose_context_without_history_or_summaries() {
        // 无摘要无历史 → 空上下文
        assert!(compose_context(&[], &[]).is_empty());
        // 只有摘要 → 单条 User
        let only_summary = compose_context(&["事件A".to_string()], &[]);
        assert_eq!(only_summary.len(), 1);
        assert!(matches!(&only_summary[0], Message::User { content } if content.contains("事件A")));
    }

    fn test_processor() -> AbstractAgentInputProcessor {
        AbstractAgentInputProcessor::assemble(
            Arc::new(SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role }),
            Arc::new(Session::new("", "test-session")),
            Arc::new(AbstractClient::with_url(String::new(), String::new())),
        )
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd kissbot-agent && cargo test abstract_input_processor`
Expected: 编译失败——`AbstractAgentInputProcessor` / 各 free 函数未定义。

- [ ] **Step 3: 实现纯逻辑**

在测试模块之前写入实现（文件顶部 `use` 与类型、free 函数、`assemble`/`enqueue`；`new`/`with_client`/tick 任务属于 Task 4 的运行期）：

```rust
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;
use kissbot_api::{ChannelKey, ChannelRecord, IncomingMessageEvent, LocateMessage};
use kissbot_llm::{Message, Session};
use tokio::sync::Notify;

use crate::{
    abstract_client::AbstractClient,
    message::{MessageContent, extract_content, pack_memory_messages},
    types::SessionKey,
};

/// 消息去重键：时间 + 发送者名 + 文本 + 是否本人（同一条消息在两个来源里等价）
type MessageKey = (String, String, String, bool);

/// 单个 channel 的待处理批次
struct PendingBatch {
    events: Vec<Arc<IncomingMessageEvent>>,
    /// 最后一条消息到达时间（超时判定基准）
    last_at: Instant,
}

pub struct AbstractAgentInputProcessor {
    /// 绑定的会话
    session_key: Arc<SessionKey>,
    /// 会话实例（装配时注入；触发时用于重置上下文）
    session: Arc<Session>,
    /// 摘要记忆客户端（装配时注入；单测可传空地址实例）
    client: Arc<AbstractClient>,
    /// 各 channel 的待处理批次（accept 写入，tick 任务取走）
    batches: Arc<DashMap<ChannelKey, PendingBatch>>,
    /// 会话销毁通知（Drop 时 notify_one → tick 任务退出）
    notify: Arc<Notify>,
}

/// 消息所属通道：messenger + 接收方用户（agent 在该通道绑定的身份）+ 群组；发送者 user_id 不参与
fn event_channel(event: &IncomingMessageEvent) -> ChannelKey {
    ChannelKey {
        messenger_id: event.incoming_message.messenger_id.clone(),
        self_user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
    }
}

/// 已超时的 channel 列表：最后一条消息到达时间距现在已超过 timeout
fn due_channels(batches: &DashMap<ChannelKey, PendingBatch>, timeout: Duration) -> Vec<ChannelKey> {
    batches
        .iter()
        .filter(|entry| entry.value().last_at.elapsed() >= timeout)
        .map(|entry| entry.key().clone())
        .collect()
}

/// 新消息 → 定位请求消息（输入处理器只处理上行消息，is_self 固定 0）
fn locate_message(event: &IncomingMessageEvent) -> LocateMessage {
    LocateMessage {
        time: event.incoming_message.time.clone(),
        user_name: event.incoming_message.user_name.clone(),
        is_self: 0,
        messenger_id: event.incoming_message.messenger_id.clone(),
        self_user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
        content: event.incoming_message.content.clone(),
    }
}

/// 一条消息的去重键（时间 + 发送者 + 文本 + is_self 全同即视为同一条）
fn message_key(time: &str, user_name: &str, text: &str, is_self: bool) -> MessageKey {
    (time.to_string(), user_name.to_string(), text.to_string(), is_self)
}

/// 新消息的去重键（与通道记录的可比字段对齐：时间 / 发送者 / 文本 / is_self）
fn new_message_key(event: &IncomingMessageEvent) -> MessageKey {
    let mc = extract_content(event.incoming_message.user_name.clone(), &event.incoming_message.content, 0);
    let text = join_text(&mc);
    message_key(event.incoming_message.time.as_str(), mc.user_name.as_str(), &text, false)
}

/// 文本拼接（Multi 拆出的多个 Text 段以 \n 连接，与 pack_batch 一致）
fn join_text(mc: &MessageContent) -> String {
    mc.content.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
}

/// 历史记录过滤与去重：只留时间窗内（time >= cutoff）且文本非空的通道记录，
/// 事件之间按 (time, sn) 去重，并与本批新消息去重（查出的消息和新消息做去重）
fn filter_history(
    records: &[Arc<ChannelRecord>],
    cutoff: &str,
    new_keys: &HashSet<MessageKey>,
) -> Vec<MessageContent> {
    let mut sorted: Vec<&Arc<ChannelRecord>> = records.iter().collect();
    sorted.sort_by(|a, b| (a.time.as_str(), a.sn).cmp(&(b.time.as_str(), b.sn)));
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    let mut history = Vec::new();
    for record in sorted {
        if record.time.as_str() < cutoff {
            continue; // 时间窗外的记录不取（只取最后一段时间）
        }
        if !seen.insert((record.time.as_str().to_string(), record.sn)) {
            continue; // 事件之间/尾部重叠的同一条记录
        }
        let mc = extract_content(record.user_name.clone(), &record.content, record.is_self);
        if mc.content.is_empty() {
            continue; // 非文本记录跳过
        }
        let text = join_text(&mc);
        if new_keys.contains(&message_key(record.time.as_str(), record.user_name.as_str(), &text, record.is_self)) {
            continue; // 与本批新消息重复：本批会进 pipeline，历史里去掉
        }
        history.push(mc);
    }
    history
}

/// 组装重置用的上下文：摘要块置首（并入首条 User，避免连续两条 User），其后为历史消息
fn compose_context(summaries: &[String], history: &[MessageContent]) -> Vec<Message> {
    let mut messages = pack_memory_messages(history);
    if summaries.is_empty() {
        return messages;
    }
    let block = format!(
        "【相关事件摘要】\n{}",
        summaries.iter().map(|s| format!("- {}", s)).collect::<Vec<_>>().join("\n")
    );
    if matches!(messages.first(), Some(Message::User { .. })) {
        if let Some(Message::User { content }) = messages.first_mut() {
            *content = Arc::new(format!("{}\n\n{}", block, content.as_str()));
        }
    } else {
        messages.insert(0, Message::User { content: Arc::new(block) });
    }
    messages
}

impl AbstractAgentInputProcessor {
    /// 只搭状态、不启动 tick 任务（生产构造与单测共用；单测不启动可避开进程级单例）
    fn assemble(session_key: Arc<SessionKey>, session: Arc<Session>, client: Arc<AbstractClient>) -> Self {
        Self {
            session_key,
            session,
            client,
            batches: Arc::new(DashMap::new()),
            notify: Arc::new(Notify::new()),
        }
    }

    /// 入批：按 channel 分桶并刷新最后一条到达时间（accept 与单测共用）
    fn enqueue(&self, event: Arc<IncomingMessageEvent>) {
        let key = event_channel(&event);
        let now = Instant::now();
        let mut batch = self.batches.entry(key).or_insert_with(|| PendingBatch { events: Vec::new(), last_at: now });
        batch.events.push(event);
        batch.last_at = now;
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd kissbot-agent && cargo test abstract_input_processor`
Expected: 7 passed（`event_channel_uses_recipient_user_and_group` / `locate_message_maps_recipient_to_self_user` / `new_message_key_matches_record_identity` / `due_channels_only_returns_timed_out` / `enqueue_buckets_by_channel` / `filter_history_windows_time_channel_and_dedups_new_messages` / `compose_context_puts_summary_first_and_keeps_history` / `compose_context_without_history_or_summaries`）。

- [ ] **Step 5: 提交**

```bash
git add kissbot-agent/src/pipelines/abstract_input_processor.rs
git commit -m "新增摘要记忆输入处理器的攒批与上下文装配纯逻辑（channel 分桶、超时判定、时间窗与去重、摘要置首）及单测"
```

---

### Task 4: 输入处理器（运行期：tick 触发与摘要记忆重建）

**Files:**
- Modify: `kissbot-agent/src/pipelines/abstract_input_processor.rs`
- Modify: `kissbot-agent/src/pipelines.rs`

**Interfaces:**
- Consumes: Task 2 的 `AbstractClient`；Task 3 的 `event_channel` / `due_channels` / `locate_message` / `new_message_key` / `filter_history` / `compose_context` / `enqueue` / `PendingBatch`
- Produces:
  - `AbstractAgentInputProcessor::new(session_key: Arc<SessionKey>, session: Arc<Session>) -> Self`
  - `impl AgentInputProcessor for AbstractAgentInputProcessor`（`accept`）
  - `impl AgentTrigger for AbstractAgentInputProcessor`、`impl Drop`
  - `PP_IN_ABSTRACT: &str = "abstract_input_processor"`
  - `create_input_processor(session_key, session, name)`、`create_agent_input_processors(session_key, session)`

- [ ] **Step 1: 写失败测试（运行期构造与降级路径的可测部分）**

在 Task 3 的测试模块里追加：

```rust
    #[tokio::test]
    async fn accept_enqueues_through_trait_entry() {
        // accept（trait 入口）只入批：分桶与最后到达时间刷新见 enqueue
        let processor = test_processor();
        processor.accept(ev("u1", "a", "2026-09-23 10:00:00")).await;
        processor.accept(ev("u2", "b", "2026-09-23 10:00:01")).await;
        let g1 = ChannelKey::new("web", "self", "g1");
        assert_eq!(processor.batches.get(&g1).unwrap().events.len(), 2);
    }

    #[tokio::test]
    async fn build_context_fails_without_abstract_service() {
        // 摘要记忆未配置：build_context 返回 Err（调用方据此降级为普通合批）
        let processor = test_processor();
        let err = build_context(
            &processor.session_key,
            &processor.client,
            &[ev("u1", "a", "2026-09-23 10:00:00")],
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::AbstractError(_)), "未配置摘要记忆应返回 AbstractError: {:?}", err);
    }
```

同时在测试模块的 `use super::*;` 之后补 `use crate::types::Error;`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cd kissbot-agent && cargo test abstract_input_processor`
Expected: 编译失败——`AbstractAgentInputProcessor::accept`（trait 未实现）与 `build_context` 未定义。

- [ ] **Step 3: 实现运行期**

在 `impl AbstractAgentInputProcessor` 里补 `new` / `with_client`，并新增 `TICK_INTERVAL_SECS`、运行期 free 函数、trait 实现与 Drop：

```rust
/// tick 间隔（秒）：每秒定时检测是否有 channel 已超时
const TICK_INTERVAL_SECS: u64 = 1;

impl AbstractAgentInputProcessor {
    /// 生产构造：从进程级配置构造摘要记忆客户端，并启动 tick 任务（装配期在异步上下文中调用）
    pub fn new(session_key: Arc<SessionKey>, session: Arc<Session>) -> Self {
        Self::with_client(session_key, session, Arc::new(AbstractClient::new()))
    }

    /// 显式客户端构造：搭状态并启动 tick 任务
    pub fn with_client(session_key: Arc<SessionKey>, session: Arc<Session>, client: Arc<AbstractClient>) -> Self {
        let processor = Self::assemble(session_key, session, client);
        tokio::spawn(tick_loop(
            processor.session_key.clone(),
            processor.session.clone(),
            processor.client.clone(),
            processor.batches.clone(),
            processor.notify.clone(),
        ));
        processor
    }
}

/// tick 任务：每秒检测一次，逐个触发已超时 channel 的批次（顺序执行 → 同会话上下文不并发写）；
/// 会话销毁（notify）时退出
async fn tick_loop(
    session_key: Arc<SessionKey>,
    session: Arc<Session>,
    client: Arc<AbstractClient>,
    batches: Arc<DashMap<ChannelKey, PendingBatch>>,
    notify: Arc<Notify>,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = notify.notified() => break,
            _ = ticker.tick() => {}
        }
        // 超时阈值取会话配置（与普通合批同一项配置）
        let timeout = Duration::from_secs(
            ConfigManager::get()
                .session_config::<ChannelBatchConfig, ChannelBatchConfig>(&session_key)
                .await
                .channel_batch_interval_secs,
        );
        for channel in due_channels(&batches, timeout) {
            let Some((_, batch)) = batches.remove(&channel) else { continue };
            flush_batch(&session_key, &session, &client, &channel, batch.events).await;
        }
    }
}

/// 触发一个 channel 的批次：摘要记忆重建上下文（失败降级为不重置）→ 本批新消息进 pipeline
async fn flush_batch(
    session_key: &Arc<SessionKey>,
    session: &Arc<Session>,
    client: &Arc<AbstractClient>,
    channel: &ChannelKey,
    events: Vec<Arc<IncomingMessageEvent>>,
) {
    match build_context(session_key, client, &events).await {
        Ok(messages) => session.context_archive_and_clear_cache_and_reset_messages(messages).await,
        Err(e) => warn!(
            "摘要记忆重建上下文失败，降级为普通合批（channel={}:{}:{}）: {:?}",
            channel.messenger_id, channel.self_user_id, channel.group_id, e
        ),
    }
    let _ = Nexus::get().run_pipeline(session_key.as_ref(), pack_batch(&events)).await;
}

/// 用摘要记忆重建上下文：定位事件 → 各事件摘要 + 事件结尾原文副本 → 时间窗与去重 → 组装消息
async fn build_context(
    session_key: &SessionKey,
    client: &Arc<AbstractClient>,
    events: &[Arc<IncomingMessageEvent>],
) -> Result<Vec<Message>> {
    let agent_id = session_key.agent_id.as_str();
    let role = role_mode(session_key.role_name.as_str(), &session_key.mode);
    let cfg = ConfigManager::get()
        .session_config::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(session_key)
        .await;
    // 1. 定位本批消息涉及的事件
    let messages: Vec<LocateMessage> = events.iter().map(|e| locate_message(e.as_ref())).collect();
    let located = client.locate(agent_id, &role, messages).await?;
    // 2. 涉及事件的全部事件摘要
    let mut summaries = Vec::new();
    for event in located.events.iter() {
        let node = client.event_summary(agent_id, &role, event.event_id.as_str()).await?;
        summaries.push(node.summary.as_str().to_string());
    }
    // 3. 各事件结尾若干条原文副本（只保留通道记录）
    let mut records: Vec<Arc<ChannelRecord>> = Vec::new();
    for event in located.events.iter() {
        let tail = client
            .event_records_tail(agent_id, &role, event.event_id.as_str(), cfg.memory_count as u64)
            .await?;
        for record in tail.records.iter() {
            if let EventRecord::Channel(channel_record) = record.as_ref() {
                records.push(channel_record.clone());
            }
        }
    }
    // 4. 最后一段时间 + 与本批新消息去重
    let cutoff = (Local::now() - ChronoDuration::seconds(cfg.memory_time_secs as i64))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let new_keys: HashSet<MessageKey> = events.iter().map(|e| new_message_key(e.as_ref())).collect();
    let history = filter_history(&records, &cutoff, &new_keys);
    // 5. 组装上下文（摘要在前，历史在后）
    Ok(compose_context(&summaries, &history))
}

impl AgentTrigger for AbstractAgentInputProcessor {}

#[async_trait]
impl AgentInputProcessor for AbstractAgentInputProcessor {
    async fn accept(&self, event: Arc<IncomingMessageEvent>) {
        self.enqueue(event);
    }
}

impl Drop for AbstractAgentInputProcessor {
    fn drop(&mut self) {
        // 会话销毁：通知 tick 任务退出（notify_one permit 语义：任务错过唤醒后下一轮 notified 立即完成）
        self.notify.notify_one();
    }
}
```

文件顶部 `use` 区补齐（与 Task 3 的 use 合并；`message::pack_batch` 也要加进 Task 3 那条 `message::{...}`）：

```rust
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Local};
use kissbot_api::EventRecord;
use tracing::warn;

use crate::{
    config_manager::ConfigManager,
    configs::{ChannelBatchConfig, EffectiveMemoryRecoverConfig, MemoryRecoverConfig},
    nexus::Nexus,
    pipeline::{AgentInputProcessor, AgentTrigger},
    types::{Result, role_mode},
};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd kissbot-agent && cargo test abstract_input_processor`
Expected: 9 passed。

- [ ] **Step 5: 注册处理器并注入会话**

`kissbot-agent/src/pipelines.rs`：

```rust
pub const PP_IN_BATCH: &str = "batch_input_processor";
pub const PP_IN_ABSTRACT: &str = "abstract_input_processor";
```

```rust
pub fn create_input_processor(session_key: Arc<SessionKey>, session: Arc<Session>, name: &str) -> Result<Arc<dyn AgentInputProcessor>> {
    match name {
        PP_IN_BATCH => Ok(Arc::new(BatchAgentInputProcessor::new(session_key))),
        PP_IN_ABSTRACT => Ok(Arc::new(AbstractAgentInputProcessor::new(session_key, session))),
        _ => Err(Error::PipelineNotFound("input processor".to_string(), name.to_string(), session_key.as_ref().clone())),
    }
}
```

`create_agent_input_processors` 增加 `session` 形参并传给工厂：

```rust
pub async fn create_agent_input_processors(session_key: Arc<SessionKey>, session: Arc<Session>) -> Result<DashMap<String, Arc<dyn AgentInputProcessor>>> {
```

```rust
    for name in names.drain(..) {
        let input_processor = create_input_processor(session_key.clone(), session.clone(), name.as_str())?;
        input_processors.insert(name, input_processor);
    }
```

`sync_pipeline` 里同步改调用：

```rust
        let input_processors = create_agent_input_processors(session_key.clone(), session.clone()).await?;
```

- [ ] **Step 6: 构建并跑全量测试**

Run: `cd kissbot-agent && cargo build && cargo test`
Expected: 构建成功；全部测试通过（既有 `nexus`/`pipelines` 测试不回归）。

- [ ] **Step 7: 提交**

```bash
git add kissbot-agent/src/pipelines/abstract_input_processor.rs kissbot-agent/src/pipelines.rs
git commit -m "摘要记忆输入处理器接入 pipeline：按 channel 攒批每秒检测超时、触发时经摘要记忆重建上下文并运行新消息，摘要记忆不可用时降级为普通合批；create_input_processor 增加会话注入"
```

---

### Task 5: 文档同步

**Files:**
- Modify: `docs/spec/agent-nexus.md`
- Modify: `docs/spec/agent-modules.md`
- Modify: `docs/design/components-design/kissbot-agent-nexus.md`

**Interfaces:**
- Consumes: Task 1~4 的最终行为与命名（`abstract_input_processor` / `api.memory_struct_abstract_url` / `AbstractClient` / `Error::AbstractError`）
- Produces: 与代码一致的文档

- [ ] **Step 1: agent-nexus.md 补 api 段字段**

`### 公共配置（agent 相关段）` 的 `api` 段示例改为：

```jsonc
  "api": {
    "memory_store_url": "http://127.0.0.1:8082",           // memory-store 地址（空 = 关闭读写）
    "memory_ego_url": "http://127.0.0.1:3001",             // memory-ego 地址（空 = ego 未配置）
    "memory_struct_abstract_url": "http://127.0.0.1:8083"  // 摘要记忆地址（空 = 未部署，输入处理器降级为普通合批）
  },
```

- [ ] **Step 2: agent-nexus.md 补处理器名与接口小节**

在 `说明` 列表里 `input_processor` / `system_prompter` 那条后追加一行：

```markdown
- `input_processor` 可选 `batch_input_processor`（默认，按会话合批）与 `abstract_input_processor`（摘要记忆：按通道攒批 + 超时触发 + 事件上下文重建）。
```

在 `## Memory-Ego API 交互` 之前插入新小节：

````markdown
## Memory-Struct-Abstract API 交互

摘要记忆输入处理器（`abstract_input_processor`，需在 `pipeline.input_processor` 显式启用）经 `AbstractClient` 调用摘要记忆，鉴权用 `X-Api-Key`：

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/abstract/locate` | 定位一批上行消息涉及的事件（请求含 agent_id、role 与带通道三元组的消息） |
| POST | `/abstract/event/summary` | 取事件摘要（节点本身：摘要文本 + 层级 + 记录数 + 起止时间） |
| POST | `/abstract/event/records-tail` | 取事件结尾 count 条原文副本（升序） |

- `role` 与会话写记忆同一编码：角色模式为角色名，事件模式为 `{role}-{event}`（`role_mode`）。
- 触发策略：消息按通道（messenger + 接收方用户 + 群组）攒批，每秒检测一次，最后一条消息超过 `channel_batch.channel_batch_interval_secs` 即触发该通道批次。
- 触发时用「涉及事件的全部摘要 + 各事件结尾 `memory_recover.memory_count` 条、且时间落在 `memory_recover.memory_time_secs` 内的通道原文副本」重置会话上下文；查出的记录与本批新消息按（时间、发送者、文本、is_self）去重，去重后本批新消息整体进 pipeline。
- `memory_struct_abstract_url` 为空或任一调用失败：记 warn，不重置上下文，直接以合批消息运行 pipeline（与 `batch_input_processor` 行为一致）。
````

- [ ] **Step 3: agent-nexus.md 错误类型补一条**

`## 错误类型` 的 `enum Error` 代码块里，在 `MemoryEgoError(String),          // 预留` 之后插入：

```rust
    AbstractError(String),           // 摘要记忆客户端（未配置 / 请求 / 解析 / 业务失败）
```

- [ ] **Step 4: agent-modules.md 补模块与调用关系**

组件清单表格在 `memory_ego_client` 行之后插入：

```markdown
| abstract_client | AbstractClient | 摘要记忆（memory-struct-abstract）REST 客户端：事件定位、事件摘要、事件原文副本尾部查询 | 被 pipelines（abstract_input_processor）调用 |
```

`pipelines` 行的职责列，把「合批输入（mpsc + DelayQueue）在 flush 时回调 `Nexus::run_pipeline`」改为：

```markdown
合批输入（mpsc + DelayQueue）与摘要记忆输入（按通道攒批 + 1s 检测超时，触发时经摘要记忆重建上下文）都在触发时回调 `Nexus::run_pipeline`
```

调用关系总览 mermaid 中，`MS["kissbot-memory-store"]` 之后插入外部节点，并在 `MEC --> EG` 之后补边：

```
        MSA["kissbot-memory-struct-abstract"]
```
```
    ABC["abstract_client<br/>AbstractClient"]
```
```
    PLS --> ABC
    ABC --> MSA
```

- [ ] **Step 5: kissbot-agent-nexus.md 补输入处理器说明**

`### 5. pipeline 体系（会话执行流程）` 里「**输入处理器（AgentInputProcessor）**」这条改为：

```markdown
- **输入处理器（AgentInputProcessor）**：会话消息入口，按名索引可挂多个实现。默认实现为合批处理器——连续到达的消息先入队并刷新合批截止时间，到期后打包为一条 user 消息并请求启动该会话的 pipeline；摘要记忆处理器则按通道（messenger + 接收方用户 + 群组）攒批、每秒检测最后一条是否超时，触发时经摘要记忆定位事件并取事件摘要与结尾原文副本重建上下文，再把本批新消息交给 pipeline，摘要记忆未配置或调用失败时降级为普通合批。
```

`### pipeline 执行流程（agentic loop）` 代码块首行改为：

```
普通消息 → 输入处理（合批：连续消息等待合批间隔后打包为一条 user 消息；摘要记忆：按通道超时触发，先用摘要记忆重建上下文）
```

- [ ] **Step 6: 提交**

```bash
git add docs/spec/agent-nexus.md docs/spec/agent-modules.md docs/design/components-design/kissbot-agent-nexus.md
git commit -m "同步摘要记忆输入处理器文档：api 段新增 memory_struct_abstract_url、摘要记忆接口交互与触发策略、abstract_client 模块与调用关系、错误类型新增 AbstractError"
```

---

### Task 6: 全量验证

**Files:**
- 无新增；只跑验证命令

**Interfaces:**
- Consumes: Task 1~5 的全部改动
- Produces: 构建与测试通过的证据

- [ ] **Step 1: kissbot-api 测试**

Run: `cd kissbot-api && cargo test`
Expected: 全部通过（含 Task 1 的 2 个用例）。

- [ ] **Step 2: kissbot-agent 构建与测试**

Run: `cd kissbot-agent && cargo build && cargo test`
Expected: 构建成功；全部通过（含 Task 3/4 的 9 个用例）。

- [ ] **Step 3: 依赖方构建**

Run: `cd kissbot-agent && cargo build 2>&1 | tail -5`
Expected: `Finished`，无 error。若其他组件（如 `kissbot-memory-manage`）依赖 kissbot-api，另跑一次其构建确认字段新增不破坏编译。

- [ ] **Step 4: 提交（如有验证期修正）**

```bash
git status --short
git add -A
git commit -m "摘要记忆输入处理器全量验证修正"
```

（若无修正则跳过提交。）

---

## Self-Review

**Spec coverage:**
- 按 channel（messenger + 接收方用户 + 群组）攒批 → Task 3 `event_channel` + `enqueue` + `enqueue_buckets_by_channel`。
- 每秒定时检测、最后一条超时才触发 → Task 4 `tick_loop`（`TICK_INTERVAL_SECS = 1`）+ Task 3 `due_channels` 及其用例。
- 触发时消息发往摘要记忆定位事件 → Task 4 `build_context` 第 1 步 `/abstract/locate`。
- 查询对应事件的所有事件摘要 → Task 4 `build_context` 第 2 步 `/abstract/event/summary`（逐个涉及事件）。
- 取最后一段时间最后若干条 → Task 4 第 3 步 `/abstract/event/records-tail`（`memory_count`）+ 第 4 步时间窗（`memory_time_secs`）。
- 用这些数据重置上下文 → Task 4 `context_archive_and_clear_cache_and_reset_messages(compose_context(...))`。
- 查出的消息与新消息去重，去重后本批新消息 run_pipeline → Task 3 `filter_history` + Task 4 `flush_batch` 的 `pack_batch` → `Nexus::run_pipeline`。
- 已确认的三项决策（只注册不改 preset、api 段新地址、失败降级）分别落在 Task 4 Step 5、Task 1、Task 4 `flush_batch` 的 Err 分支。

**Placeholder scan:** 无 TBD/TODO；每个改动步骤都给了完整代码或精确 diff。

**Type consistency:** `MessageKey`（4 元组）在 `message_key` / `new_message_key` / `filter_history` 三处一致；`PendingBatch { events, last_at }` 在 Task 3 定义、Task 3/4 使用一致；`build_context(session_key, client, events)` 的签名在 Task 4 的 `flush_batch` 调用与测试中一致；`assemble`（Task 3）被 Task 3 测试与 Task 4 `with_client` 共用，`new` / `with_client` 只在 Task 4 工厂与生产路径使用；`create_input_processor(session_key, session, name)` 三参签名在定义与 `create_agent_input_processors` 调用处一致。
