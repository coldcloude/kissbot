# out_channel 路由模型重构 设计文档

> 子项目 1（前置）：out_channel 路由模型。子项目 2（Content 加 Think/ToolCall/ToolResult 变体 + think 双字段 + key 流程）在本设计之上叠加，另出 spec。

## 1. 背景与目标

### 1.1 问题
当前 channel 单绑定（`bind_user: ChannelUser`），`is_send_channel: bool` 标志发送 channel。存在三个未定义/缺陷：

1. **OutgoingMessage 的 group_id 未定义**：一个 session（agent+role+mode）的 IncomingMessage 可能来自不同 group_id（多群会话），当前 outgoing 直接用 incoming 的 group_id，假设单群。
2. **channel 不可重复绑定**：bind 覆盖式，无法绑多个 ChannelUser；unbind 无 user_id 参数。
3. **发送 channel 仅 bool 标志**：`is_send_channel` 不携带身份与 group 信息，无法表达"用哪个 ChannelUser 身份发到哪个 group"。

### 1.2 目标
- channel 支持多绑定（`bind_users: Vec<ChannelUser>`），bind 追加、unbind 带 ChannelUser 移除
- session 绑定至多 1 个 out_channel = (channel_id, ChannelUser, group_id)，替代 is_send_channel
- out_channel 统一用于所有 Agentic Loop 产出（OutgoingMessage、Think、ToolCall、ToolResult）的身份与 group
- 无 out_channel 时：处理系统命令 + 存 incoming ChannelRecord，但不进 Agentic Loop（不调模型、不产 Outgoing/Think/Tool）
- 系统命令回复始终发回来源 channel（不走 out_channel）

### 1.3 模型确认
- session **不分群**（保持 agent+role+mode），一个 session 可收多群 incoming（共享上下文）
- out_channel 绑定一个固定 group_id；多群 incoming 共享上下文，outgoing 只发 out_channel 的 group（agent 感知多群、主动发一群）
- out_channel 跟 **channel 不跟 mode**：存 channel 配置，该 channel 所有 mode 的 session 共用

## 2. 数据模型

### 2.1 ChannelConfig 结构变更（`kissbot-agent/src/config_manager.rs`）

```rust
pub struct ChannelConfig {
    pub channel_id: Arc<String>,
    pub agent_name: Arc<String>,
    pub role_name: Arc<String>,
    /// 多绑定（替代单 bind_user）；bind 追加去重，unbind 带 ChannelUser 移除
    pub bind_users: Vec<ChannelUser>,
    /// out_channel 配置（Option，至多 1 个；存于被绑定的 channel 下）
    pub outgoing: Option<OutChannelConfig>,
    pub ws_url: Arc<String>,
    pub enabled: bool,
    // is_send_channel 删除（不兼容旧值，配置文件直接改）
}
```

**不兼容旧格式**：`bind_user`/`default_bind_user` 单值不做 serde 兼容；`is_send_channel` 不做 serde 忽略。配置文件（nexus.json）直接改成新格式（见 §2.4）。

### 2.2 OutChannelConfig（持久化，nexus.json）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutChannelConfig {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
}
```

字段与 `/bind-outgoing` 三参数对应。`messenger_id`+`user_id` 共同标识 ChannelUser（多 messenger 下 user_id 可能重复，故需 messenger_id）。

### 2.3 OutChannel（运行态，coordinator 实时构造）

```rust
pub struct OutChannel {
    pub channel_id: Arc<String>,
    pub user: ChannelUser,    // (messenger_id, user_id)
    pub group_id: Arc<String>,
}
```

### 2.4 配置文件改动（直接改，不兼容）

涉及文件：
- `script/template/nexus.json`
- `test/workspace-template/agent-data/nexus.json`
- （`test/workspace/agent-data/nexus.json` 为 gitignored 工作副本，resetWorkspace 从 template 复制重建，无需手改）

改动：
- `bind_user: { ... }` -> `bind_users: [ { ... } ]`
- 删除 `is_send_channel` 字段
- `outgoing` 可选（template 可不设，运行时通过 `/bind-outgoing` 设置）

示例：
```json
{
  "channel_id": "web-main",
  "agent_name": "a1",
  "role_name": "r1",
  "bind_users": [
    { "messenger_id": "web", "user_id": "u1" },
    { "messenger_id": "web", "user_id": "u2" }
  ],
  "outgoing": { "messenger_id": "web", "user_id": "u1", "group_id": "g1" },
  "ws_url": "...",
  "enabled": true
}
```

## 3. 命令

### 3.1 AdminCommand 变更（`kissbot-agent/src/types.rs`）

```rust
pub enum AdminCommand {
    Bind { messenger_id: String, user_id: String },        // 语义：覆盖 -> 追加去重
    Unbind { messenger_id: String, user_id: String },      // 新增 user_id 参数（原仅 messenger_id）
    BindOutgoing(Option<OutChannelParams>),                // 新增：Some 设/覆盖，None 清空
    Admin { ... }, Unadmin { ... }, SetAgent { ... }, SetRole(Option<String>),
    ModeEvent(Option<String>), ModeRole, Reenter(String),
    Events, Reset, Model(ProviderModel, bool),
    // SendChannel 删除
}
```

`OutChannelParams`（命令参数，转 OutChannelConfig 持久化）：
```rust
pub struct OutChannelParams {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
}
```

### 3.2 命令格式（`kissbot-agent/src/command_router.rs`）

```
/bind messenger <messenger_id> <user_id>            追加 ChannelUser（已存在幂等忽略）
/unbind messenger <messenger_id> <user_id>          移除 ChannelUser；若被 outgoing 引用则清空 outgoing
/bind-outgoing <messenger_id> <user_id> <group_id>  设 out_channel（ChannelUser 须已绑；清除同 agent+role 其他 channel 的 outgoing）
/bind-outgoing off                                  清空 out_channel（回到"只存不回复"模式）
```

删除 `/send-channel` 解析分支。

### 3.3 执行逻辑（`CommandRouter::execute`）

- **Bind**：`update_channel` 追加 ChannelUser（去重，已存在则幂等忽略）；`CommandEffect::None`
- **Unbind**：`update_channel` 移除指定 ChannelUser；若移除的 = 该 channel `outgoing` 引用身份（messenger_id+user_id 匹配），清空 `outgoing`；`CommandEffect::None`
- **BindOutgoing(Some)**：
  1. 校验 ChannelUser 在来源 channel 的 `bind_users` 中（未绑拒绝，返回错误）
  2. `update_channel` 设 `outgoing`
  3. 遍历同 `(agent_name, role_name)` 的其他 channel，清空其 `outgoing`（保证 session 至多 1 个 out_channel）
  4. `CommandEffect::None`
- **BindOutgoing(None)**：清空来源 channel 的 `outgoing`；`CommandEffect::None`

**命令回复路径**：所有系统命令回复发回**来源 channel**（`send_admin_reply`，见 §4.5）。

## 4. session 与消息处理

### 4.1 IncomingMessageEvent 传递（ws -> Terminal）

当前 `process_incoming_message`（`kissbot-channel/src/channel_manager.rs`）解包 `event.incoming_message` 发给 ws，**丢弃 recipient_user_id**。改为发整个 `IncomingMessageEvent`。

`IncomingMessageEvent`（`kissbot-channel/src/data.rs`，已有）：
```rust
pub struct IncomingMessageEvent {
    pub recipient_user_id: Arc<String>,        // 接收方（agent self）
    pub incoming_message: Arc<IncomingMessage>,
}
```

改动：
- `process_incoming_message`：`payload = serde_json::to_value(event.as_ref())`（发整个 event，含 recipient_user_id）
- `Terminal::incoming_message`（`kissbot-channel-client/src/terminal.rs`）签名改为 `async fn incoming_message(&self, id: &str, message: Arc<IncomingMessageEvent>)`
- `kissbot-channel-client` ws 接收逻辑改 IncomingMessageEvent
- Coordinator `incoming_message` 处理 `Arc<IncomingMessageEvent>`

**IncomingMessage 不加字段**（保持原样）。

### 4.2 out_channel 运行态获取（实时查配置，不缓存）

config_manager 是 source of truth，实时查避免缓存不一致；bind-outgoing/unbind 改配置后立即生效。

```rust
/// 取来源 channel 所属 (agent,role) 的 out_channel（跨 channel 找有 outgoing 配置的，至多 1 个）
async fn resolve_out_channel(&self, channel_id: &str) -> Option<OutChannel> {
    let ch = self.channel_config(channel_id).await?;
    let channels = self.config.channels().await;
    for (_, c) in &channels {
        if c.agent_name == ch.agent_name && c.role_name == ch.role_name {
            if let Some(out) = &c.outgoing {
                return Some(OutChannel {
                    channel_id: c.channel_id.clone(),
                    user: ChannelUser {
                        messenger_id: out.messenger_id.clone(),
                        user_id: out.user_id.clone(),
                    },
                    group_id: out.group_id.clone(),
                });
            }
        }
    }
    None
}
```

### 4.3 incoming_message 流程

```
incoming_message(channel_id, event: Arc<IncomingMessageEvent>):
  1. 来源 channel 在配置中？否 -> return
  2. msg_id 回显判定：命中跳过
  3. 推上行消息到记忆（ChannelRecord, is_self=0）：
     - messenger_id/user_id/group_id = event.incoming_message 的
     - self_user_id = event.recipient_user_id（接收方 = agent self）
  4. 分流：
     - 系统事件（GroupJoin/GroupLeave/UserRemove）-> return
     - 管理命令 -> handle_admin_command（回复发回来源 channel；无论有无 out_channel 都处理）
     - 普通消息 -> resolve_out_channel(channel_id)：
       - 无 out_channel -> 不进 Agentic Loop（ChannelRecord 已存，结束）
       - 有 out_channel -> ensure_session + run_agentic_loop
```

### 4.4 run_agentic_loop（outgoing 走 out_channel）

```
run_agentic_loop(channel_id, session, event, out_channel):
  1. 无模型静默忽略
  2. 追加用户消息到上下文
  3. 调用模型
  4. 记录已发送内容到上下文
  5. send_outgoing(out_channel, content)  -- 见 §4.5
  6. think/tool 记录走 out_channel 身份（子项目 2 消费）
  7. 检查上下文超长
```

### 4.5 send_outgoing / send_admin_reply（拆分当前 send_reply）

**send_outgoing（Agentic Loop，走 out_channel）：**
- 发到 `out_channel.channel_id` 的 ChannelClient
- `OutgoingMessage { messenger_id: out_channel.user.messenger_id, user_id: out_channel.user.user_id, group_id: out_channel.group_id, content }`
- 成功后 push_channel_record（is_self=1）：`messenger_id`/`user_id`/`self_user_id` = `out_channel.user`（self_user_id = out_channel.user.user_id），`group_id` = `out_channel.group_id`；`messenger_name`/`user_name`/`group_name`/`content`/`time` 取自 OutgoingMessageResponse

**send_admin_reply（系统命令，走来源 channel）：**
- 发回来源 channel_id
- `OutgoingMessage { messenger_id: event.incoming_message.messenger_id, user_id: event.recipient_user_id, group_id: event.incoming_message.group_id, content }`（接收方即回复发声身份，且是群成员，send 校验通过）
- 成功后 push_channel_record（is_self=1）：`messenger_id` = `event.incoming_message.messenger_id`，`user_id`/`self_user_id` = `event.recipient_user_id`，`group_id` = `event.incoming_message.group_id`；`messenger_name`/`user_name`/`group_name`/`content`/`time` 取自 OutgoingMessageResponse

**删除**：`resolve_send_channel`、`set_send_channel`（依赖已删的 is_send_channel）。

## 5. 边界处理

### 5.1 unbind 一致性
`/unbind` 移除 ChannelUser 时，若该 ChannelUser（messenger_id+user_id 匹配）是所在 channel `outgoing` 引用身份，清空 `outgoing`（避免悬空引用）。

### 5.2 无 out_channel 行为
- 系统命令：正常处理，回复发回来源 channel
- incoming：转 ChannelRecord 存储（is_self=0，self_user_id=recipient_user_id）
- 普通消息：不进 Agentic Loop（不调模型、不产 Outgoing/Think/ToolCall/ToolResult）

### 5.3 out_channel 校验
`/bind-outgoing` 时校验 ChannelUser 在 bind_users 中（未绑拒绝）。配置文件手写出 outgoing 引用未绑 ChannelUser 属配置错误，运行时 send 失败告警（OutgoingMessage.user_id 非群成员，channel-web send 返回错误）。

### 5.4 out_channel 唯一性
`/bind-outgoing` 在 channel X 执行时，清空同 (agent_name, role_name) 其他 channel 的 outgoing，保证至多 1 个。

## 6. 涉及文件

| 文件 | 改动 |
|------|------|
| `kissbot-channel/src/channel_manager.rs` | `process_incoming_message` 发 IncomingMessageEvent（含 recipient_user_id） |
| `kissbot-channel-client/src/terminal.rs` | `Terminal::incoming_message` 签名改 `Arc<IncomingMessageEvent>` |
| `kissbot-channel-client` ws 接收 | 改 IncomingMessageEvent |
| `kissbot-agent/src/config_manager.rs` | ChannelConfig 改（bind_users、outgoing、删 is_send_channel）；新增 OutChannelConfig/OutChannel |
| `kissbot-agent/src/types.rs` | AdminCommand 改（Bind/Unbind 语义、新增 BindOutgoing、删 SendChannel）；OutChannelParams |
| `kissbot-agent/src/command_router.rs` | 解析（/bind 追加、/unbind 加 user_id、/bind-outgoing、删 /send-channel）+ 执行（校验、唯一性清除、unbind 清空 outgoing） |
| `kissbot-agent/src/coordinator.rs` | incoming_message 处理 IncomingMessageEvent；resolve_out_channel；send_outgoing/send_admin_reply 拆分；删 resolve_send_channel/set_send_channel；无 out_channel 不进 Agentic Loop |
| `kissbot-api/src/channel.rs` | 不变（IncomingMessage/OutgoingMessage 保持原样） |
| `script/template/nexus.json` | bind_user -> bind_users；删 is_send_channel |
| `test/workspace-template/agent-data/nexus.json` | 同上 |

## 7. 测试策略

### 7.1 单测
- **config_manager**：bind_users 序列化/反序列化；outgoing 配置读写；update_channel 追加/移除 ChannelUser；outgoing 清空
- **command_router**：
  - /bind 解析 + 追加去重（已存在幂等）
  - /unbind 解析（含 user_id）+ 移除 + 清空引用的 outgoing
  - /bind-outgoing 解析（三参数 + off）+ 校验 ChannelUser 已绑 + 唯一性清除（同 agent+role 其他 channel outgoing 清空）
  - /send-channel 已删除（解析报未知命令）
- **coordinator**：
  - resolve_out_channel：有 outgoing 返回 OutChannel；无返回 None；跨 channel 找同 (agent,role) 的
  - 无 out_channel 时普通消息不进 Agentic Loop（ChannelRecord 已存）
  - send_outgoing 走 out_channel 身份；send_admin_reply 走来源 channel 身份

### 7.2 集成测试
- 多绑定 + /bind-outgoing 设置 out_channel + outgoing 发送到 out_channel（验证 OutgoingMessage 的 messenger_id/user_id/group_id 来自 out_channel）
- 无 out_channel 时 incoming 只存 ChannelRecord 不回复（不进 Agentic Loop）
- /bind-outgoing off 清空后回到"只存不回复"
- /unbind 移除 out_channel 引用的 ChannelUser 后 outgoing 自动清空

## 8. 范围说明

本 spec 仅覆盖子项目 1（out_channel 路由模型）。子项目 2（Content 加 Think/ToolCall/ToolResult 变体 + think content 拆 reasoning_content/thinking 双字段 + key=UUID 关联流程 + MemoryStoreClient push 用 *Request 作参数删自定义 Record）在本设计之上叠加，另出 spec。

子项目 1 为子项目 2 提供的契约：out_channel 身份（channel_id + ChannelUser + group_id）作为 Think/ToolCall/ToolResult 记录的身份来源（§4.4 步骤 6 消费）。
