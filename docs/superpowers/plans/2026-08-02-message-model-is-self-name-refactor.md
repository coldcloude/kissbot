# 消息模型 is_self/name 重构 + memory-store role 目录 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** IncomingMessage 去 is_self 改 msg_id 匹配回显判定；IncomingMessage/OutgoingMessageResponse/GroupChange/UserRemove/ChannelRequest/ChannelRecord 贯通 messenger_name/user_name/group_name（Messenger 填写）；memory-store role 目录改为 `{year}-{role_name}` 移除空值特判。

**Architecture:** OutgoingMessageResponse 携带 name 回传（Messenger 填），is_self=1 record 的 name 取自 response、is_self=0 取自 IncomingMessage。每 channel 一个 ChannelContext 维护「已发未回显 msg_id 集合」（TTL 懒清理），incoming 按 msg_id 命中跳过、未命中存 is_self=0。移除会话级内容兜底 echo 机制。群组变更 is_self=0、按 Content 类型跳过 agentic loop；user_remove 不存 record。

**Tech Stack:** Rust + cargo（kissbot-api / kissbot-memory / kissbot-memory-store / kissbot-channel / kissbot-channel-web / kissbot-channel-client(-cli) / kissbot-agent）；serde；tokio；dashmap。

## Global Constraints

- 所有文本文件 UTF-8 编码、`\n` 换行。
- 不删除代码注释。
- 读写文件用 Read/Write/Edit 工具，禁止 sed/python 改文件。
- Git commit 用中文，包含本次所有改动。
- TDD：每个任务先写失败测试再实现。
- 每个任务结束 `cargo test` 全过且 commit。

## File Structure

- `kissbot-api/src/channel.rs`：IncomingMessage / OutgoingMessageResponse 结构。
- `kissbot-api/src/message.rs`：GroupChangeNotification / UserRemoveNotification 结构。
- `kissbot-api/src/memory.rs`：ChannelRequest / ChannelRecord 结构。
- `kissbot-memory/src/data.rs`：ensure_year_role_dir、ChannelParser。
- `kissbot-memory/src/index.rs`：路径相关测试。
- `kissbot-memory-store/src/record.rs`：ChannelRequest 构造测试。
- `kissbot-channel/src/data.rs`：group_change_to_incoming_message_event。
- `kissbot-channel/src/memory_store_client.rs`：ChannelRequest 构造（未启用，保编译）。
- `kissbot-channel-web/src/messenger.rs`：send / send_stored / notify_group_change / remove_user 填 name。
- `kissbot-channel-client-cli/src/main.rs`、`kissbot-channel-client/tests/mock.rs`：IncomingMessage 构造（测试）。
- `kissbot-agent/src/coordinator.rs`：ChannelContext、msg_id 匹配、handle_incoming。
- `kissbot-agent/src/session_manager.rs`：移除 sent_contents / is_self_echo / record_sent_content。
- `kissbot-agent/src/memory_store_client.rs`：agent 侧 ChannelRecord 加 name。
- `docs/spec/channel-message.md`、`docs/spec/memory-store.md`：文档同步。

---

### Task 1: kissbot-api 消息/记忆结构加 name 字段（is_self 暂留）

**Files:**
- Modify: `kissbot-api/src/channel.rs`、`kissbot-api/src/message.rs`、`kissbot-api/src/memory.rs`
- Modify（构造点）：`kissbot-channel-web/src/messenger.rs`、`kissbot-channel/src/data.rs`、`kissbot-channel/src/memory_store_client.rs`、`kissbot-channel-client/tests/mock.rs`、`kissbot-memory/src/data.rs`、`kissbot-memory-store/src/record.rs`、`kissbot-agent/src/memory_store_client.rs`、`kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Produces：IncomingMessage/OutgoingMessageResponse/GroupChangeNotification/UserRemoveNotification/ChannelRequest/ChannelRecord 新增 `messenger_name/user_name/group_name`（UserRemove 无 group_name）；构造点暂填 `Arc::new(String::new())`；is_self 暂留。

- [ ] **Step 1: 加字段并更新 serde 测试（先改测试使其失败）**

`kissbot-api/src/channel.rs` OutgoingMessageResponse 末尾加三字段；IncomingMessage 在 is_self 后加三字段：
```rust
pub struct OutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub content: Content,  // 转换后的 content（已嵌入 key）
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
    pub group_name: Arc<String>,
}
```
```rust
pub struct IncomingMessage {
    pub msg_id: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
    pub group_name: Arc<String>,
    pub content: Content,
    pub time: Arc<String>,
}
```

`kissbot-api/src/message.rs`：
```rust
pub struct GroupChangeNotification {
    pub messenger_id: Arc<String>,
    pub group_id: Arc<String>,
    pub user_id: Arc<String>,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
    pub group_name: Arc<String>,
}

pub struct UserRemoveNotification {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
}
```

`kissbot-api/src/memory.rs` ChannelRequest 在 is_self 后、ChannelRecord 在 is_self 后加三字段（messenger_name/user_name/group_name）。

更新 `kissbot-api/src/channel.rs`、`message.rs`、`memory.rs` 内 serde 测试构造体（加 `messenger_name: Arc::new("...".to_string())` 等占位值）。

- [ ] **Step 2: 运行测试确认编译失败**

Run: `cd kissbot-api && cargo test`
Expected: 编译错误（各 crate 构造点缺新字段）。

- [ ] **Step 3: 更新全部构造点（暂填空串）**

对每个构造 IncomingMessage/OutgoingMessageResponse/GroupChangeNotification/UserRemoveNotification/ChannelRequest/ChannelRecord 的位置，新增 `messenger_name: Arc::new(String::new())` / `user_name: Arc::new(String::new())` / `group_name: Arc::new(String::new())`（UserRemove 无 group_name）。已知构造点：
- `kissbot-channel-web/src/messenger.rs`：send() 的 admin_msg、send() 返回的 OutgoingMessageResponse、send_stored() 分发 IncomingMessage、notify_group_change() 的 GroupChangeNotification、remove_user() 的 UserRemoveNotification。
- `kissbot-channel/src/data.rs`：group_change_to_incoming_message_event 的 IncomingMessage。
- `kissbot-channel/src/memory_store_client.rs`：write() 内 ChannelRequest 构造（从 IncomingMessage 透传，暂空）。
- `kissbot-channel-client/tests/mock.rs`：IncomingMessage 构造。
- `kissbot-memory/src/data.rs`：ChannelParser::parse_request 的 ChannelRecord（从 ChannelRequest 透传 name）。
- `kissbot-memory-store/src/record.rs`：测试内 ChannelRequest 构造。
- `kissbot-agent/src/memory_store_client.rs`：ChannelRecord 结构体加字段 + write() 转 ChannelRequest 透传。
- `kissbot-agent/src/coordinator.rs`：incoming_message 与 send_reply 的 ChannelRecord 构造加字段（暂空）。

ChannelParser（kissbot-memory/src/data.rs）parse_request 内 ChannelRecord 构造需拷贝 request 的 name：
```rust
let record = ChannelRecord {
    user_id: user_id,
    is_self: request.is_self,
    messenger_name: request.messenger_name.clone(),
    user_name: request.user_name.clone(),
    group_name: request.group_name.clone(),
    content: request.content.clone(),
    time: request.time.clone(),
    sn: 0,
};
```

agent 侧 memory_store_client.rs 的 ChannelRecord 结构体加三字段，write() 转 ChannelRequest 时透传：
```rust
let requests: Vec<ChannelRequest> = records.into_iter().map(|r| ChannelRequest {
    agent_id: r.agent_id, role_name: r.role_name,
    messenger_id: r.messenger_id, user_id: r.user_id, group_id: r.group_id,
    is_self: r.is_self,
    messenger_name: r.messenger_name, user_name: r.user_name, group_name: r.group_name,
    content: r.content, time: r.time,
}).collect();
```

- [ ] **Step 4: 运行全工作区测试确认通过**

Run: `cargo test --workspace`
Expected: PASS（name 字段为空串，行为不变）。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(api): 消息/记忆结构加 messenger_name/user_name/group_name 字段（is_self 暂留，构造点暂填空串）"
```

---

### Task 2: channel-web 填 name

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs`

**Interfaces:**
- Consumes: Task 1 的 name 字段。
- Produces: OutgoingMessageResponse/IncomingMessage/GroupChangeNotification/UserRemoveNotification 由 WebMessenger 填入真实 name。

- [ ] **Step 1: 写失败测试 — send 返回的 response 含 name**

`kissbot-channel-web/src/messenger.rs` 测试模块（或新建 tests/send_name_test.rs）加测试：构造 WebMessenger，调用 send(OutgoingMessage{messenger_id,user_id=ADMIN_USER_ID,group_id=admin-user group})，断言 response.messenger_name=="Web Chat"、response.user_name==admin_name、response.group_name==admin_name。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-channel-web && cargo test send_response_name`
Expected: FAIL（name 为空串）。

- [ ] **Step 3: 实现 name 解析与填充**

在 WebMessenger 加常量与辅助方法：
```rust
const WEB_MESSENGER_NAME: &str = "Web Chat";

impl WebMessenger {
    /// 按 user_id/group_id 解析 (messenger_name, user_name, group_name)
    fn resolve_names(&self, cfg: &WebMessengerRepo, user_id: &str, group_id: &str) -> (Arc<String>, Arc<String>, Arc<String>) {
        let messenger_name = Arc::new(WEB_MESSENGER_NAME.to_string());
        let user_name = cfg.users.get(user_id)
            .map(|s| s.load().user_name.clone())
            .unwrap_or_else(|| Arc::new(String::new()));
        let group_name = if group_id.starts_with(ADMIN_USER_GROUP_PREFIX) {
            // admin-user 单聊组：group_name = 该 user 的 user_name
            cfg.users.get(user_id)
                .map(|s| s.load().user_name.clone())
                .unwrap_or_else(|| Arc::new(String::new()))
        } else {
            cfg.groups.get(group_id)
                .map(|s| s.load().group_name.clone())
                .unwrap_or_else(|| Arc::new(String::new()))
        };
        (messenger_name, user_name, group_name)
    }
}
```

send() 内（cfg 仍持有读锁时）解析 name 并填入 admin_msg 与 OutgoingMessageResponse：
```rust
// 在 drop(cfg) 之前解析 name（cfg 已读）
let (messenger_name, user_name, group_name) = self.resolve_names(&cfg, outgoing.user_id.as_str(), outgoing.group_id.as_str());
drop(cfg);
// ... new_content / msg_id / time ...
let admin_msg = IncomingMessage {
    msg_id: msg_id.clone(),
    messenger_id: self.messenger_id.clone(),
    user_id: outgoing.user_id.clone(),
    group_id: outgoing.group_id.clone(),
    is_self: is_admin,
    messenger_name: messenger_name.clone(),
    user_name: user_name.clone(),
    group_name: group_name.clone(),
    content: new_content.clone(),
    time: time.clone(),
};
// ... appender ...
Ok(Arc::new(OutgoingMessageResponse {
    msg_id,
    time,
    content: new_content,
    messenger_name,
    user_name,
    group_name,
}))
```

send_stored() 内分发 IncomingMessage 时从 admin_msg 透传 name（替换原 `is_self` 计算，name 三字段直接 clone admin_msg 的）：
```rust
let incoming = Arc::new(IncomingMessage {
    msg_id: admin_msg.msg_id.clone(),
    messenger_id: admin_msg.messenger_id.clone(),
    user_id: admin_msg.user_id.clone(),
    group_id: admin_msg.group_id.clone(),
    is_self,
    messenger_name: admin_msg.messenger_name.clone(),
    user_name: admin_msg.user_name.clone(),
    group_name: admin_msg.group_name.clone(),
    content: admin_msg.content.clone(),
    time: admin_msg.time.clone(),
});
```

notify_group_change() 与 remove_user() 内构造通知时填 name：notify_group_change 需读 cfg 解析（user_name=被通知 user、group_name=group），调用 resolve_names 后填入 GroupChangeNotification；remove_user 解析 user_name 填入 UserRemoveNotification（messenger_name=WEB_MESSENGER_NAME）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-channel-web && cargo test`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(channel-web): Messenger 填写 OutgoingMessageResponse/IncomingMessage/GroupChange/UserRemove 的 name 字段"
```

---

### Task 3: channel data.rs 透传 name + memory_store_client is_self=0

**Files:**
- Modify: `kissbot-channel/src/data.rs`、`kissbot-channel/src/memory_store_client.rs`

**Interfaces:**
- Consumes: Task 1 的 GroupChangeNotification name 字段。
- Produces: group_change_to_incoming_message_event 透传 name；channel memory_store_client 推 ChannelRequest is_self=0、name 取自 IncomingMessage。

- [ ] **Step 1: 写失败测试 — group_change_to_incoming 透传 name**

`kissbot-channel/src/data.rs` 测试模块加测试：构造 GroupChangeEvent（GroupChangeNotification 含 name），调用 group_change_to_incoming_message_event，断言结果 IncomingMessage 的 messenger_name/user_name/group_name 与通知一致。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-channel && cargo test group_change_name`
Expected: FAIL（name 为空串）。

- [ ] **Step 3: 实现透传**

`kissbot-channel/src/data.rs` group_change_to_incoming_message_event：
```rust
let incoming = Arc::new(IncomingMessage {
    msg_id: message.msg_id.clone(),
    messenger_id: message.notification.messenger_id.clone(),
    user_id: message.notification.user_id.clone(),
    group_id: message.notification.group_id.clone(),
    is_self: 1,  // 暂留，Task 5 移除
    messenger_name: message.notification.messenger_name.clone(),
    user_name: message.notification.user_name.clone(),
    group_name: message.notification.group_name.clone(),
    content,
    time: message.time.clone(),
});
```

`kissbot-channel/src/memory_store_client.rs` write() 内 ChannelRequest 构造：`is_self: 0`（不再用 record.message.is_self），name 透传 `record.message.messenger_name.clone()` 等。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-channel && cargo test`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(channel): group_change 透传 name、memory_store_client 推 is_self=0 并透传 name"
```

---

### Task 4: agent ChannelContext + msg_id 匹配

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`、`kissbot-agent/src/session_manager.rs`

**Interfaces:**
- Consumes: Task 1/2 的 IncomingMessage/OutgoingMessageResponse name、msg_id。
- Produces: 每 channel ChannelContext（pending msg_id 集合 + TTL）；incoming 按 msg_id 命中跳过、未命中存 is_self=0；send_reply 先入 pending 再存 is_self=1（name 取自 response）；handle_incoming 按 Content 类型跳过 agentic loop；移除 session_manager 内容兜底。

- [ ] **Step 1: 写失败测试 — ChannelContext msg_id 匹配与 TTL**

`kissbot-agent/src/coordinator.rs` 测试模块加 ChannelContext 单元测试：add_pending(msg_id) 后 is_pending(msg_id) 返回 true 且移除条目；未加入的 msg_id 返回 false；TTL 过期后条目被淘汰（用 Instant 减去超过 TTL 的时刻模拟，或直接构造过期条目验证 evict）。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-agent && cargo test channel_context`
Expected: FAIL（ChannelContext 未定义）。

- [ ] **Step 3: 实现 ChannelContext 与 coordinator 集成**

`kissbot-agent/src/coordinator.rs` 顶部加：
```rust
use std::collections::HashMap;
use std::time::{Duration, Instant};

const CHANNEL_CONTEXT_TTL_SECS: u64 = 60;

struct ChannelContext {
    pending_outgoing: HashMap<Arc<String>, Instant>,
}

impl ChannelContext {
    fn new() -> Self { Self { pending_outgoing: HashMap::new() } }
    fn add_pending(&mut self, msg_id: Arc<String>) {
        self.evict();
        self.pending_outgoing.insert(msg_id, Instant::now());
    }
    /// 命中则移除并返回 true（回显消费）
    fn consume_pending(&mut self, msg_id: &str) -> bool {
        self.evict();
        self.pending_outgoing.remove(msg_id).is_some()
    }
    fn evict(&mut self) {
        let ttl = Duration::from_secs(CHANNEL_CONTEXT_TTL_SECS);
        self.pending_outgoing.retain(|_, t| t.elapsed() < ttl);
    }
}
```

AgentCoordinator 加字段：
```rust
channel_contexts: Arc<DashMap<String, Arc<tokio::sync::Mutex<ChannelContext>>>>,
```
new() 内初始化 `channel_contexts: Arc::new(DashMap::new())`。

加辅助方法：
```rust
async fn record_outgoing_msg_id(&self, channel_id: &str, msg_id: &Arc<String>) {
    let ctx = self.channel_contexts
        .entry(channel_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(ChannelContext::new())))
        .clone();
    ctx.lock().await.add_pending(msg_id.clone());
}

async fn is_self_echo_by_msg_id(&self, channel_id: &str, msg_id: &Arc<String>) -> bool {
    if let Some(ctx) = self.channel_contexts.get(channel_id) {
        ctx.lock().await.consume_pending(msg_id.as_str())
    } else {
        false
    }
}
```

改 Terminal::incoming_message：先判 msg_id 回显，未命中存 is_self=0（name 取自 IncomingMessage）：
```rust
async fn incoming_message(&self, channel_id: &str, message: Arc<IncomingMessage>) {
    let Some(ch) = self.channel_config(channel_id).await else { return; };
    // msg_id 回显判定
    if self.is_self_echo_by_msg_id(channel_id, &message.msg_id).await {
        return;
    }
    if let Some(key) = self.session_key_for(&ch) {
        let role_name = memory_role(&key);
        self.memory_store_client.push_channel_record(ChannelRecord {
            agent_id: Arc::new(key.agent_id.clone()),
            role_name: Arc::new(role_name),
            messenger_id: message.messenger_id.clone(),
            user_id: message.user_id.clone(),
            group_id: message.group_id.clone(),
            messenger_name: message.messenger_name.clone(),
            user_name: message.user_name.clone(),
            group_name: message.group_name.clone(),
            is_self: 0,
            content: message.content.clone(),
            time: message.time.clone(),
        }).await;
    }
    self.handle_incoming(channel_id, ch, message).await;
}
```

改 send_reply：response 拿到后**先**入 pending 再 push record（name 取自 response）：
```rust
match client.send_message(msg).await {
    Ok(response) => {
        if let Some(key) = self.session_key_for(&ch) {
            let role_name = memory_role(&key);
            self.record_outgoing_msg_id(send_channel_id, &response.msg_id).await;
            self.memory_store_client.push_channel_record(ChannelRecord {
                agent_id: Arc::new(key.agent_id.clone()),
                role_name: Arc::new(role_name),
                messenger_id: bound.messenger_id.clone(),
                user_id: bound.user_id.clone(),
                group_id: Arc::new(group_id.to_string()),
                messenger_name: response.messenger_name.clone(),
                user_name: response.user_name.clone(),
                group_name: response.group_name.clone(),
                is_self: 1,
                content: response.content.clone(),
                time: response.time.clone(),
            }).await;
        }
    }
    Err(e) => { warn!("send_reply 失败: {:?}", e); }
}
```
删除原 send_reply 内 `ctx.record_sent_content(content)` 调用。

改 handle_incoming：删除开头 `is_self == 1` 回显判定块，改为按 Content 类型跳过：
```rust
async fn handle_incoming(&self, channel_id: &str, ch: Arc<ChannelConfig>, incoming: Arc<IncomingMessage>) {
    let messenger_id = incoming.messenger_id.to_string();
    let user_id = incoming.user_id.to_string();
    let group_id = incoming.group_id.to_string();
    let content_text = extract_text(&incoming.content);

    // 系统事件（群组变更/用户移除）不进 agentic loop
    match &incoming.content {
        Content::GroupJoin(_) | Content::GroupLeave(_) | Content::UserRemove(_) => return,
        _ => {}
    }

    // 管理命令
    if CommandRouter::is_command(&content_text) { ... }
    // 普通消息 ...
}
```

`kissbot-agent/src/session_manager.rs`：移除 `sent_contents: VecDeque<String>` 字段、`record_sent_content` 方法、`is_self_echo` 方法（及 SessionContext::new 中相关初始化）。搜索全 crate 确认无残留调用（coordinator 已删 record_sent_content/is_self_echo 调用）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-agent && cargo test`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(agent): is_self 改 msg_id 匹配（ChannelContext + TTL），移除内容兜底 echo，handle_incoming 按 Content 跳过系统事件"
```

---

### Task 5: IncomingMessage 去 is_self

**Files:**
- Modify: `kissbot-api/src/channel.rs`、`kissbot-channel-web/src/messenger.rs`、`kissbot-channel/src/data.rs`、`kissbot-channel-client/tests/mock.rs`、`kissbot-channel-client-cli/src/main.rs`（若有构造）

**Interfaces:**
- Produces: IncomingMessage 无 is_self 字段；所有构造点与读取点移除 is_self。

- [ ] **Step 1: 改测试断言 is_self 不存在**

`kissbot-api/src/channel.rs` serde 测试 test_serde_incoming_message 移除 `is_self: 0` 构造与断言。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-api && cargo test`
Expected: 编译错误（各处仍用 is_self）。

- [ ] **Step 3: 移除字段与全部引用**

`kissbot-api/src/channel.rs` IncomingMessage 删除 `pub is_self: usize,`。

构造点移除 `is_self: ...`：
- `kissbot-channel-web/src/messenger.rs`：admin_msg（删 `is_self: is_admin`）、send_stored 分发消息（删 `is_self`，并删 `let is_self = ...` 与 `let is_admin = ...` 计算行）。
- `kissbot-channel/src/data.rs`：group_change_to_incoming_message_event（删 `is_self: 1`）。
- `kissbot-channel-client/tests/mock.rs`：IncomingMessage 构造删 is_self。
- `kissbot-channel-client-cli/src/main.rs`：若 incoming_message 回调读取 is_self 则删（当前仅打印 content，应无读取）。

读取点：`kissbot-channel/src/memory_store_client.rs`（Task 3 已改 is_self=0，不再读 message.is_self）、`kissbot-agent/src/coordinator.rs`（Task 4 已不读 incoming.is_self）。确认无残留 `incoming.is_self` / `message.is_self`。

- [ ] **Step 4: 运行全工作区测试确认通过**

Run: `cargo test --workspace`
Expected: PASS。

- [ ] **Step 5: 更新文档**

`docs/spec/channel-message.md`：「消息方向」节移除 IncomingMessage 含 is_self 的描述。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(api): IncomingMessage 移除 is_self 字段，全调用方同步"
```

---

### Task 6: memory-store role 目录格式

**Files:**
- Modify: `kissbot-memory/src/data.rs`、`kissbot-memory/src/index.rs`、`kissbot-memory-store/src/record.rs`（测试路径断言）

**Interfaces:**
- Produces: ensure_year_role_dir 始终 `format!("{}-{}", year, role_name)`，移除空值特判；空 role -> `2026-`，事件模式 -> `2026--event1`。

- [ ] **Step 1: 写失败测试 — 空 role 目录为 `2026-`**

`kissbot-memory/src/data.rs` 测试模块加测试：key.role_name="" 时 get_path 文件名含目录 `2026-`（非 `2026`）。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-memory && cargo test empty_role_dir`
Expected: FAIL（当前空 role 返回 `2026`）。

- [ ] **Step 3: 实现格式变更**

`kissbot-memory/src/data.rs` ensure_year_role_dir：
```rust
let year_role_dir = store_dir.join(format!("{}-{}", year, role_name));
```
（删除 `if role_name.is_empty() { store_dir.join(year) } else { ... }` 分支。）

- [ ] **Step 4: 更新现有路径断言测试**

搜索 `kissbot-memory/src/index.rs`、`kissbot-memory-store/src/record.rs`、`kissbot-memory/src/data.rs` 中 assert 路径含空 role 的测试（如 `channel-...records-....jsonl` 所在目录为 `2026` 的），改为 `2026-`。非空 role（如 `default`）的 `2026-default` 断言不变。

- [ ] **Step 5: 运行全工作区测试确认通过**

Run: `cargo test --workspace`
Expected: PASS。

- [ ] **Step 6: 更新文档**

`docs/spec/memory-store.md`、`docs/spec/memory-directory.md`（若有）更新 role 目录格式说明（`{year}-{role_name}`，空值形如 `2026-`）。

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(memory): role 目录格式统一为 {year}-{role_name}，移除空值特判"
```

---

## Self-Review

**1. Spec coverage（计划 1 覆盖 spec 第 1/2/5/6 节中属于本计划的部分）：**
- 第 1 节 is_self（IncomingMessage 去 is_self、OutgoingMessageResponse 加 name、ChannelContext msg_id 匹配、移除内容兜底、群组变更 is_self=0、user_remove 不存 record）：Task 1/2/3/4/5 覆盖。✓
- 第 2 节 name 贯通（IncomingMessage/GroupChange/UserRemove 加 name 由 Messenger 填、OutgoingMessage 不变、ChannelRecord name 来源）：Task 1/2/3 覆盖。✓
- 第 5 节 memory-store role 目录：Task 6 覆盖。✓
- 第 6 节受影响面中本计划部分（api/memory/memory-store/channel/channel-web/channel-client-cli/agent + 文档）：各 Task 覆盖。✓
- 注：spec 第 1 节「user_remove 不存 record」现状即不存，Task 4 不涉及其存储路径，仅 UserRemoveNotification 加 name（Task 1/2）。✓

**2. Placeholder scan：** 无 TBD/TODO；构造点更新以「pattern + 已知站点列表」给出，非占位。✓

**3. Type consistency：** name 字段命名统一 `messenger_name/user_name/group_name`；ChannelContext 方法 `add_pending/consume_pending/record_outgoing_msg_id/is_self_echo_by_msg_id` 前后一致。✓

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-02-message-model-is-self-name-refactor.md`. 计划 2（memory-ego + agent_name 绑定）待本计划完成后再写。
