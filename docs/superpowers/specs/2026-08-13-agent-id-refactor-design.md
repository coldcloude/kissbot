# Agent 标识统一改造：agent_name → agent_id

日期：2026-08-13
状态：设计已确认

## 1. 目标

agent 侧彻底消灭 `agent_name`，会话标识全链路改用 `agent_id`（UUID 或保留值 `"0"`）；
删除 agent_name → agent_id 的运行时解析/缓存链（不再需要获取或缓存该关联关系）。

改造范围仅 `kissbot-agent`；kissbot-memory-ego 的 `/agent/search-name` 接口保留（管理界面等其他用途），本次不动。

## 2. 核心决策

| # | 决策 | 说明 |
|---|------|------|
| 1 | 配置直接存 `agent_id` | `ChannelConfig.agent_name` 改名为 `agent_id`，nexus.json 直接写 UUID |
| 2 | `agent_id` 非空不变式 | 恒非空；保留 agent 显式写 `"0"`（`RESERVED_AGENT_ID`） |
| 3 | 入口归一化 `""` → `"0"` | 程序加载、API 修改、命令修改三处入口，遇到空串自动变 `"0"`，不拒绝 |
| 4 | `/agent` 切换校验存在 | `/agent <agent_id> [role]` 接受 UUID，切换前调 ego `/agent/get` 校验，不存在保持原 agent |
| 5 | 删除运行时绑定链 | `channel_agent` / `bind_channel_runtime` / `set_channel_runtime` / `resolve_agent_id_for_bind` / `resolve_agent_id_http` 全删；`Session.agent_id` 直接来自 SessionKey |
| 6 | 不引入纯函数构造器 | 删 `session_key_of`，`session_key_for` 直接构造；不变式由入口校验保证，中间层无归一化 |

## 3. 改动清单

### 3.1 配置层（config_manager.rs）

- `ChannelConfig.agent_name: Arc<String>` → `agent_id: Arc<String>`
  - `#[serde(default = "default_agent_id", deserialize_with = "deserialize_agent_id")]`
  - 缺省 → `"0"`；显式 `""` → 自动归一化为 `"0"`
  - 注释：绑定的 agent_id（UUID；空 = 保留 agent = `"0"`，建会话用默认系统提示词，不调 memory-ego）
- `role_name: Arc<String>` 不变（`serde(default)` 空串 = 保留 role）
- `NexusRepo.context` map key 语义：`agent_name → AgentContextConfig` → `agent_id → AgentContextConfig`（三层继承不变）
- `context_config(agent_name, role_name)` → `context_config(agent_id, role_name)`
- 相关测试字段名/默认值更新

### 3.2 coordinator.rs

- 删常量 `RESERVED_AGENT_NAME`（仅保留 `RESERVED_AGENT_ID = "0"`）
- 删 `session_key_of` 纯函数
- `session_key_for` 直接构造：
  ```rust
  fn session_key_for(&self, ch: &ChannelConfig) -> SessionKey {
      SessionKey {
          agent_id: ch.agent_id.to_string(),
          role_name: ch.role_name.to_string(),
          mode: self.channel_mode(&ch.channel_id),
      }
  }
  ```
- `channel_session_key` 改为复用 `session_key_for`
- 删运行时绑定链：`channel_agent` / `bind_channel_runtime` / `set_channel_runtime` / `resolve_agent_id_for_bind` / `resolve_agent_id_http`
- `ensure_session` 删除 `let agent_id = self.channel_agent(...)`（`get_or_create` 不收 agent_id，Session.agent_id 来自 key）
- 记忆写入身份：`incoming_message` / `send_admin_reply` / `send_outgoing` 中 `channel_agent(channel_id)` → `Arc::new(key.agent_id.clone())`（key 即会话三元组，与 Session 一致）
- `ConfigChange::ApplyKey` 删除 `agent_id` 字段；`change_channel_key(channel_id, new_key)` 去掉 agent_id 参数；`apply_channel_key` 写 `c.agent_id = Arc::new(new_key.agent_id.clone())`
- `resolve_out_channel` / `resolve_out_channel_for_session`：`c.agent_id` / `session.agent_id`
- `enqueue_batch` / `tools_for_session` / `execute_tool_call`：`context_config(session.agent_id.as_str(), ...)`
- 新增公共方法 `verify_agent_exists(&self, agent_id: &str) -> Result<()>`：`"0"` 直接 Ok；否则 POST ego `/agent/get`，data 为 Some 才 Ok

### 3.3 command_router.rs

- `channel_current_key` fallback：`SessionKey { agent_id: RESERVED_AGENT_ID.to_string(), role_name: String::new(), mode: Mode::Role }`
- `/agent` 解析：变量名 `agent_name` → `agent_id`（`AdminCommand::SetAgent { agent_id, role }` 字段名已在 types.rs 改好）
- `SetAgent` 执行：
  1. `new_agent_id = agent_id.filter(|s| !s.is_empty()).unwrap_or(RESERVED_AGENT_ID)`（缺省或空串 → `"0"`）
  2. 非 `"0"` 时先 `coordinator.verify_agent_exists(&new_agent_id).await?`，失败返回 Err 保持原 agent
  3. 构造 new_key → `coordinator.change_channel_key(channel_id, new_key).await?`
- `SetRole` / `ModeEvent` / `ModeRole` / `Reenter`：`cur.agent_name` → `cur.agent_id`
- `BindOutgoing` 唯一性：`c.agent_name == src.agent_name` → `c.agent_id == src.agent_id`
- 相关测试更新

### 3.4 types.rs

- `AdminCommand::SetAgent { agent_id, role }` 已改好；注释更新（`agent_id=""` → `agent_id="0"` 保留语义）

### 3.5 http_server.rs

- 测试 JSON：`"agent_name": ""` → `"agent_id": "0"`
- `POST /config/channels` 反序列化自动走 `deserialize_agent_id`（`""` → `"0"`）

### 3.6 测试更新

- 删除：`session_key_of_*` 测试、`resolve_agent_id_http_*` 测试、`should_write_think` 残留测试（函数已删，测试尚存）
- 新增：`deserialize_agent_id` 归一化测试（`""` → `"0"`、缺省 → `"0"`、UUID 原样）
- `config_manager` / `command_router` 测试字段名与默认值更新

## 4. 数据流（改造后）

- **启动**：不再 `bind_channel_runtime`；对每个 channel 直接 `session_key_for(&ch)` → `ensure_session`
- **普通消息**：`incoming_message` 用 `key.agent_id`（归一化后的 `"0"` 或 UUID）推记忆；会话 key 同样来源
- **/agent 切换**：输入 UUID → `verify_agent_exists` 校验 → `change_channel_key` 写 config agent_id → 会话重定位
- **保留 agent**：config `agent_id` 缺省/空 → `"0"` → `ensure_session` 判 `== RESERVED_AGENT_ID` 用 NexusRepo 默认系统提示词，否则 `load_ego_info(agent_id, role_name)`

## 5. 错误处理

- `/agent` 校验失败：返回 Err，不写 config，保持原 agent 不变
- ego 不可达/未配置：`verify_agent_exists` 返回 Err → `/agent` 命令失败，保持原 agent
- 旧配置 `agent_name` 字段：serde 默认忽略（无 deny_unknown_fields）→ `agent_id` 缺省 `"0"` → 旧绑定 agent 回落为保留 agent（提示性，不自动迁移）

## 6. 迁移影响（提示性，不自动处理）

- 旧 nexus.json 需人工把 `agent_name` 改为 `agent_id`（UUID），否则回落为保留 agent `"0"`
- 会话缓存文件名 `{agent_id}-{role_name}-{event}` 随 key 变化，旧缓存文件不再命中（不影响正确性）
