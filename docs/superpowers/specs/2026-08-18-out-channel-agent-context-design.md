# 2026-08-18 out_channel 移入 (agent, role) 的 ContextConfig 设计

## 背景与动机

当前 out_channel（回复通道）配置存在 `ChannelConfig.outgoing`（per-channel），运行时经
`resolve_out_channel` / `resolve_out_channel_for_session` 跨全部 channel 遍历「同 (agent, role)
有 outgoing 配置的 channel」动态解析（每次合批 flush 全量遍历，浪费且语义绕）。

out_channel 本质是「某 (agent, role) 会话的回复通道」，应归属会话上下文配置而非 channel 绑定：
- 配置移到 `ContextConfig`（`AgentContextConfig.default_context_config` 与 `roles[role_name]` 复用），
  即 (agent, role) 级——role 覆盖 or agent 默认回落，与 ContextConfig 现有字段语义一致
- 命令 `/bind-outgoing`、`/unbind-outgoing` 修改 (agent, role) 的 out_channel 配置（纯配置写，
  `write_nexus_config` 锁内原子，无需队列），不再改 channel
- 发送校验：`send_outgoing` 发送前校验 out_channel 身份在目标 channel 仍绑定；未绑定 → 返回失败
  并清理该 (agent, role) 的 out 配置

## 1. 配置结构（config_manager.rs）

- `ContextConfig` 加字段：
  ```rust
  /// out_channel（agent+role 级回复通道；/bind-outgoing、/unbind-outgoing 修改；role 覆盖 or agent 默认回落）
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub out_channel: Option<Arc<OutChannel>>,
  ```
- `OutChannel` 补 `Serialize/Deserialize`（含 `channel_id` 发送目标，持久化为完整对象，读取即用）
- **删除 `OutChannelConfig`**（唯一用途是 ChannelConfig.outgoing）
- **删除 `ChannelConfig.outgoing`** 字段
- `merge_context_config` 增加 out_channel 回落合并（`role.out_channel` 覆盖，否则 `agent 默认`），
  `EffectiveContextConfig` 加 `pub out_channel: Option<Arc<OutChannel>>`；`context_config()` 一处读取

## 2. ConfigManager 新方法

- `set_out_channel(&self, agent_id: &str, role_name: &str, out: Option<Arc<OutChannel>>) -> Result<()>`
  - role_name 为空 → 写 `agent_contexts[agent_id].default_context_config.out_channel`
  - role_name 非空 → 写 `agent_contexts[agent_id].roles[role_name].out_channel`
  - `write_nexus_config` 单次原子（RwLock 锁内 clone-modify-store + 落盘），无需串行队列
  - agent 条目不存在则创建（ArcSwapHashMap entry 懒建）

## 3. 命令（command_router.rs）

- `"bind-outgoing"`：
  1. 取当前 channel 配置 `(agent_id, role_name)`
  2. 校验 `(messenger_id, user_id)` 在该 channel `bind_users` 中（未绑拒绝，InvalidCommand）
  3. 构造 `OutChannel { channel_id: 当前 channel, user: (m,u), group_id }`
  4. `ConfigManager::set_out_channel(agent_id, role_name, Some(out))`
- `"unbind-outgoing"`：取 `(agent_id, role_name)` → `set_out_channel(agent_id, role_name, None)`
- **删除** `ChannelCommand::BindOutgoing/ClearOutgoing` 变体、`ChannelManager::bind_outgoing/clear_outgoing`
  方法（含跨 channel 清理逻辑，key 唯一后不再需要）、`/unbind` 的「outgoing 引用清空」联动
- `apply_channel_command` 同步删两个分支；`ChannelCommand` 枚举缩为 BindUser/UnbindUser

## 4. 读取（删除两个 resolve 函数）

- 删除 `Nexus::resolve_out_channel` / `resolve_out_channel_for_session`
- `incoming_message` 步骤 5：`context_config(key.agent_id, key.role_name).out_channel` 为 None →
  跳过（不建会话不进 agentic loop；ChannelRecord 已存）
- `run_agentic_loop`（session_manager.rs）：`context_config(session.agent_id, session.role_name)
  .out_channel`，None → warn 跳过；Some → 发送用

## 5. send_outgoing 校验 + 失败清理

- 签名改为 `pub async fn send_outgoing(&self, agent_id: &str, role_name: &str, out_channel: &OutChannel, content: Arc<String>)`
  （agent/role 来自调用方 session，不再从目标 channel 配置推断）
- 发送前校验：`ConfigManager::channel(out_channel.channel_id).bind_users` 含 `out_channel.user`
  - 未绑定 → `set_out_channel(agent_id, role_name, None)` 清理该 (agent, role) out 配置 + warn 跳过发送
  - 绑定通过 → 发送；推记忆用参数 `(agent_id, role_name)` + 目标 channel 运行态 mode（memory_role 编码）

## 6. E2E 测试迁移

- `nexus-ego-chat-store.spec.ts` 场景 5：落盘断言 `readChannelConfig().outgoing` →
  改读 `agent_contexts[agent_id]` 的 out_channel（role 覆盖 or agent 默认定位）
- 场景 6：「unbind 移除后 outgoing 自动清空」→ 新语义「unbind 不动 out_channel；send 时校验失败才清理」，
  断言相应调整（unbind 后配置仍存在；send 后清理）
- 测试模板 `test/workspace-template/**/nexus.json`：channel 的 outgoing 段迁移到 agent context

## 数据流

```
/bind-outgoing <m> <u> <g>（channel X 执行）
  → 取 X 的 (agent_id, role_name) → 校验已绑定
  → OutChannel { channel_id: X, user: (m,u), group_id }
  → ConfigManager::set_out_channel(agent_id, role_name, Some)（write_nexus_config 原子写）

agentic loop flush
  → context_config(agent, role).out_channel（None → 跳过）
  → send_outgoing(agent_id, role_name, out_channel, content)
    → 校验目标 channel bind_users 含 out_channel.user
      → 未绑定：set_out_channel(agent, role, None) 清理 + 跳过
      → 绑定：发送 + 推记忆（is_self=1）
```

## 影响面

- `kissbot-agent/src/config_manager.rs`：ContextConfig/EffectiveContextConfig/merge + OutChannel serde +
  删 OutChannelConfig/ChannelConfig.outgoing + set_out_channel 方法
- `kissbot-agent/src/command_router.rs`：bind-outgoing/unbind-outgoing 分支改写；ChannelCommand 缩变体
- `kissbot-agent/src/channel_manager.rs`：删 bind_outgoing/clear_outgoing；unbind_user 删 outgoing 清空
- `kissbot-agent/src/nexus.rs`：删 resolve_out_channel*/apply_channel_command 两分支；
  incoming_message 步骤 5 改读 context；send_outgoing 签名+校验+清理
- `kissbot-agent/src/session_manager.rs`：run_agentic_loop 改读 context_config.out_channel；send_outgoing 传参
- `kissbot-agent/src/types.rs`：ChannelCommand 缩为 BindUser/UnbindUser
- 测试模板 + E2E 断言迁移
