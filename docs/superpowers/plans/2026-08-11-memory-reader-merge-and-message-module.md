# MemoryReader 并入 MemoryStoreClient + message 模块提取实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除 MemoryReader（读取并入 MemoryStoreClient，共享 StoreHttpConfig），提取 message.rs 模块（MessageContent + 打包），BatchConsumer 复用批打包。

**Architecture:** StoreHttpConfig 增加返回响应体的查询方法；MemoryStoreClient 持 `config: Arc<StoreHttpConfig>` 并承担读（read_recent_for_context）+ 写；新模块 kissbot-agent/src/message.rs 集中消息内容组装（MessageContent、extract_content、pack_memory_messages、pack_batch）；coordinator 只用 MemoryStoreClient；session_manager 的 BatchConsumer 改调 pack_batch。

**Tech Stack:** Rust（binary crate kissbot-agent，无 workspace；kissbot-api 提供类型），reqwest/axum（测试 mock），kai-file（写路径 appender）。

## Global Constraints

- 不删除代码中的注释（被整体删除的代码块连同其注释可删，如 memory-struct 调用块；不允许只删注释留代码）
- 提交 comment 用中文，包含该提交所有改动
- 不新增 clippy warning（`cargo clippy --all-targets` 的 warning 数不增）
- 文本文件 UTF-8、\n 换行
- 测试在 kissbot-agent 目录内跑：`cargo test <过滤词>`
- kissbot-agent 是 binary crate：模块声明在 src/main.rs

---

### Task 1: StoreHttpConfig 读方法 + MemoryStoreClient 共享 config 字段

**Files:**
- Modify: `kissbot-agent/src/memory_store_client.rs`（StoreHttpConfig 加 send_store_query；MemoryStoreClient 加 config 字段 + with_config 构造；tests 加两个测试）

**Interfaces:**
- Consumes: `StoreHttpConfig { client: Client, base_url: String, api_key: Arc<String> }`（现有私有字段）、`send_store_request`（现有写方法）
- Produces: `StoreHttpConfig::send_store_query<T: serde::de::DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> std::result::Result<T, kai_file::Error>`；`MemoryStoreClient::with_config(config: Arc<StoreHttpConfig>) -> Self`（私有，同文件测试可用）；`MemoryStoreClient { config: Arc<StoreHttpConfig>, ... }`（私有字段）

- [ ] **Step 1: 写失败测试（send_store_query 成功反序列化 + 非 2xx 报错）**

在 `kissbot-agent/src/memory_store_client.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
    #[tokio::test]
    async fn send_store_query_deserializes_response_and_errors_on_non_2xx() {
        use axum::routing::post;
        use axum::{Json, Router};
        use serde_json::json;
        use tokio::net::TcpListener;

        // 本地 mock：/ok 返回 JSON 体；/bad 返回 400
        let app = Router::new()
            .route("/ok", post(|| async { Json(json!({ "success": true, "data": { "k": 1 } })) }))
            .route("/bad", post(|| async { (axum::http::StatusCode::BAD_REQUEST, "boom") }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = StoreHttpConfig {
            client: Client::new(),
            base_url: format!("http://{}", addr),
            api_key: Arc::new("k".into()),
        };
        // 成功：响应体反序列化（含鉴权头由 mock 忽略）
        let v: serde_json::Value = config.send_store_query("/ok", &json!({})).await.unwrap();
        assert_eq!(v["data"]["k"], 1, "响应体应反序列化返回");
        // 非 2xx → Err（含状态码）
        let rst = config.send_store_query::<serde_json::Value>("/bad", &json!({})).await;
        assert!(rst.is_err(), "非 2xx 应报错");
    }

    #[tokio::test]
    async fn send_store_query_empty_base_url_errors() {
        // base_url 空 → 相对路径请求 → reqwest 报错（读路径无 store 配置不可静默跳过，与写路径 Ok 跳过语义不同）
        let config = StoreHttpConfig {
            client: Client::new(),
            base_url: String::new(),
            api_key: Arc::new("k".into()),
        };
        let rst = config.send_store_query::<serde_json::Value>("/x", &serde_json::json!({})).await;
        assert!(rst.is_err(), "base_url 空应报错");
    }
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cd kissbot-agent && cargo test send_store_query`
Expected: FAIL——`error[E0599]: no method named 'send_store_query' found for struct 'StoreHttpConfig'`

- [ ] **Step 3: 实现 send_store_query**

在 `StoreHttpConfig` 的 impl 内、`send_store_request` 之后追加（共用 client/HEADER_API_KEY/base_url 字段）：

```rust
    /// 查询请求：POST {base_url}{path}，带 X-Api-Key 鉴权头，返回反序列化后的响应体；
    /// 非 2xx 返回 Err（错误含状态码与返回体）；base_url 为空时 URL 为相对路径 → reqwest 报错
    /// （读路径无 store 配置不可静默跳过，与写路径 Ok 跳过语义不同）
    async fn send_store_query<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> std::result::Result<T, kai_file::Error> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let response = self.client.post(&url)
            .header(HEADER_API_KEY, self.api_key.as_str())
            .json(body)
            .send()
            .await
            .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?;
        if !response.status().is_success() {
            let status = response.status();
            let msg = response.text().await.unwrap_or_default();
            return Err(kai_file::Error::WriteError(format!("[{}] {}", status, msg)));
        }
        response.json::<T>().await
            .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))
    }
```

- [ ] **Step 4: 运行测试验证通过**

Run: `cd kissbot-agent && cargo test send_store_query`
Expected: PASS（2 passed）

- [ ] **Step 5: MemoryStoreClient 加 config 字段 + with_config 重构（保持现有行为）**

结构体加字段：

```rust
pub struct MemoryStoreClient {
    /// 共享 HTTP 配置（与各 context 同一 Arc；读路径 read_recent_for_context 也经它发请求）
    config: Arc<StoreHttpConfig>,
    channel_appender: FileObjectAppender<String, ChannelRequest, StoreSender<ChannelStoreContext>, ChannelStoreContext, LoggingErrorHandler>,
    think_appender: FileObjectAppender<String, ThinkRequest, StoreSender<ThinkStoreContext>, ThinkStoreContext, LoggingErrorHandler>,
    tool_call_appender: FileObjectAppender<String, ToolCallRequest, StoreSender<ToolCallStoreContext>, ToolCallStoreContext, LoggingErrorHandler>,
    tool_result_appender: FileObjectAppender<String, ToolResultRequest, StoreSender<ToolResultStoreContext>, ToolResultStoreContext, LoggingErrorHandler>,
}
```

把 `pub fn new() -> Self` 替换为 with_config + new 两段（config 克隆进 self 与各 context，最后一份 move 给 tool_result）：

```rust
    /// 指定共享 HTTP 配置构造（new() 内部走此路径；测试注入 mock store 地址用）
    fn with_config(config: Arc<StoreHttpConfig>) -> Self {
        // 共享 HTTP 配置：self 与各类型 context 经 Arc 同源引用（client / base_url / api_key 一份）
        Self {
            config: config.clone(),
            channel_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ChannelStoreContext { config: config.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            think_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ThinkStoreContext { config: config.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_call_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolCallStoreContext { config: config.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_result_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolResultStoreContext { config })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
        }
    }

    pub fn new() -> Self {
        // 共享 HTTP 配置：各类型 context 经 Arc 同源引用（client / base_url / api_key 一份）
        Self::with_config(Arc::new(StoreHttpConfig::new()))
    }
```

- [ ] **Step 6: 全量测试 + clippy**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -3 && cargo clippy --all-targets 2>&1 | grep -c "warning:"`
Expected: 全绿（116 + 2 新增 = 118 passed）；clippy warning 数 ≤ 55

- [ ] **Step 7: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/memory_store_client.rs
git commit -m "refactor(agent): StoreHttpConfig 新增 send_store_query（返回响应体），MemoryStoreClient 增加共享 config 字段与 with_config 构造（读路径并入的准备）"
```

---

### Task 2: message.rs 模块（MessageContent + extract_content + 打包函数）

**Files:**
- Create: `kissbot-agent/src/message.rs`
- Modify: `kissbot-agent/src/memory_reader.rs`（删 MemoryMsg/pack_memory_messages/collect_text_parts，改用 message.rs；parse 用 extract_content；测试改 import）
- Modify: `kissbot-agent/src/coordinator.rs:18`（import 改）
- Modify: `kissbot-agent/src/main.rs`（加 `mod message;`）

**Interfaces:**
- Consumes: `kissbot_api::channel::IncomingMessageEvent`、`kissbot_api::message::Content`（变体：Text(Arc<String>)/Multi(Vec<Content>)/ToolCall(Arc<String>) 等）、`crate::types::Message`（User{content: Arc<String>}/Assistant{content, reasoning_content, tool_calls}）
- Produces: `pub struct MessageContent { pub user_name: Arc<String>, pub content: Vec<Arc<String>>, pub is_self: bool }`；`pub fn extract_content(user_name: &Arc<String>, is_self: u32, content: &Content) -> MessageContent`；`pub fn pack_memory_messages(msgs: &[MessageContent]) -> Vec<Message>`；`pub fn pack_batch(events: &[Arc<IncomingMessageEvent>]) -> Message`

- [ ] **Step 1: 写 message.rs 完整内容（含测试）**

创建 `kissbot-agent/src/message.rs`：

```rust
use std::sync::Arc;

use kissbot_api::channel::IncomingMessageEvent;
use kissbot_api::message::Content;

use crate::types::Message;

/// 消息内容（channel record 的最小视图：name + 文本段 + is_self；id 类不保留；时间由排序键承担，不保留）
#[derive(Debug, Clone)]
pub struct MessageContent {
    pub user_name: Arc<String>,
    /// 文本段列表：每个元素为 Content::Text 的 Arc 克隆（Multi 拆多个元素；打包时以 \n 拼接）
    pub content: Vec<Arc<String>>,
    /// 是否 agent 自身消息（is_self：1=self，0=他人）
    pub is_self: bool,
}

/// parse_channel_groups 与 pack_batch 共用的 MessageContent 构造：
/// user_name 克隆 Arc；content 递归提取 Text 段（collect_text_parts）；is_self 按 >0 转 bool
pub fn extract_content(user_name: &Arc<String>, is_self: u32, content: &Content) -> MessageContent {
    let mut parts = Vec::new();
    collect_text_parts(content, &mut parts);
    MessageContent { user_name: user_name.clone(), content: parts, is_self: is_self > 0 }
}

/// 递归收集 Content 中的全部 Text 段到 parts：Text 直接 clone 其 Arc<String>；Multi 递归遍历（可嵌套任意深度），
/// 各 Text 子项各占一个元素；其余变体（附件/系统通知/Think/ToolCall/ToolResult 等）不产生段（调用方跳过空结果）
/// 注意：Content 用 #[serde(tag="msg_type", content="data")] 序列化，反序列化后为类型化枚举，直接匹配即可
fn collect_text_parts(content: &Content, parts: &mut Vec<Arc<String>>) {
    match content {
        Content::Text(text) => parts.push(text.clone()),
        Content::Multi(items) => {
            for item in items {
                collect_text_parts(item, parts);
            }
        }
        _ => {}
    }
}

/// is_self=0 的单行格式（pack_memory_messages 的 User 分支与 pack_batch 共用）：
/// name 为空只留 text，否则 "name: text"
fn user_line(user_name: &str, text: &str) -> String {
    if user_name.is_empty() {
        text.to_string()
    } else {
        format!("{}: {}", user_name, text)
    }
}

/// 打包记忆消息为交替的 User/Assistant 消息序列：
/// content 为空的记录（非文本）跳过；
/// 找到第一条非 self（User）消息（之前的 self 消息丢弃，对话必须以 User 开头），
/// 连续同 is_self 的记录合并为一条消息，User/Assistant 交替；
/// User 段 content 逐行 "name: text"（name 为空只留 text），Assistant 段只保留 content（不含 name/time）；
/// 若以 User 结尾则补一条 Content 为空的 Assistant；空输入/无 User 返回空 Vec
pub fn pack_memory_messages(msgs: &[MessageContent]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    let mut user_buf: Vec<String> = Vec::new();
    let mut asst_buf: Vec<String> = Vec::new();
    let mut is_asst = false;  // 当前段类型（false=User 段）
    let mut started = false;  // 是否已开始对话（找到第一条非 self 消息）；此前 self 全部丢弃
    for m in msgs {
        if m.content.is_empty() {
            continue;  // 非文本记录（空 content）跳过
        }
        if !started {
            // 对话必须以 User 开头：丢弃前导 self
            if m.is_self {
                continue;
            }
            started = true;
            is_asst = false;
        }
        if m.is_self != is_asst {
            // 段类型切换：flush 上一段（连续同 is_self 已合并）
            if is_asst {
                out.push(Message::Assistant { content: Arc::new(asst_buf.join("\n")), reasoning_content: None, tool_calls: None });
            } else {
                out.push(Message::User { content: Arc::new(user_buf.join("\n")) });
            }
            user_buf.clear();
            asst_buf.clear();
            is_asst = m.is_self;
        }
        // 拼接文本提前算一遍（is_self 与 user_line 两分支共用；元素为 Content::Text 的 Arc 克隆）
        let text = m.content.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
        if m.is_self {
            asst_buf.push(text);  // Assistant：只要 content，不带 name/time
        } else {
            user_buf.push(user_line(m.user_name.as_str(), &text));  // User：name 空只留 content，否则 "name: text"
        }
    }
    // flush 最后一段（仅当已开始对话）
    if started {
        if is_asst {
            out.push(Message::Assistant { content: Arc::new(asst_buf.join("\n")), reasoning_content: None, tool_calls: None });
        } else {
            out.push(Message::User { content: Arc::new(user_buf.join("\n")) });
            // 以 User 结尾：补一条空 Assistant（模型对话需以待回答的 Assistant 结尾）
            out.push(Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: None });
        }
    }
    out
}

/// 将一批 IncomingMessageEvent 打包为一条 User Message（替换 session_manager BatchConsumer 的内联拼接）：
/// 每 event 经 extract_content 构造（is_self=0），空 content（非文本）跳过，逐行 user_line 拼接（\n 连接）；
/// 全部跳过时返回空 content 的 User（try_flush 已在 items 为空时提前返回，此处输入必非空）
pub fn pack_batch(events: &[Arc<IncomingMessageEvent>]) -> Message {
    let mut lines: Vec<String> = Vec::new();
    for e in events {
        let mc = extract_content(&e.incoming_message.user_name, 0, &e.incoming_message.content);
        if mc.content.is_empty() {
            continue;  // 非文本事件跳过（与 pack_memory_messages 的 is_self=0 处理一致）
        }
        let text = mc.content.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
        lines.push(user_line(mc.user_name.as_str(), &text));
    }
    Message::User { content: Arc::new(lines.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;

    // 构造测试用 MessageContent
    fn msg(name: &str, content: &str, is_self: bool) -> MessageContent {
        MessageContent { user_name: Arc::new(name.to_string()), content: vec![Arc::new(content.to_string())], is_self }
    }

    #[test]
    fn pack_memory_messages_alternates_merges_and_appends_empty_assistant() {
        let msgs = vec![
            msg("agent", "a0", true),   // 开头 self → 丢弃（对话必须以 User 开头）
            msg("u1", "m0", false),
            msg("", "m1", false),       // 空 name → 只留 content
            msg("agent", "line1", true),
            msg("agent", "line2", true),
            msg("u3", "m2", false),
        ];
        let out = pack_memory_messages(&msgs);
        // 期望：[User("u1: m0\nm1"), Assistant("line1\nline2"), User("u3: m2"), Assistant("")]
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u1: m0\nm1"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.as_str() == "line1\nline2"));
        assert!(matches!(&out[2], Message::User { content } if content.as_str() == "u3: m2"));
        assert!(matches!(&out[3], Message::Assistant { content, .. } if content.is_empty()));
    }

    #[test]
    fn pack_memory_messages_empty_or_all_self_returns_empty() {
        assert!(pack_memory_messages(&[]).is_empty());
        // 全 self（无 User 开头）→ 无可打包
        assert!(pack_memory_messages(&[msg("agent", "a", true), msg("agent", "b", true)]).is_empty());
    }

    #[test]
    fn pack_memory_messages_skips_empty_content_records() {
        // 非文本记录（content 为空 Vec）→ 跳过：不产生消息、不触发段切换、不阻止后续合并
        let empty = || MessageContent { user_name: Arc::new("agent".to_string()), content: vec![], is_self: true };
        let msgs = vec![
            msg("u1", "hi", false),
            empty(),                       // 空 content → 跳过（夹在两条 User 之间也不切段）
            msg("u2", "there", false),
            msg("agent", "ok", true),
            empty(),                       // 结尾空 content → 跳过（不影响末尾 Assistant flush）
        ];
        let out = pack_memory_messages(&msgs);
        // 期望：[User("u1: hi\nu2: there"), Assistant("ok")]
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u1: hi\nu2: there"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.as_str() == "ok"));
    }

    #[test]
    fn pack_memory_messages_single_user_appends_empty_assistant() {
        let out = pack_memory_messages(&[msg("u", "hi", false)]);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u: hi"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.is_empty()));
    }

    #[test]
    fn extract_content_collects_text_parts_and_is_self_flag() {
        // Text → 单段；is_self > 0 → true
        let mc = extract_content(&Arc::new("u".to_string()), 1, &Content::Text(Arc::new("hi".into())));
        assert_eq!(mc.content, vec![Arc::new("hi".to_string())]);
        assert!(mc.is_self, "is_self>0 → true");
        // Multi 递归收集 Text 段；非文本子项跳过
        let multi = Content::Multi(vec![
            Content::Text(Arc::new("a".into())),
            Content::ToolCall(Arc::new("k".into())),
            Content::Multi(vec![Content::Text(Arc::new("b".into()))]),
        ]);
        let mc2 = extract_content(&Arc::new("u".to_string()), 0, &multi);
        assert_eq!(mc2.content, vec![Arc::new("a".to_string()), Arc::new("b".to_string())]);
        assert!(!mc2.is_self, "is_self=0 → false");
        // 非文本 → 空 content
        let mc3 = extract_content(&Arc::new("u".to_string()), 0, &Content::ToolCall(Arc::new("k".into())));
        assert!(mc3.content.is_empty());
    }

    // 构造测试用 IncomingMessageEvent（content 由调用方指定）
    fn ev(name: &str, content: Content) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: Arc::new("self".into()),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: Arc::new("m".into()),
                messenger_id: Arc::new("web".into()),
                user_id: Arc::new("u".into()),
                group_id: Arc::new("g".into()),
                messenger_name: Arc::new("".into()),
                user_name: Arc::new(name.into()),
                group_name: Arc::new("".into()),
                content,
                time: Arc::new("2026-08-07 10:00:00".into()),
            }),
        })
    }

    #[test]
    fn pack_batch_joins_user_lines_and_skips_non_text() {
        let events = vec![
            ev("u1", Content::Text(Arc::new("a".into()))),
            ev("", Content::Text(Arc::new("b".into()))),
            ev("u3", Content::ToolCall(Arc::new("k".into()))),  // 非文本 → 跳过
            ev("u4", Content::Text(Arc::new("c".into()))),
        ];
        let msg = pack_batch(&events);
        assert!(matches!(&msg, Message::User { content } if content.as_str() == "u1: a\nb\nu4: c"), "非文本事件跳过，空 name 只留 text");

        // 全非文本 → 空 content 的 User
        let all_empty = vec![ev("u", Content::ToolCall(Arc::new("k".into())))];
        let msg2 = pack_batch(&all_empty);
        assert!(matches!(&msg2, Message::User { content } if content.is_empty()));
    }
}
```

- [ ] **Step 2: main.rs 声明模块**

`kissbot-agent/src/main.rs` 在 `mod memory_store_client;` 行后追加：

```rust
mod message;
```

- [ ] **Step 3: memory_reader.rs 切换到 message.rs 类型**

编辑 `kissbot-agent/src/memory_reader.rs`：
1. 删除 `MemoryMsg` 结构体定义、`pack_memory_messages` 函数、`collect_text_parts` 函数（全部移入 message.rs）
2. import 追加：

```rust
use crate::message::{MessageContent, extract_content};
```

3. `read_recent_for_context` 返回类型 `Result<Vec<MemoryMsg>>` → `Result<Vec<MessageContent>>`；内部 `BTreeMap<(String, u64), MemoryMsg>` → `BTreeMap<(String, u64), MessageContent>`
4. `parse_channel_groups` 签名与 body：

```rust
/// 解析 query 响应分组 → 写入 BTreeMap：(time, sn) 为唯一键（sn 为同文件内唯一序号），
/// 同键重复记录只保留第一条（两查询共用 map → 并集自动去重，迭代天然按 (time, sn) 升序）
/// 文本提取走 extract_content（递归收集全部 Text 段）
fn parse_channel_groups(groups: QueryChannelData, map: &mut BTreeMap<(String, u64), MessageContent>) {
    for (_, records) in groups {
        for (_, rec) in records {
            let time = rec.time.as_str().to_string();  // 键用（MessageContent 值不再保留 time）
            let sn = rec.sn;
            let key = (time, sn);
            // 同 (time, sn) 已存在（两查询并集在 ln 处重叠）→ 提前跳过，免去 user_name clone 与文本提取
            if map.contains_key(&key) {
                continue;
            }
            map.insert(key, extract_content(&rec.user_name, rec.is_self, &rec.content));
        }
    }
}
```

5. 测试模块：删除 `msg` 辅助函数与 4 个 `pack_memory_messages_*` 测试（已移入 message.rs）；import 追加 `use crate::message::pack_memory_messages;`（`read_recent_keeps_non_text_as_empty_content` 与 `read_recent_extracts_is_self_and_packs_alternating` 仍调用它）

- [ ] **Step 4: coordinator import 切换**

`kissbot-agent/src/coordinator.rs:18`：

```rust
use crate::memory_reader::{MemoryReader, pack_memory_messages};
```
改为：
```rust
use crate::memory_reader::MemoryReader;
use crate::message::pack_memory_messages;
```

- [ ] **Step 5: 全量测试 + clippy**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -3 && cargo clippy --all-targets 2>&1 | grep -c "warning:"`
Expected: 全绿（118 个，其中 message 新增 6 个：pack 4 + extract_content 1 + pack_batch 1；memory_reader 减 4 个 pack 测试 → 净 +2）；clippy ≤ 55

- [ ] **Step 6: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/message.rs kissbot-agent/src/memory_reader.rs kissbot-agent/src/coordinator.rs kissbot-agent/src/main.rs
git commit -m "refactor(agent): 新增 message.rs 模块（MessageContent 改名自 MemoryMsg、extract_content 共用构造、pack_memory_messages、pack_batch 批打包），memory_reader 切换使用，测试随迁"
```

---

### Task 3: session_manager BatchConsumer 复用 pack_batch

**Files:**
- Modify: `kissbot-agent/src/session_manager.rs`（import 与 try_flush 内联拼接替换）

**Interfaces:**
- Consumes: `crate::message::pack_batch(&[Arc<IncomingMessageEvent>]) -> Message`（恒返回 `Message::User { content: Arc<String> }`）
- Produces: 无新接口（try_flush 行为不变：drain → 打包 → accept_batch）

- [ ] **Step 1: 改 import**

`kissbot-agent/src/session_manager.rs:18`：

```rust
use crate::coordinator::{AgentCoordinator, extract_text};
```
改为：
```rust
use crate::coordinator::AgentCoordinator;
use crate::message::pack_batch;
```

- [ ] **Step 2: 替换 try_flush 内联拼接**

`try_flush` 中：

```rust
        // 打包为一条 user 消息的 content（内联 pack_events）：逐行 "name: text"（name 为空只留 text）
        let content = items.iter().map(|e| {
            let name = e.incoming_message.user_name.as_str();
            let text = extract_text(&e.incoming_message.content);
            if name.is_empty() { text } else { format!("{}: {}", name, text) }
        }).collect::<Vec<_>>().join("\n");
        session.accept_batch(content).await;
```
替换为：
```rust
        // 打包为一条 user 消息的 content（复用 message::pack_batch：extract_content + user_line + 空 content 跳过）
        let content = match pack_batch(&items) {
            Message::User { content } => (*content).clone(),
            _ => unreachable!("pack_batch 恒返回 User"),
        };
        session.accept_batch(content).await;
```

- [ ] **Step 3: 全量测试 + clippy**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -3 && cargo clippy --all-targets 2>&1 | grep -c "warning:"`
Expected: 全绿（118）；clippy ≤ 55（coordinator.rs 的 extract_text 仍被 handle_incoming 使用，无 dead code）

- [ ] **Step 4: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/session_manager.rs
git commit -m "refactor(agent): BatchConsumer.try_flush 改调 message::pack_batch 打包为单条 User（删除内联拼接与 extract_text 依赖）"
```

---

### Task 4: MemoryReader 删除并合并到 MemoryStoreClient

**Files:**
- Modify: `kissbot-agent/src/memory_store_client.rs`（读路径：read_recent_for_context + parse_channel_groups + QueryChannelData；tests 迁移 11 个 read 测试）
- Modify: `kissbot-agent/src/coordinator.rs`（删 memory_reader 字段/构造/import + memory-struct 调用块；build_role_context 改调 memory_store_client）
- Delete: `kissbot-agent/src/memory_reader.rs`
- Modify: `kissbot-agent/src/main.rs`（删 `mod memory_reader;`）

**Interfaces:**
- Consumes: Task 1 的 `MemoryStoreClient::with_config`、`StoreHttpConfig::send_store_query`；Task 2 的 `message::extract_content`/`MessageContent`；现有 `EffectiveContextConfig`（config_manager）、`Error::MemoryTimeWindow`/`Error::MemoryStoreError`（types）
- Produces: `MemoryStoreClient::read_recent_for_context(&self, agent_id: Arc<String>, role_name: Arc<String>, cfg: &EffectiveContextConfig) -> Result<Vec<MessageContent>>`（crate::types::Result）；私有 `parse_channel_groups`、`type QueryChannelData`

- [ ] **Step 1: memory_store_client.rs 加读路径**

1. import 区追加：

```rust
use std::collections::BTreeMap;

use kissbot_api::memory::{
    ChannelRecord, ChannelRequest, ChannelRequests, QueryRequest, RecentQuery, RecordKey,
    ThinkRequest, ThinkRequests, ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests,
};
use kissbot_api::ApiResponse;

use crate::config_manager::EffectiveContextConfig;
use crate::message::{MessageContent, extract_content};
use crate::types::{Error, Result};
```

2. `type QueryChannelData` 与 `read_recent_for_context`、`parse_channel_groups` 追加到 `MemoryStoreClient` impl 之后（文件级，同 memory_reader.rs 的布局——QueryChannelData 类型与 parse 为私有自由项）：

```rust
/// query/channel 响应 data 类型：Vec<(RecordKey, Vec<(sn, ChannelRecord)>)>（组 → 记录列表）
type QueryChannelData = Vec<(RecordKey, Vec<(u32, ChannelRecord)>)>;
```

3. 在 `impl MemoryStoreClient` 内（push 方法之后）加：

```rust
    /// role 模式记忆打包：两次查询并集——① RecentQuery 最近 N 条（无时间参数）→ ln = 最旧一条 time；
    /// ② QueryRequest 时间范围 [M, ln]（M = min(时间窗起点, ln)，无 limit，M == ln 时退化为单点取 ln 同时间组）；
    /// 结果 = ① ∪ ②（两次解析共用同一 BTreeMap，键 (time, sn) → 去重 + 天然时间正序），升序返回
    /// （原 MemoryReader.read_recent_for_context 迁入；HTTP 经共享 config.send_store_query 发）
    pub async fn read_recent_for_context(
        &self,
        agent_id: Arc<String>,
        role_name: Arc<String>,
        cfg: &EffectiveContextConfig,
    ) -> Result<Vec<MessageContent>> {
        // 时间窗起点计算失败（checked_sub_signed 溢出）→ 直接报错，不静默回退
        let start = chrono::Local::now()
            .checked_sub_signed(chrono::Duration::seconds(cfg.memory_time_secs as i64))
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .ok_or_else(|| Error::MemoryTimeWindow("计算记忆时间窗起点失败".to_string()))?;

        let count = cfg.memory_count;

        // ===== Query1：POST {store}/store/query/channel/recent（RecentQuery，无时间参数） =====
        let body = RecentQuery {
            agent_id: agent_id.clone(),
            role_name: role_name.clone(),
            count: count as u32,
        };
        // 响应反序列化为类型化 ApiResponse<QueryChannelData>（tuple 由 serde 解析，无需手拼索引）
        let resp: ApiResponse<QueryChannelData> = self.config
            .send_store_query("/store/query/channel/recent", &body).await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        let groups = resp.data.unwrap_or_default();

        // ===== 解析：BTreeMap 以 (time, sn) 为键同时做去重与排序（迭代天然升序） =====
        // 两次查询共用同一 map → 并集自动去重（Query2 的 [M, ln] 区间与 Query1 尾部在 ln 处重叠）
        let mut map: BTreeMap<(String, u64), MessageContent> = BTreeMap::new();
        parse_channel_groups(groups, &mut map);

        // ln = 已解析记录最旧一条的 time（BTreeMap 首键）；map 为空说明无记录（count=0 时 Query1 必为空）
        // 一并在此处理；Query1 最旧若为非文本，其同时间组除非文本外无其他记录（否则文本记录会占据更小首键）
        let Some((ln, _)) = map.keys().next() else {
            return Ok(Vec::new());
        };
        let ln = ln.clone();
        // 不足 N 条：直接返回（无需 Query2）。理论上去重后记录不重复，map 大小即取到的数量；
        // 注意：以解析后文本数（map.len()）判定——非文本记录在最后 N 内被跳过时也会提前返回
        if map.len() < count {
            return Ok(map.into_values().collect());
        }
        // M = min(时间窗起点 start, ln)
        let m = if start.as_str() < ln.as_str() { start } else { ln.clone() };  // M = min(cutoff, ln)

        // ===== Query2：POST {store}/store/query/channel（QueryRequest 时间范围 [M, ln]，无 limit） =====
        // M == ln 时退化为单点 [ln, ln]（取 ln 同时间组）；与 Query1 并集（共用 map 去重）
        let body = QueryRequest {
            agent_id,
            role_name,
            start_time: Arc::new(m),
            end_time: Arc::new(ln),
        };
        let resp: ApiResponse<QueryChannelData> = self.config
            .send_store_query("/store/query/channel", &body).await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        parse_channel_groups(resp.data.unwrap_or_default(), &mut map);
        Ok(map.into_values().collect())
    }
```

4. 文件级追加 parse_channel_groups：

```rust
/// 解析 query 响应分组 → 写入 BTreeMap：(time, sn) 为唯一键（sn 为同文件内唯一序号），
/// 同键重复记录只保留第一条（两查询共用 map → 并集自动去重，迭代天然按 (time, sn) 升序）
/// 文本提取走 extract_content（递归收集全部 Text 段）
fn parse_channel_groups(groups: QueryChannelData, map: &mut BTreeMap<(String, u64), MessageContent>) {
    for (_, records) in groups {
        for (_, rec) in records {
            let time = rec.time.as_str().to_string();  // 键用（MessageContent 值不再保留 time）
            let sn = rec.sn;
            let key = (time, sn);
            // 同 (time, sn) 已存在（两查询并集在 ln 处重叠）→ 提前跳过，免去 user_name clone 与文本提取
            if map.contains_key(&key) {
                continue;
            }
            map.insert(key, extract_content(&rec.user_name, rec.is_self, &rec.content));
        }
    }
}
```

- [ ] **Step 2: coordinator 改造**

1. import（coordinator.rs:18）改为：

```rust
use crate::message::pack_memory_messages;
```

2. 结构体删字段 `memory_reader: Arc<MemoryReader>,`
3. `new()` 删 `let memory_reader = Arc::new(MemoryReader::new());` 与 Self 字面量里的 `memory_reader,` 一行
4. `ensure_session` 删 memory-struct 调用块（含其注释）：

```rust
            // 顶层记忆索引（memory-struct 未实现时静默跳过）
            let _ = self.memory_reader
                .read_memory_struct_index(&self.config, session.agent_id.as_str(), &session.role_name, &session.mode)
                .await;
```

5. `build_role_context` 改为：

```rust
        let new_messages = self.memory_store_client
            .read_recent_for_context(session.agent_id.clone(), session.role_name.clone(), &cfg).await
            .map_or_else(|_| vec![], |msgs| pack_memory_messages(&msgs));
```

- [ ] **Step 3: 删除 memory_reader.rs + main.rs 声明**

```bash
cd /home/admin/project/kissbot
git rm kissbot-agent/src/memory_reader.rs
```
`kissbot-agent/src/main.rs` 删除 `mod memory_reader;` 行。

- [ ] **Step 4: 迁移 11 个 read 测试到 memory_store_client.rs tests**

从 `git show 4dbad30:kissbot-agent/src/memory_reader.rs` 取原测试内容（或按 Task 2 后形态），迁入 `memory_store_client.rs` 的 `#[cfg(test)] mod tests`：

1. 原 memory_reader.rs tests 的辅助函数整体搬入：`ctx_config`、`time_ago`、`channel_record_json`、`record_json`、`record_json_self`、`channel_data`、`start_mock_store`（其 axum/TcpListener import 需在 memory_store_client.rs tests 内补齐）
2. `reader_at` 替换为：

```rust
    // 测试构造：指定 memory-store 根地址（覆盖 new() 的 ApiConfig 读取，避免与 http_server 测试的全局配置冲突）
    fn client_at(url: &str) -> MemoryStoreClient {
        MemoryStoreClient::with_config(Arc::new(StoreHttpConfig {
            client: Client::new(),
            base_url: url.to_string(),
            api_key: Arc::new("k".into()),
        }))
    }
```

3. 11 个 `read_recent_*` 测试中所有 `reader_at(&url).read_recent_for_context(...)` → `client_at(&url).read_recent_for_context(...)`（`read_recent_extracts_is_self_and_packs_alternating` 与 `read_recent_keeps_non_text_as_empty_content` 中调用的 `pack_memory_messages` 改 `crate::message::pack_memory_messages`，import 或全路径）
4. 测试内不再有 `MemoryMsg` 引用（原引用点在 msg 辅助函数与断言类型推断处，均已随 Task 2 移除/改名）；`use crate::types::Message;` 保留（extracts_is_self 测试用）

- [ ] **Step 5: 全量测试 + clippy**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -3 && cargo clippy --all-targets 2>&1 | grep -c "warning:"`
Expected: 全绿（118）；clippy ≤ 55；无 unused import / dead code warning（ConfigManager import 随 memory_reader.rs 删除消失）

- [ ] **Step 6: Commit**

```bash
cd /home/admin/project/kissbot
git add -A kissbot-agent/
git commit -m "refactor(agent): 删除 MemoryReader，读取功能并入 MemoryStoreClient（共享 StoreHttpConfig + send_store_query），coordinator 改调 memory_store_client；read 测试迁入 memory_store_client"
```

---

### Task 5: 文档同步

**Files:**
- Modify: `docs/spec/kissbot-agent-modules.md`
- Modify: `docs/spec/kissbot-agent-nexus.md`

**Interfaces:** 无（纯文档）

- [ ] **Step 1: kissbot-agent-modules.md**

1. 表格删除 memory_reader 行（约第 16 行）：

```markdown
| memory_reader | MemoryReader | 从 memory-store 读记忆构建上下文（最近 N + 时间段两次查询并集打包、事件列表、记忆索引） | 被 coordinator 调用；依赖 config + memory-store |
```

2. memory_store_client 行改为承担读写：

```markdown
| memory_store_client | MemoryStoreClient | memory-store 全读写：推记录（channel / think / tool_call / tool_result）+ 读记忆（最近 N + 时间段两次查询并集打包） | 被 coordinator 调用；读经共享 StoreHttpConfig |
```

3. 新增 message 模块行（插在 memory_store_client 行前）：

```markdown
| message | MessageContent / pack_memory_messages / pack_batch | 消息内容组装：extract_content（record/event → 文本段）、记忆交替序列打包、batch → 单条 User 打包 | 被 memory_store_client / session_manager / coordinator 调用 |
```

4. 组件图（约第 45 行）`MR["memory_reader<br/>MemoryReader"]` 替换为：

```markdown
        MSG["message<br/>MessageContent<br/>pack_memory_messages / pack_batch"]
```

5. 时序图（约第 190、206、207 行）：`participant MR as MemoryReader` → `participant MSC as MemoryStoreClient`；`CO->>MR: read_recent_for_context（最近 N + 时间段两次查询并集打包为一条 user 消息）` → `CO->>MSC: ...`（内容不变）；删除 `CO->>MR: read_memory_struct_index（顶层记忆索引，未实现时跳过）` 行

6. 对外交互边界表（约第 216 行）`MemoryStoreClient 推记录、MemoryReader 读记忆` → `MemoryStoreClient 推记录 + 读记忆`

- [ ] **Step 2: kissbot-agent-nexus.md**

1. 第 129 行 `### 读取（MemoryReader）` → `### 读取（MemoryStoreClient）`
2. 该节表格若缺 `/query/channel/recent` 行则补：

```markdown
| POST | /query/channel/recent | RecentQuery — 按 (agent_id, role_name) 取最近 count 条（跨日期文件） |
```

3. 删除"记忆索引读取（MemoryReader → Memory-Struct）"整节（含 /index 表格，约第 136-142 行；功能已删，roadmap 实现时再补文档）

- [ ] **Step 3: 检查仓库内残留引用**

Run: `cd /home/admin/project/kissbot && rg -n "MemoryReader|memory_reader|read_memory_struct_index" --glob '!docs/superpowers/**'`
Expected: 无匹配（spec/plan 历史文档除外——若 spec/plan 引用是历史记录则保留）

- [ ] **Step 4: Commit**

```bash
cd /home/admin/project/kissbot
git add docs/spec/kissbot-agent-modules.md docs/spec/kissbot-agent-nexus.md
git commit -m "docs: memory_reader 删除后同步模块表/组件图/时序图/交互边界（memory_store_client 承担读写，新增 message 模块行；nexus 读取节改 MemoryStoreClient、删记忆索引节）"
```

---

## Self-Review

**1. Spec coverage:**
- §1 StoreHttpConfig 读方法 + MemoryStoreClient 合并 → Task 1（send_store_query/with_config）+ Task 4（read_recent_for_context 迁入）
- §1 read_memory_struct_index 删除 → Task 4 Step 2.4（coordinator 调用块）+ 文件删除
- §2 message.rs（MessageContent/extract_content/collect_text_parts 私有/user_line/pack_memory_messages/pack_batch）→ Task 2
- §3 session_manager → Task 3；coordinator（memory_reader 字段/build_role_context/extract_text 保留）→ Task 4 Step 2；main.rs → Task 2 Step 2 + Task 4 Step 3
- §4 测试 → Task 1 Step 1、Task 2 Step 1、Task 4 Step 4
- §5 文档 → Task 5

**2. Placeholder scan:** 无 TBD/TODO；每步含完整代码。

**3. Type consistency:** `MessageContent`/`extract_content(user_name: &Arc<String>, is_self: u32, content: &Content)`/`pack_memory_messages(&[MessageContent]) -> Vec<Message>`/`pack_batch(&[Arc<IncomingMessageEvent>]) -> Message`/`send_store_query<T: DeserializeOwned>`/`with_config(Arc<StoreHttpConfig>)`/`read_recent_for_context(...) -> Result<Vec<MessageContent>>` 在全部任务中签名一致。

**已知行为差异（符合 spec）：** 批打包跳过空 content 事件（原内联产出空行/“name: ”行）；error 文案由 send_store_query 统一（[status] body），测试不断言错误字符串。
