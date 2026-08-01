# Agent 多会话适配设计（代码变更）

设计文档（docs/design/components-design/kissbot-agent-nexus.md）已完成变更：一个 nexus 可同时管理多个会话。本文档是 **kissbot-agent 代码侧** 的适配设计，仅改 agent 代码，不改 memory-store / channel 等其他组件。

## 背景

当前 `AgentCoordinator` 是单会话假设：全局 `current_agent_id` / `current_role` / `current_mode` / `current_model`（ArcSwap 单值）、单个 `ContextBuilder`、运行态 `bound_channels`（ChannelUser 仅 messenger_id + user_id，不回写）。所有消息进入同一个 agentic loop，回复发回来源 channel。

新设计要求：

- 每个绑定 channel 配置 `(agent_id, role_name, mode)`，所有绑定项去重，每个三元组 = 一个会话
- 各会话独立的 LLM 上下文、记忆读取范围、模式状态、模型
- 消息按来源 channel 的绑定配置路由到对应会话
- 每个会话从绑定它的多个 channel 中选定一个作为发送（回复）channel

## 1. 数据结构与配置变更（配置即运行值，更新即回写）

```rust
pub struct ChannelConfig {
    pub channel_id: Arc<String>,
    pub ws_url: Arc<String>,
    pub admins: Arc<HashSet<ChannelUser>>,
    pub bind: ChannelBinding,      // 必填（当前阶段，auto-bind 以后再做）
    pub enabled: bool,
}

pub struct ChannelBinding {
    pub user: ChannelUser,          // messenger_id + user_id
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub is_send_channel: bool,      // 是否选为该会话的发送 channel
}
```

- `default_bind_user: Option<ChannelUser>` → `bind: ChannelBinding`（必填）；`enabled_by_default` → `enabled`
- **ChannelConfig 为唯一权威**：bind / enabled / agent_id / role_name / is_send_channel 的运行时修改（管理命令）经 ConfigManager 更新并 `save_nexus()` 回写
- **删除运行态 `bound_channels`**；coordinator 直接查 ConfigManager 的 channels
- **运行态仅保存 per-channel mode**：`DashMap<channel_id, Mode>`（默认 `Mode::Role`，`/mode`、`/reenter` 修改，不回写，重启回 Role）
- `/bind` 只设置 `bind.user`，不动 agent_id / role_name；`/unbind` **暂不进行任何操作**（返回提示即可），channel 必须保持 bind 状态
- **保留值**：`agent_id = "0"`、`role_name = "0"`
  - `/agent` 无参 → agent_id = "0"；`/agent <id>` 不带 role → role_name = "0"
  - `/role` 无参 → role_name = "0"
- **agent 脱离态**：agent_id 为 `"0"`（或空）表示不关联 agent——不生成会话，该 channel 只处理管理命令，普通消息不进入 agentic loop

## 2. SessionManager（合并 ContextBuilder，删除 context_builder.rs）

```rust
pub struct SessionKey {
    pub agent_id: String,
    pub role_name: String,
    pub mode: Mode,               // Hash + Eq；Event 含 event_id
}

struct Session {
    key: SessionKey,
    context: Mutex<SessionContext>,     // 原 ContextBuilder 全部逻辑内联：
                                        //   messages / system_message / sent_contents
                                        //   build() / is_overflow() / clear() 等
    model: ArcSwap<ProviderModel>,      // 会话级模型
}

pub struct SessionManager {
    sessions: DashMap<SessionKey, Arc<Session>>,
}
```

- `locate(channel_id) -> Option<Arc<Session>>`：按来源 channel 的 ChannelConfig.bind 三元组 + 运行态 mode 定位；不存在则创建（触发初始上下文构建）；agent 脱离态返回 None
- 绑定信息变化（/agent /role /mode /reenter /bind 回写后）：来源 channel 重新定位会话，必要时创建新会话；无任何 channel 绑定的会话销毁
- **会话模型**：创建时取 `NexusRepo.default_model`；`/model` 调整来源 channel 所属会话的模型（运行态，不回写，重启后新会话回到 default_model）。coordinator 的全局 `current_model` 删除
- **发送 channel**：取自绑定该会话的各 channel 的 `ChannelConfig.bind.is_send_channel`；均为 false 时首个绑定的 channel 兜底；`/send-channel on|off` 切换并回写（on 时清除同会话其他 channel 的标志）
- 锁粒度为会话级：不同会话的 agentic loop 可并行

## 3. 消息路由与 agentic loop

```
上行消息（channel_id, msg）
  → 查 ChannelConfig.bind（无 bind.user → 丢弃）
  → 按绑定三元组 + mode 写 channel 记录到记忆（事件模式 role_name 编码见 §4）
  → SessionManager::locate(channel_id)
    ├─ None（agent 脱离态）→ 仅管理命令可处理，普通消息丢弃
    └─ Some(session) → is_self 检测（该会话 sent_contents）→ 命中丢弃
  → 管理命令前缀？
    ├─ 是 → 管理员校验 → 对来源 channel/会话执行命令
    └─ 否 → 该会话的 agentic loop
```

- agentic loop 读写目标会话的 `SessionContext`；调用模型用会话的 `model`
- **回复一律走会话的发送 channel**（可能与来源 channel 不同）；发送 channel 不可用时记 warn 并回退来源 channel
- 上下文超长 → 只重置该会话（clear → 重载 ego / history / memory index）
- 命令与错误提示的回复同样走会话发送 channel；agent 脱离态 channel 的命令回复走来源 channel

## 4. 记忆读写

- **role_name 与 mode 在会话 key 中是独立字段**；仅记忆读写边界做编码：mode 为 `Event(event)` 时 role_name 编码为 `{role_name}-{event}`（横线分隔）
- 编码对 memory-store 透明（它只存字符串），memory-store 无任何改动；agent **读取侧**由现有 `role:event`（冒号）改为 `role-event`（横线），**写入侧**（channel 记录、think、tool call、tool result）补齐同样编码
- 上行/下行 channel 记录的 agent_id / role_name 取消息所属会话的 key（上行 = 来源 channel 的会话，下行 = 发送 channel 的会话）
- ego 读取用原始 agent_id + role_name（不带事件编码）
- 角色模式会话读取该角色全部记录；事件模式会话只读本事件记录（由编码后的 role_name 天然隔离）

## 5. 管理命令

| 命令 | 语义 | 回写 |
|------|------|------|
| `/bind messenger <mid> <uid>` | 设置来源 channel 的 `bind.user`（不动 agent/role） | 是 |
| `/unbind ...` | 暂不操作，回复提示 | — |
| `/agent [id] [role]` | 改来源 channel 的 agent_id（缺省 "0"）；role 缺省重置为 "0" | 是 |
| `/role [name]` | 改来源 channel 的 role_name（缺省 "0"） | 是 |
| `/mode event [event-id]` / `/mode role` | 改来源 channel 的运行态 mode（event 无参自动生成 UUID） | 否 |
| `/reenter <event-id>` | 来源 channel mode → Event(event-id) | 否 |
| `/send-channel on\|off` | 设置/取消来源 channel 的 is_send_channel（on 时清除同会话其他 channel 标志） | 是 |
| `/model <provider> <model>` | 改来源 channel 所属会话的模型 | 否 |
| `/reset` `/events` | 作用于来源 channel 对应会话 | — |
| `/admin` `/unadmin` | 不变 | 是 |

凡改动绑定三元组的命令，执行后触发来源 channel 的会话重定位（必要时创建新会话并构建初始上下文），并回复确认消息。非管理员命令忽略。

## 6. 启动流程

```
加载配置（ConfigManager）
→ 校验：enabled 的 channel 必须有 bind（当前阶段必填，缺失则告警并跳过该 channel）
→ SessionManager 按全部 channel 的 bind 三元组去重生成会话集合（跳过 agent 脱离态）
→ 每个会话：model ← default_model；加载 ego + history + memory index 构建初始上下文
→ 连接 enabled 的 channels，按 bind.user 发送绑定请求
→ 协调器就绪
```

## 7. 错误处理

- 记忆读写失败：沿用现状（读取失败用空历史，写入失败只记日志不重试）
- 模型调用失败：回复错误提示到会话发送 channel（回退来源 channel）
- 会话定位失败（bind 信息缺失）：消息丢弃并记日志
- 配置回写失败：命令返回错误提示

## 8. 测试

- **单测**：
  - SessionManager：三元组去重、locate 创建/复用、绑定变更后会话迁移、无绑定会话销毁
  - 发送 channel：is_send_channel 首选、首个绑定兜底、/send-channel 切换排他
  - 事件编码 `{role}-{event}` 读写一致；ego 用原始 role_name
  - agent 脱离态：普通消息丢弃、管理命令可执行
  - ChannelConfig 新结构 serde 往返；/agent /role 保留值缺省逻辑
- **集成测试**（playwright，沿用 channel-web + cli 基建）：
  - 两 channel 绑不同三元组 → 消息与上下文隔离
  - 两 channel 绑同三元组 → 共享会话，回复走发送 channel，`/send-channel` 切换后改走新 channel

## 影响文件

- 新增 `session_manager.rs`；删除 `context_builder.rs`
- 重写：`coordinator.rs`（路由按会话）、`config_manager.rs`（ChannelConfig 新结构 + bind/agent/role/send-channel 回写接口）、`command_router.rs`（命令语义）
- 调整：`types.rs`（Mode derive Hash/Eq、AdminCommand 增删）、`memory_reader.rs`（事件编码改横线）、`memory_writer.rs` / `memory_store_client.rs`（写入侧编码由调用方传入）、`http_server.rs`（channels 配置结构变化）
- 测试：`test/tests/agent-commands.spec.ts` 适配新命令语义，新增多会话用例
