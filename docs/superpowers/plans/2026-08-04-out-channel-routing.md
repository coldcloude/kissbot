# out_channel 路由模型重构 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** channel 多绑定（`bind_users`）+ `out_channel=(channel_id, ChannelUser, group_id)` 路由替代 `is_send_channel`，统一 Agentic Loop 产出身份，无 out_channel 时只存不回复。

**Architecture:** `ChannelConfig` 改 `bind_users: Vec<ChannelUser>` + `outgoing: Option<OutChannelConfig>`，删 `is_send_channel`/`bind_user`；session 运行时实时 `resolve_out_channel`（跨同 agent/role channel 找有 outgoing 的，至多 1 个）；`IncomingMessageEvent` 移到 kissbot-api 并透传 `recipient_user_id`（ws -> Terminal 全链路）；回复拆 `send_outgoing`（走 out_channel）/`send_admin_reply`（发回来源 channel）；命令 `/bind`（追加去重）、`/unbind`（带 user_id 移除并清空引用的 outgoing）、`/bind-outgoing`（设/清空 out_channel，校验已绑 + 同 agent/role 唯一）。

**Tech Stack:** Rust（tokio / serde / dashmap / arc-swap），Playwright 集成测试。

## Global Constraints

- **不删除代码注释**（项目约定）
- **不兼容旧配置**：`bind_user`/`default_bind_user`/`is_send_channel` 不做 serde 兼容，配置文件直接改（`script/template/nexus.json`、`test/workspace-template/agent-data/nexus.json`）
- **`is_send_channel` 删除**：字段、`/send-channel` 命令、`SessionManager::resolve_send_channel`、coordinator `resolve_send_channel`/`set_send_channel`、`reply` 全部移除
- **IncomingMessage/OutgoingMessage 不加字段**（`kissbot-api/src/channel.rs` 保持原样；`IncomingMessageEvent` 移到 kissbot-api 并加 serde）
- out_channel 跟 **channel 不跟 mode**：存 channel 配置，该 channel 所有 mode session 共用
- out_channel 至多 1 个：`/bind-outgoing` 设时清空同 `(agent_name, role_name)` 其他 channel 的 outgoing
- 无 out_channel：系统命令正常处理（回复发回来源 channel）+ incoming 存 ChannelRecord（is_self=0）+ 普通消息不进 Agentic Loop
- 文本 UTF-8/LF；commit 中文且覆盖全部改动

---

### Task 1: IncomingMessageEvent 透传（ws -> Terminal 携带 recipient_user_id）

**Files:**
- Modify: `kissbot-api/src/channel.rs`（新增 `IncomingMessageEvent` + serde derive + 测试）
- Modify: `kissbot-channel/src/data.rs`（删除本地 `IncomingMessageEvent` 定义，用 kissbot-api 的）
- Modify: `kissbot-channel/src/channel_manager.rs:677-706`（`process_incoming_message` 发整个 event）
- Modify: `kissbot-channel-client/src/terminal.rs:14`（`Terminal::incoming_message` 签名改 `Arc<IncomingMessageEvent>`）
- Modify: `kissbot-channel-client/src/channel_client.rs:175-177`（解析 `IncomingMessageEvent`）
- Modify: `kissbot-agent/src/coordinator.rs`（`incoming_message` 签名改 event + 内部解包 `event.incoming_message` 保持行为）

**Interfaces:**
- Consumes: `kissbot_api::channel::IncomingMessage`（现有）、`kissbot_channel::IncomingMessageEvent`（当前）
- Produces: `kissbot_api::IncomingMessageEvent { recipient_user_id: Arc<String>, incoming_message: Arc<IncomingMessage> }`（含 serde）；`Terminal::incoming_message(&self, id: &str, message: Arc<IncomingMessageEvent>)`

- [ ] **Step 1: 写失败测试（IncomingMessageEvent serde roundtrip）**

在 `kissbot-api/src/channel.rs` 测试模块追加：
```rust
#[test]
fn test_serde_incoming_message_event() {
    let incoming = Arc::new(IncomingMessage {
        msg_id: Arc::new("msg1".to_string()),
        messenger_id: Arc::new("web".to_string()),
        user_id: Arc::new("u2".to_string()),
        group_id: Arc::new("g1".to_string()),
        messenger_name: Arc::new("Web".to_string()),
        user_name: Arc::new("User2".to_string()),
        group_name: Arc::new("Group1".to_string()),
        content: Content::Text(Arc::new("hello".to_string())),
        time: Arc::new("2026-01-01 00:00:00".to_string()),
    });
    let obj = IncomingMessageEvent {
        recipient_user_id: Arc::new("u1".to_string()),
        incoming_message: incoming,
    };
    let json = serde_json::to_value(&obj).unwrap();
    let deserialized: IncomingMessageEvent = serde_json::from_value(json).unwrap();
    assert_eq!(*deserialized.recipient_user_id, "u1");
    assert_eq!(*deserialized.incoming_message.user_id, "u2");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-api && cargo test`
Expected: 编译错误 `IncomingMessageEvent` 未定义。

- [ ] **Step 3: 在 kissbot-api 定义 IncomingMessageEvent**

`kissbot-api/src/channel.rs` 中 `IncomingMessage` 定义之后（impl Record 之前）追加：
```rust
/// 通道内统一的消息分发事件。recipient_user_id 为接收者（用于 bound_map）。
/// incoming_message.user_id 为**发送者**。两者不同时表示转发（如 admin → agent）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessageEvent {
    pub recipient_user_id: Arc<String>,
    pub incoming_message: Arc<IncomingMessage>,
}
```

- [ ] **Step 4: kissbot-channel/data.rs 删本地定义**

删除 `kissbot-channel/src/data.rs` 中 `IncomingMessageEvent` 结构定义（保留 `group_change_to_incoming_message_event` 等其余内容；文件头部已有 `use kissbot_api::channel::*;` 自动引入 kissbot-api 版本）。

- [ ] **Step 5: channel_manager 发整个 event**

`kissbot-channel/src/channel_manager.rs` `process_incoming_message` 中：
```rust
let payload = serde_json::to_value(event.incoming_message.as_ref())?;
```
改为：
```rust
// 发整个 event（含 recipient_user_id），agent 侧直接获得接收方身份
let payload = serde_json::to_value(event.as_ref())?;
```

- [ ] **Step 6: Terminal 签名改**

`kissbot-channel-client/src/terminal.rs`：
```rust
async fn incoming_message(&self, id: &str, message: Arc<IncomingMessageEvent>);
```

- [ ] **Step 7: channel_client 解析 IncomingMessageEvent**

`kissbot-channel-client/src/channel_client.rs` `process_json`：
```rust
TYPE_INCOMING_MESSAGE => match serde_json::from_value::<IncomingMessage>(payload) {
    Ok(m) => terminal.incoming_message(client.id(), Arc::new(m)).await,
    Err(e) => error!("parse incoming message error: {:?}", e),
},
```
改为：
```rust
TYPE_INCOMING_MESSAGE => match serde_json::from_value::<IncomingMessageEvent>(payload) {
    Ok(ev) => terminal.incoming_message(client.id(), ev).await,
    Err(e) => error!("parse incoming message event error: {:?}", e),
},
```
（文件头部 `use kissbot_api::channel::*;` 已覆盖 IncomingMessageEvent。）

- [ ] **Step 8: coordinator incoming_message 签名改并解包**

`kissbot-agent/src/coordinator.rs` `impl Terminal for AgentCoordinator` 中 `incoming_message` 签名与内部改用 event（此步仅解包 `event.incoming_message` 保持现有行为，`self_user_id` 暂用 `ch.bind_user.user_id`，Task 2 改为 `recipient_user_id`）：
```rust
async fn incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>) {
    // 1. 来源 channel 必须在配置中
    let Some(ch) = self.channel_config(channel_id).await else { return; };

    // 2. msg_id 回显判定：命中（已发未回显）则跳过，不存 record、不进 agentic loop
    if self.is_self_echo_by_msg_id(channel_id, &event.incoming_message.msg_id).await {
        return;
    }

    // 3. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 取来源 channel 运行态绑定，事件模式编码）
    let key = self.session_key_for(&ch);
    let role_name = memory_role(&key);
    let agent_id = self.channel_agent(channel_id).await;
    self.memory_store_client.push_channel_record(ChannelRecord {
        agent_id,
        role_name: Arc::new(role_name),
        messenger_id: event.incoming_message.messenger_id.clone(),
        user_id: event.incoming_message.user_id.clone(),
        // 接收方身份 = channel 绑定的 user_id（agent 视角的 self；文件名按此分文件）
        self_user_id: Arc::new(ch.bind_user.user_id.clone()),
        group_id: event.incoming_message.group_id.clone(),
        is_self: 0,
        messenger_name: event.incoming_message.messenger_name.clone(),
        user_name: event.incoming_message.user_name.clone(),
        group_name: event.incoming_message.group_name.clone(),
        content: event.incoming_message.content.clone(),
        time: event.incoming_message.time.clone(),
    }).await;

    // 4. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
    self.handle_incoming(channel_id, ch, event).await;
}
```

同步修改 `handle_incoming` 与 `run_agentic_loop` 签名：`incoming: Arc<IncomingMessage>` 改 `event: Arc<IncomingMessageEvent>`，内部所有 `incoming.xxx` 改 `event.incoming_message.xxx`（`extract_text(&event.incoming_message.content)`、`messenger_id/user_id/group_id/time` 同理）。`handle_admin_command`/`reply` 本步不动（Task 2 重构）。

- [ ] **Step 9: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test`
Expected: 全部 PASS（kissbot-api / kissbot-channel / kissbot-channel-client / kissbot-agent 各 crate 编译 + 测试）。

- [ ] **Step 10: 提交**

```bash
cd /home/admin/project/kissbot
git add kissbot-api/src/channel.rs kissbot-channel/src/data.rs kissbot-channel/src/channel_manager.rs kissbot-channel-client/src/terminal.rs kissbot-channel-client/src/channel_client.rs kissbot-agent/src/coordinator.rs
git commit -m "refactor(channel): IncomingMessageEvent 移到 kissbot-api 并加 serde，ws 到 Terminal 全链路透传 recipient_user_id——channel_manager 发整个 event；Terminal::incoming_message 签名改 Arc<IncomingMessageEvent>；coordinator 解包 event.incoming_message 保持行为"
```

---

### Task 2: 数据模型 + coordinator 消息处理重构

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`（`ChannelConfig` 改 `bind_users`/`outgoing`/删 `bind_user`/`is_send_channel`；新增 `OutChannelConfig`/`OutChannel`；测试改）
- Modify: `script/template/nexus.json`、`test/workspace-template/agent-data/nexus.json`（`bind_user` -> `bind_users`、删 `is_send_channel`、加 `outgoing`）
- Modify: `kissbot-agent/src/session_manager.rs`（删 `resolve_send_channel` 方法；`sample_channel` 测试改；删 `resolve_send_channel_flag_then_first` 测试）
- Modify: `kissbot-agent/src/coordinator.rs`（`resolve_out_channel`/`send_outgoing`/`send_admin_reply` 新增；`handle_incoming` 分流；`run_agentic_loop` 走 out_channel；删 `reply`/`send_reply`/`resolve_send_channel`/`set_send_channel`；`incoming_message` self_user_id 用 `recipient_user_id`；`load_ego_info` 遍历 `bind_users`）

**Interfaces:**
- Consumes: `kissbot_api::IncomingMessageEvent`（Task 1）；`kissbot_api::ChannelUser`；`kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse}`
- Produces: `ChannelConfig { channel_id, ws_url, admins, bind_users: Vec<ChannelUser>, outgoing: Option<OutChannelConfig>, agent_name, role_name, enabled }`；`OutChannelConfig { messenger_id: Arc<String>, user_id: Arc<String>, group_id: Arc<String> }`；`OutChannel { channel_id: Arc<String>, user: ChannelUser, group_id: Arc<String> }`；`resolve_out_channel(&self, channel_id: &str) -> Option<OutChannel>`；`send_outgoing(&self, out_channel: &OutChannel, content: String)`；`send_admin_reply(&self, channel_id: &str, event: &Arc<IncomingMessageEvent>, content: String)`

- [ ] **Step 1: 写失败测试（config_manager bind_users/outgoing 序列化 + update_channel 追加/移除）**

`kissbot-agent/src/config_manager.rs` 测试模块追加：
```rust
#[test]
fn channel_config_bind_users_and_outgoing_roundtrip() {
    let ch = ChannelConfig {
        channel_id: Arc::new("c1".into()),
        ws_url: Arc::new("ws://127.0.0.1:8201".into()),
        admins: Arc::new(HashSet::new()),
        bind_users: vec![
            ChannelUser { messenger_id: "web".into(), user_id: "u1".into() },
            ChannelUser { messenger_id: "web".into(), user_id: "u2".into() },
        ],
        outgoing: Some(OutChannelConfig {
            messenger_id: Arc::new("web".into()),
            user_id: Arc::new("u1".into()),
            group_id: Arc::new("g1".into()),
        }),
        agent_name: Arc::new("a1".into()),
        role_name: Arc::new("r1".into()),
        enabled: true,
    };
    let json = serde_json::to_value(&ch).unwrap();
    assert_eq!(json["bind_users"][0]["user_id"], "u1", "bind_users 数组序列化");
    assert_eq!(json["outgoing"]["group_id"], "g1");
    assert!(json.get("is_send_channel").is_none(), "is_send_channel 已删除");
    let back: ChannelConfig = serde_json::from_value(json).unwrap();
    assert_eq!(back.bind_users.len(), 2);
    assert_eq!(back.outgoing.as_ref().unwrap().group_id.as_str(), "g1");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test channel_config_bind_users`
Expected: 编译错误（`bind_users`/`outgoing` 字段不存在、`OutChannelConfig` 未定义）。

- [ ] **Step 3: config_manager.rs 数据结构改**

`kissbot-agent/src/config_manager.rs` `ChannelConfig` 替换为：
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_id: Arc<String>,         // agent 内部唯一标识，与消息方 messenger 无关
    pub ws_url: Arc<String>,
    pub admins: Arc<HashSet<ChannelUser>>,
    /// 多绑定身份（bind 追加去重，unbind 带 ChannelUser 移除）
    pub bind_users: Vec<ChannelUser>,
    /// out_channel 配置（Option，至多 1 个；存于被绑定的 channel 下）
    pub outgoing: Option<OutChannelConfig>,
    /// 绑定的 agent_name（代号；空 = 保留 agent，建会话用默认系统提示词，不调 memory-ego）
    #[serde(default)]
    pub agent_name: Arc<String>,
    #[serde(default)]
    pub role_name: Arc<String>,
    /// 是否启用（连接由 enabled 控制）
    pub enabled: bool,
}

/// out_channel 配置（持久化到 nexus.json；与 /bind-outgoing 三参数对应）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutChannelConfig {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
}

/// out_channel 运行态（coordinator 实时从配置构造）
#[derive(Debug, Clone)]
pub struct OutChannel {
    pub channel_id: Arc<String>,
    pub user: ChannelUser,
    pub group_id: Arc<String>,
}
```

- [ ] **Step 4: 配置文件改**

`script/template/nexus.json` channels 段：
```json
"web1": {
  "channel_id": "web1",
  "ws_url": "ws://127.0.0.1:8201",
  "admins": [{ "messenger_id": "web", "user_id": "admin" }],
  "bind_users": [{ "messenger_id": "web", "user_id": "u1" }],
  "agent_name": "",
  "role_name": "",
  "outgoing": { "messenger_id": "web", "user_id": "u1", "group_id": "g1" },
  "enabled": true
}
```

`test/workspace-template/agent-data/nexus.json` channels 段（对照现有 `web-main` 条目改）：
```json
"web-main": {
  "channel_id": "web-main",
  "ws_url": "ws://127.0.0.1:8201",
  "admins": [{ "messenger_id": "web", "user_id": "u2" }],
  "bind_users": [{ "messenger_id": "web", "user_id": "u1" }],
  "agent_name": "",
  "role_name": "",
  "outgoing": { "messenger_id": "web", "user_id": "u1", "group_id": "g1" },
  "enabled": true
}
```
（模板带 `outgoing` 使 agent 默认可回复，现有集成测试不回归；`test/workspace/agent-data/nexus.json` 为 gitignored 工作副本，resetWorkspace 从 template 复制，无需手改。）

- [ ] **Step 5: session_manager.rs 删 resolve_send_channel + 测试改**

删除 `kissbot-agent/src/session_manager.rs` 中 `resolve_send_channel` 方法（用 `is_send_channel`，已删）。`sample_channel` 测试函数改：
```rust
fn sample_channel(id: &str, agent: &str, role: &str) -> ChannelConfig {
    ChannelConfig {
        channel_id: Arc::new(id.into()),
        ws_url: Arc::new("ws://127.0.0.1:8201".into()),
        admins: Arc::new(HashSet::new()),
        bind_users: vec![ChannelUser { messenger_id: "web".into(), user_id: "u1".into() }],
        outgoing: None,
        agent_name: Arc::new(agent.into()),
        role_name: Arc::new(role.into()),
        enabled: true,
    }
}
```
删除 `resolve_send_channel_flag_then_first` 测试（方法已删）。

- [ ] **Step 6: coordinator.rs 重构（out_channel 路由 + 发送拆分 + 分流）**

`kissbot-agent/src/coordinator.rs`：
1. import 追加：`use kissbot_api::channel::{IncomingMessage, OutgoingMessage, BindRequest, IncomingMessageEvent};`（IncomingMessage 可能仍需用于 extract 辅助；确认后保留）
2. `incoming_message` 步骤 3 的 `self_user_id` 改：
```rust
// 接收方身份 = event.recipient_user_id（agent 视角的 self；文件名按此分文件）
self_user_id: event.recipient_user_id.clone(),
```
3. 删除 `reply`、`send_reply`、`resolve_send_channel`、`set_send_channel` 四个方法。
4. `handle_incoming` 改为分流：
```rust
async fn handle_incoming(
    &self,
    channel_id: &str,
    ch: Arc<crate::config_manager::ChannelConfig>,
    event: Arc<IncomingMessageEvent>,
) {
    let messenger_id = event.incoming_message.messenger_id.to_string();
    let user_id = event.incoming_message.user_id.to_string();
    let content_text = extract_text(&event.incoming_message.content);

    // 1. 系统事件（群组变更/用户移除）不进 agentic loop
    match &event.incoming_message.content {
        Content::GroupJoin(_) | Content::GroupLeave(_) | Content::UserRemove(_) => return,
        _ => {}
    }

    // 2. 管理命令（无论有无 out_channel 都处理；回复发回来源 channel）
    if CommandRouter::is_command(&content_text) {
        if CommandRouter::check_admin(&self.config, channel_id, &messenger_id, &user_id).await {
            self.handle_admin_command(channel_id, &event, &content_text).await;
        }
        // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
        return;
    }

    // 3. 普通消息：无 out_channel 不进 Agentic Loop（ChannelRecord 已存，结束）
    let Some(out_channel) = self.resolve_out_channel(channel_id).await else {
        return;
    };
    let key = self.session_key_for(&ch);
    let (session, _) = self.ensure_session(&key, channel_id).await;
    self.run_agentic_loop(channel_id, &session, event, &out_channel).await;
}
```
5. `handle_admin_command` 改签名与回复路径：
```rust
async fn handle_admin_command(
    &self,
    channel_id: &str,
    event: &Arc<IncomingMessageEvent>,
    content: &str,
) {
    match CommandRouter::parse(content) {
        Ok(cmd) => {
            match CommandRouter::execute(&cmd, &self.config, self, channel_id).await {
                Ok((reply, effect)) => {
                    // 回复：系统命令始终发回来源 channel（不走 out_channel）
                    self.send_admin_reply(channel_id, event, reply).await;

                    // 应用命令执行效果
                    match effect {
                        crate::types::CommandEffect::Relocate => {
                            self.relocate_channel(channel_id).await;
                        }
                        crate::types::CommandEffect::ResetSession => {
                            self.reset_session_for(channel_id).await;
                        }
                        crate::types::CommandEffect::None => {}
                    }
                }
                Err(e) => {
                    self.send_admin_reply(channel_id, event,
                        format!("❌ 命令执行失败: {}", e)).await;
                }
            }
        }
        Err(e) => {
            self.send_admin_reply(channel_id, event,
                format!("⚠️ {}", e)).await;
        }
    }
}
```
6. `send_admin_reply` 新增：
```rust
/// 系统命令回复：始终发回来源 channel（不走 out_channel）
/// 身份：messenger_id = incoming.messenger_id；user_id/self_user_id = event.recipient_user_id（接收方即发声身份，且是群成员）
async fn send_admin_reply(&self, channel_id: &str, event: &Arc<IncomingMessageEvent>, content: String) {
    let Some(client) = self.channel_clients.get(channel_id) else {
        warn!("send_admin_reply: 未找到 channel client: {}", channel_id);
        return;
    };
    let Some(ch) = self.channel_config(channel_id).await else {
        warn!("send_admin_reply: 未找到 channel 配置: {}", channel_id);
        return;
    };

    let msg = OutgoingMessage {
        messenger_id: event.incoming_message.messenger_id.clone(),
        user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
        content: Content::Text(Arc::new(content.clone())),
    };

    match client.send_message(msg).await {
        Ok(response) => {
            // 下行成功后：先记 msg_id 到 pending（回显判定），再推记忆（is_self=1）
            let key = self.session_key_for(&ch);
            let role_name = memory_role(&key);
            let agent_id = self.channel_agent(channel_id).await;
            self.record_outgoing_msg_id(channel_id, &response.msg_id).await;
            self.memory_store_client.push_channel_record(ChannelRecord {
                agent_id,
                role_name: Arc::new(role_name),
                messenger_id: event.incoming_message.messenger_id.clone(),
                user_id: event.recipient_user_id.clone(),
                self_user_id: event.recipient_user_id.clone(),
                group_id: event.incoming_message.group_id.clone(),
                is_self: 1,
                messenger_name: response.messenger_name.clone(),
                user_name: response.user_name.clone(),
                group_name: response.group_name.clone(),
                content: response.content.clone(),
                time: response.time.clone(),
            }).await;
        }
        Err(e) => {
            warn!("send_admin_reply 失败: {:?}", e);
        }
    }
}
```
7. `resolve_out_channel` 新增：
```rust
/// 取来源 channel 所属 (agent,role) 的 out_channel（跨 channel 找有 outgoing 配置的，至多 1 个）
/// out_channel 跟 channel 不跟 mode：该 channel 所有 mode 的 session 共用
async fn resolve_out_channel(&self, channel_id: &str) -> Option<OutChannel> {
    let ch = self.channel_config(channel_id).await?;
    let channels = self.config.channels().await;
    for (_, c) in &channels {
        if c.agent_name == ch.agent_name && c.role_name == ch.role_name {
            if let Some(out) = &c.outgoing {
                return Some(OutChannel {
                    channel_id: c.channel_id.clone(),
                    user: ChannelUser {
                        messenger_id: out.messenger_id.to_string(),
                        user_id: out.user_id.to_string(),
                    },
                    group_id: out.group_id.clone(),
                });
            }
        }
    }
    None
}
```
8. `send_outgoing` 新增：
```rust
/// Agentic Loop 产出回复：发到 out_channel（channel_id + ChannelUser + group_id）
async fn send_outgoing(&self, out_channel: &OutChannel, content: String) {
    let Some(client) = self.channel_clients.get(out_channel.channel_id.as_str()) else {
        warn!("send_outgoing: 未找到 channel client: {}", out_channel.channel_id);
        return;
    };
    let Some(ch) = self.channel_config(out_channel.channel_id.as_str()).await else {
        warn!("send_outgoing: 未找到 channel 配置: {}", out_channel.channel_id);
        return;
    };

    let msg = OutgoingMessage {
        messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
        user_id: Arc::new(out_channel.user.user_id.clone()),
        group_id: out_channel.group_id.clone(),
        content: Content::Text(Arc::new(content.clone())),
    };

    match client.send_message(msg).await {
        Ok(response) => {
            // 下行成功后：先记 msg_id 到 pending（回显判定），再推记忆（is_self=1）
            let key = self.session_key_for(&ch);
            let role_name = memory_role(&key);
            let agent_id = self.channel_agent(out_channel.channel_id.as_str()).await;
            self.record_outgoing_msg_id(out_channel.channel_id.as_str(), &response.msg_id).await;
            self.memory_store_client.push_channel_record(ChannelRecord {
                agent_id,
                role_name: Arc::new(role_name),
                messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
                user_id: Arc::new(out_channel.user.user_id.clone()),
                self_user_id: Arc::new(out_channel.user.user_id.clone()),
                group_id: out_channel.group_id.clone(),
                is_self: 1,
                messenger_name: response.messenger_name.clone(),
                user_name: response.user_name.clone(),
                group_name: response.group_name.clone(),
                content: response.content.clone(),
                time: response.time.clone(),
            }).await;
        }
        Err(e) => {
            warn!("send_outgoing 失败: {:?}", e);
        }
    }
}
```
9. `run_agentic_loop` 签名与发送改：
```rust
async fn run_agentic_loop(&self, channel_id: &str, session: &Arc<Session>, event: Arc<IncomingMessageEvent>, out_channel: &OutChannel) {
    // 无可用模型：静默忽略普通消息（仅管理指令可用）
    if session.model.load().is_none() {
        return;
    }
    let content_text = extract_text(&event.incoming_message.content);
    let messenger_id = event.incoming_message.messenger_id.to_string();
    let user_id = event.incoming_message.user_id.to_string();
    let group_id = event.incoming_message.group_id.to_string();
    let time = event.incoming_message.time.to_string();

    // 1. 追加用户消息到该会话上下文
    {
        let mut ctx = session.context.lock().await;
        ctx.push_user_message(ContextMessage::User {
            messenger_id: messenger_id.clone(),
            user_id: user_id.clone(),
            group_id: group_id.clone(),
            content: content_text.clone(),
            time: time.clone(),
        });
    }

    // 2. 调用模型（用该会话的模型）
    let response = {
        let ctx = session.context.lock().await;
        let messages = ctx.build();
        let model = session.model.load_full();
        let Some(pm) = model.as_ref() else { return; };
        let mc = self.model_client.lock().await;
        mc.call(pm, &messages).await
    };

    match response {
        Ok(model_resp) => {
            let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

            // 3. 记录已发送内容
            {
                let mut ctx = session.context.lock().await;
                ctx.push_assistant(model_resp.content.clone(), now.clone());
            }

            // 4. 推送 think 到 memory-store（事件模式编码；取记忆用会话保存的 agent_id）
            // Think 记忆只存思考内容（方案 A）：有思考内容才写，无则跳过
            if let Some(reasoning) = &model_resp.reasoning_content {
                let role_name = memory_role(&session.key);
                self.memory_store_client.push_think(
                    session.agent_id.to_string(),
                    Some(role_name),
                    reasoning.clone(),
                    now,
                ).await;
            }

            // 5. 发送回复到该会话的 out_channel
            self.send_outgoing(out_channel, model_resp.content).await;

            // 6. 检查上下文超长
            let overflow = {
                let ctx = session.context.lock().await;
                ctx.is_overflow()
            };
            if overflow {
                warn!("会话上下文超长，触发重置: {:?}", session.key);
                self.reset_context(session).await;
            }
        }
        Err(e) => {
            warn!("模型调用失败: {:?}", e);
            self.send_outgoing(out_channel,
                format!("❌ 模型调用失败: {}", e)).await;
        }
    }
}
```
10. `load_ego_info` 的 ids 收集改（遍历 bind_users）：
```rust
// agent 自身活跃标识集合：来自各 channel 绑定身份（messenger_id, user_id；群组不限定）
let mut ids = std::collections::HashSet::new();
for (_, ch) in self.config.channels().await {
    for bu in &ch.bind_users {
        ids.insert(kissbot_api::ChannelUser {
            messenger_id: bu.messenger_id.clone(),
            user_id: bu.user_id.clone(),
        });
    }
}
```
11. 删除 import 中不再使用的 `BindRequest`（若 connect_channels 仍用则保留；connect_channels 里 `client_clone.bind(BindRequest {...})` 仍存在，保留 import）。coordinator 里所有 `ch.bind_user` 引用替换完毕（仅 load_ego_info 与 incoming_message 两处，其余在 send_reply 内已随方法删除）。

- [ ] **Step 7: 修复 config_manager 既有测试的 ChannelConfig 构造**

`kissbot-agent/src/config_manager.rs` 既有测试（update_channel / 兼容性测试等）构造 `ChannelConfig` 的 `bind_user:` 与 `is_send_channel:` 字段替换为 `bind_users: vec![...]`、`outgoing: None`。逐一对照编译错误修复。

- [ ] **Step 8: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test`
Expected: 全部 PASS（kissbot-agent 编译 + 既有测试，含 config_manager 新单测；session_manager 测试适配后通过）。

- [ ] **Step 9: 提交**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/config_manager.rs kissbot-agent/src/session_manager.rs kissbot-agent/src/coordinator.rs script/template/nexus.json test/workspace-template/agent-data/nexus.json
git commit -m "refactor(agent): ChannelConfig 改 bind_users/outgoing 删 is_send_channel——新增 OutChannelConfig/OutChannel；coordinator 重构：resolve_out_channel 实时查配置、send_outgoing 走 out_channel、send_admin_reply 发回来源 channel（身份=recipient_user_id）、普通消息无 out_channel 不进 Agentic Loop；删 reply/send_reply/resolve_send_channel/set_send_channel；load_ego_info 遍历 bind_users；配置文件直接改 bind_users+outgoing"
```

---

### Task 3: 命令（/bind 追加、/unbind 带 user_id、/bind-outgoing 设/清空）

**Files:**
- Modify: `kissbot-agent/src/types.rs`（`AdminCommand` 改 Bind/Unbind 语义、新增 `BindOutgoing`、删 `SendChannel`；新增 `OutChannelParams`）
- Modify: `kissbot-agent/src/command_router.rs`（解析 + 执行）

**Interfaces:**
- Consumes: `ChannelConfig`（Task 2：bind_users/outgoing）、`OutChannelConfig`（Task 2）、`ConfigManager::update_channel/channels`
- Produces: `AdminCommand::{Bind{messenger_id,user_id}, Unbind{messenger_id,user_id}, BindOutgoing(Option<OutChannelParams>)}`；`OutChannelParams { messenger_id: String, user_id: String, group_id: String }`；命令格式 `/bind messenger <m> <u>`、`/unbind messenger <m> <u>`、`/bind-outgoing <m> <u> <g>`、`/bind-outgoing off`

- [ ] **Step 1: 写失败测试（解析）**

`kissbot-agent/src/command_router.rs` 测试模块追加：
```rust
#[test]
fn parse_bind_outgoing_params_and_off() {
    let cmd = CommandRouter::parse("/bind-outgoing web u1 g1").unwrap();
    match cmd {
        AdminCommand::BindOutgoing(Some(p)) => {
            assert_eq!(p.messenger_id, "web");
            assert_eq!(p.user_id, "u1");
            assert_eq!(p.group_id, "g1");
        }
        _ => panic!("expected BindOutgoing(Some)"),
    }
    let off = CommandRouter::parse("/bind-outgoing off").unwrap();
    assert!(matches!(off, AdminCommand::BindOutgoing(None)), "off 应清空");
    // 参数不足拒绝
    assert!(CommandRouter::parse("/bind-outgoing web u1").is_err());
}

#[test]
fn parse_unbind_requires_user_id() {
    let cmd = CommandRouter::parse("/unbind messenger web u1").unwrap();
    match cmd {
        AdminCommand::Unbind { messenger_id, user_id } => {
            assert_eq!(messenger_id, "web");
            assert_eq!(user_id, "u1");
        }
        _ => panic!("expected Unbind"),
    }
    assert!(CommandRouter::parse("/unbind messenger web").is_err(), "缺 user_id 应拒绝");
}

#[test]
fn parse_send_channel_removed() {
    assert!(CommandRouter::parse("/send-channel on").is_err(), "/send-channel 已删除");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test parse_bind_outgoing`
Expected: 编译错误（`BindOutgoing`/`OutChannelParams` 未定义；`Unbind` 无 user_id 字段；`SendChannel` 仍存在）。

- [ ] **Step 3: types.rs AdminCommand 改**

`kissbot-agent/src/types.rs`：
```rust
#[derive(Debug)]
pub enum AdminCommand {
    Bind { messenger_id: String, user_id: String },
    // messenger_id 字段为命令解析兼容保留（执行侧按 channel_id 定位，不再读取）
    Unbind { messenger_id: String, user_id: String },
    /// 设/清空 out_channel：Some 设（覆盖 + 同 agent/role 唯一），None 清空
    BindOutgoing(Option<OutChannelParams>),
    Admin { messenger_id: String, user_id: String },
    Unadmin { messenger_id: String, user_id: String },
    SetRole(Option<String>),
    ModeEvent(Option<String>),
    ModeRole,
    Reenter(String),
    Events,
    Reset,
    Model(ProviderModel, bool),
    /// 设置 channel 绑定的 agent 与 role（缺省用保留值：agent_name=""、role_name=""）
    SetAgent { agent_name: Option<String>, role: Option<String> },
}
```

新增（AdminCommand 定义之前或之后）：
```rust
/// /bind-outgoing 命令参数（转 OutChannelConfig 持久化）
#[derive(Debug, Clone)]
pub struct OutChannelParams {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
}
```
（注：`AdminCommand::Bind` 保留原字段；删除 `SendChannel` 变体。）

- [ ] **Step 4: command_router.rs 解析改**

`parse` 中 `"unbind"` 分支替换：
```rust
"unbind" => {
    if parts.len() < 4 || parts[1] != "messenger" {
        return Err(Error::InvalidCommand(
            "格式: /unbind messenger <messenger_id> <user_id>".to_string()
        ));
    }
    Ok(AdminCommand::Unbind {
        messenger_id: parts[2].to_string(),
        user_id: parts[3].to_string(),
    })
}
```

新增 `"bind-outgoing"` 分支（替换原 `"send-channel"` 分支位置）：
```rust
"bind-outgoing" => {
    // /bind-outgoing <messenger_id> <user_id> <group_id> 或 /bind-outgoing off
    if parts.len() == 2 && parts[1] == "off" {
        return Ok(AdminCommand::BindOutgoing(None));
    }
    if parts.len() < 4 {
        return Err(Error::InvalidCommand(
            "格式: /bind-outgoing <messenger_id> <user_id> <group_id> 或 /bind-outgoing off".to_string()
        ));
    }
    Ok(AdminCommand::BindOutgoing(Some(OutChannelParams {
        messenger_id: parts[1].to_string(),
        user_id: parts[2].to_string(),
        group_id: parts[3].to_string(),
    })))
}
```
删除 `"send-channel"` 分支。

- [ ] **Step 5: command_router.rs 执行改**

`execute` 中 Bind/Unbind/BindOutgoing 分支替换：
```rust
AdminCommand::Bind { messenger_id, user_id } => {
    config.update_channel(channel_id, |c| {
        // 追加去重：已存在则幂等忽略
        let cu = ChannelUser { messenger_id: messenger_id.clone(), user_id: user_id.clone() };
        if !c.bind_users.iter().any(|b| b == &cu) {
            c.bind_users.push(cu);
        }
    }).await?;
    Ok((format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id), CommandEffect::None))
}
AdminCommand::Unbind { messenger_id, user_id } => {
    config.update_channel(channel_id, |c| {
        // 移除指定 ChannelUser
        c.bind_users.retain(|b| !(b.messenger_id == *messenger_id && b.user_id == *user_id));
        // 移除的是 outgoing 引用身份则清空 outgoing（避免悬空引用）
        if let Some(out) = &c.outgoing {
            if out.messenger_id.as_str() == messenger_id && out.user_id.as_str() == user_id {
                c.outgoing = None;
            }
        }
    }).await?;
    Ok((format!("✅ 已移除 channel 用户: {} / {}", messenger_id, user_id), CommandEffect::None))
}
AdminCommand::BindOutgoing(params) => {
    match params {
        Some(p) => {
            // 1. 校验 ChannelUser 已绑定
            let channels = config.channels().await;
            let src = channels.iter().find(|(id, _)| id == channel_id).map(|(_, c)| c.clone())
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let bound = src.bind_users.iter()
                .any(|b| b.messenger_id == p.messenger_id && b.user_id == p.user_id);
            if !bound {
                return Err(Error::InvalidCommand(format!(
                    "ChannelUser 未绑定: {} / {}", p.messenger_id, p.user_id)));
            }
            // 2. 清空同 (agent_name, role_name) 其他 channel 的 outgoing（保证至多 1 个）
            for (cid, c) in channels.iter() {
                if cid != channel_id && c.agent_name == src.agent_name && c.role_name == src.role_name {
                    if c.outgoing.is_some() {
                        config.update_channel(cid, |cc| cc.outgoing = None).await?;
                    }
                }
            }
            // 3. 设来源 channel 的 outgoing
            config.update_channel(channel_id, |c| {
                c.outgoing = Some(OutChannelConfig {
                    messenger_id: Arc::new(p.messenger_id.clone()),
                    user_id: Arc::new(p.user_id.clone()),
                    group_id: Arc::new(p.group_id.clone()),
                });
            }).await?;
            Ok((format!("✅ 已设发送通道: {} / {} -> {}", p.messenger_id, p.user_id, p.group_id), CommandEffect::None))
        }
        None => {
            config.update_channel(channel_id, |c| c.outgoing = None).await?;
            Ok(("✅ 已取消发送通道（只存不回复）".to_string(), CommandEffect::None))
        }
    }
}
```
删除 `AdminCommand::SendChannel(on)` 分支。删除 `coordinator.set_send_channel` 相关引用（Task 2 已删该方法，此处无残留）。

- [ ] **Step 6: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test`
Expected: 全部 PASS（新解析测试 + 既有测试）。

- [ ] **Step 7: 提交**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/types.rs kissbot-agent/src/command_router.rs
git commit -m "feat(agent): 命令改 bind 追加去重/unbind 带 user_id 移除并清空引用的 outgoing/新增 bind-outgoing 设 out_channel（校验已绑+同 agent/role 唯一）与 off 清空；删除 send-channel 命令"
```

---

### Task 4: 集成测试（out_channel 路由端到端）

**Files:**
- Test: `test/tests/nexus-ego-chat-store.spec.ts`（适配 or 新增用例）
- Modify: `test/tests/helpers/server.ts`（如需要：模板配置断言/查询辅助）

**Interfaces:**
- Consumes: Task 1-3 产出的命令与路由行为；测试模板 nexus.json（Task 2 已加 bind_users+outgoing）

- [ ] **Step 1: 确认模板配置生效（既有测试不回归）**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/nexus-ego-chat-store.spec.ts --grep "场景1"`
Expected: 1 passed（模板 outgoing g1 使 agent 经 out_channel 回复；channel 记忆仍写入）。

- [ ] **Step 2: 新增 bind-outgoing 切换测试**

在 `test/tests/nexus-ego-chat-store.spec.ts` 或新 spec 追加用例（伪代码步骤）：
```ts
// 1. 启动服务（模板 outgoing 已设 web/u1 -> g1）
// 2. u2 发普通消息 -> agent 回复（走 out_channel）
// 3. 管理员（u2 是 admins）发 /bind-outgoing off -> 回复"已取消发送通道"
// 4. u2 再发普通消息 -> 无回复（不进 Agentic Loop），但 channel 记录仍写入（is_self=0）
//    查询 /store/query/channel（start/end 大范围）断言存在该用户消息记录
// 5. 管理员发 /bind-outgoing web u1 g1 -> 恢复 out_channel
// 6. u2 再发普通消息 -> agent 又回复
```
验证点：
- 步骤 4 无回复：等待超时后断言无新 outgoing（如 `expect(cli.getOutput()).not.toContain(...)` 或用 sleep 后查 channel 记录只有 is_self=0）
- channel 记录存在：复用现有 `assertChannelRecords` 断言（含 is_self=0 用户消息）

- [ ] **Step 3: 新增多绑定 + /bind 追加 + /unbind 清空 outgoing 测试**

```ts
// 1. 管理员发 /bind messenger web u3 -> 追加 bind_users（回复确认）
// 2. 管理员发 /bind-outgoing web u3 g1 -> 设 outgoing 指向新绑 u3
// 3. 管理员发 /unbind messenger web u3 -> 移除 u3 且 outgoing 自动清空（回复确认）
// 4. u2 发普通消息 -> 无回复（outgoing 已清空，只存不回复）
```
验证点：步骤 3 回复确认；步骤 4 无回复 + channel 记录仍有。

- [ ] **Step 4: 提交**

```bash
cd /home/admin/project/kissbot
git add test/tests/nexus-ego-chat-store.spec.ts test/tests/helpers/server.ts
git commit -m "test(nexus-ego-chat-store): out_channel 路由集成测试——bind-outgoing off 只存不回复/恢复后回复/bind 追加与 unbind 清空 outgoing"
```

---

### Task 5: 全量验证

- [ ] **Step 1: 单测全量**

Run: `cd /home/admin/project/kissbot && cargo test`
Expected: 全部 PASS，无 warning（注意：删除 `resolve_send_channel`/`set_send_channel` 后确认无 dead-code 告警；`IncomingMessageEvent` 全链路类型一致）。

- [ ] **Step 2: 集成测试全量**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/nexus-chat.spec.ts tests/nexus-ego-chat-store.spec.ts tests/agent-commands.spec.ts tests/agent-config-api.spec.ts`
Expected: 全部 passed（含 Task 4 新增用例）。

- [ ] **Step 3: 检查未提交改动**

Run: `cd /home/admin/project/kissbot && git status --short`
Expected: 无未提交改动（`test/workspace/agent-data/nexus.json` 为 gitignored 工作副本，不出现）。
