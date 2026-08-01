# ego/channel 语义重构设计

> 日期：2026-08-02
> 主题：is_self 改 msg_id 匹配 + name 字段贯通 + memory-ego full_name/代号 + agent_name 绑定

## 1. 概述与目标

本次重构对 kissbot 的消息模型、自我认知（memory-ego）与 agent 绑定语义做一组联动改造，目标：

1. **is_self 判定去 user_id 化**：IncomingMessage 不再携带 is_self，改由 nexus 按 msg_id 识别自身发出消息的回显；ChannelRecord 保留 is_self，nexus 发出 OutgoingMessage 拿到 response 后存 is_self=1，收到 IncomingMessage 时仅对非 Outgoing 对应的存 is_self=0。
2. **name 字段贯通**：IncomingMessage / ChannelRecord 等保留 messenger_name / user_name / group_name，与 MessengerInfo / UserInfo / GroupInfo 一致，由 Messenger 填写。
3. **memory-ego 引入 full_name + 代号限制**：Role / RoleRelation 增加 full_name（展示文本），role_name / individual_name 收敛为代号（仅字母数字下划线）。
4. **agent 绑定改用 agent_name**：channel 绑定与 session_key 用 agent_name（= memory-ego 的 individual_name 代号）；memory-store 仍以 agent_id（UUID）为存储键，agent 运行时按需解析 agent_name -> agent_id。

## 2. 第 1 节 · is_self 机制（msg_id 匹配）

### 2.1 数据结构变更

- **IncomingMessage**（kissbot-api/channel.rs）：**移除 `is_self` 字段**。
- **OutgoingMessageResponse**（kissbot-api/channel.rs）：新增 `messenger_name / user_name / group_name`（`Arc<String>`），由 Messenger 填写，与 msg_id/time/content 一起回传。
- **ChannelRecord / ChannelRequest**（kissbot-api/memory.rs）：保留 `is_self`；新增 `messenger_name / user_name / group_name`。
- OutgoingMessage 不变（不加字段）。

### 2.2 ChannelContext（新增，per-channel 运行时）

nexus（kissbot-agent/coordinator）按 channel_id 持有运行时 `ChannelContext`：

```rust
struct ChannelContext {
    pending_outgoing: HashMap<Arc<String>, Instant>,  // msg_id -> 加入时刻
}
```

- 无容量上限；**TTL 懒清理**：访问时淘汰过期条目（默认 TTL 60s，可配）。
- coordinator 持有 `channel_contexts: DashMap<String, Arc<Mutex<ChannelContext>>>`，按 channel_id 索引。

### 2.3 回声判定流程

- **send_reply**（发回复）：拿到 OutgoingMessageResponse 后，**先**把 `response.msg_id` 加入 `ChannelContext[send_channel_id].pending`（带时刻），再 push is_self=1 的 ChannelRecord（name 取自 response）。顺序保证 pending 先于后续回显到达。
- **incoming_message**（收上行）：查 `ChannelContext[channel_id].pending`：
  - **命中** -> 移除该 msg_id、**跳过**（自身回显，不存 record、不进 agentic loop）。
  - **未命中** -> push is_self=0 的 ChannelRecord（name 取自 IncomingMessage），再进 handle_incoming 处理。
- 不再使用 user_id 判断 is_self。

### 2.4 移除内容兜底机制

移除 `kissbot-agent/session_manager` 的 `sent_contents` / `is_self_echo` / `record_sent_content`（msg_id 成为唯一权威判据）。

### 2.5 群组变更 / 用户移除的 is_self

- **群组变更**（Content::GroupJoin / GroupLeave）：经 `group_change_to_incoming_message_event` 转 IncomingMessage（无 is_self），msg_id 不在 pending -> 存为 **is_self=0** channel record（name 取自 GroupChangeNotification）。`handle_incoming` 改为**按 Content 类型**（GroupJoin/GroupLeave/UserRemove）跳过 agentic loop（不再靠 is_self==1 跳过）。
- **用户移除**（UserRemove）：**不存 channel record**（本次不涉及存储路径，维持现状仅走 TYPE_USER_REMOVED 回调）。UserRemoveNotification 仍按第 2 节加 name 字段。

## 3. 第 2 节 · name 字段贯通

### 3.1 结构变更

- **IncomingMessage**：新增 `messenger_name / user_name / group_name`，由 Messenger 从 MessengerInfo / UserInfo / GroupInfo 填写。
- **GroupChangeNotification**（kissbot-api/message.rs）：新增 `messenger_name / user_name / group_name`。
- **UserRemoveNotification**：新增 `messenger_name / user_name`（无 group）。
- 均由 Messenger 填写，与 MessengerInfo / UserInfo / GroupInfo 中对应字段一致。
- OutgoingMessage 不变。

### 3.2 各组件改动

- **kissbot-channel/data.rs** `group_change_to_incoming_message_event`：构造 IncomingMessage 时透传 name（来自 GroupChangeNotification），移除 `is_self`。
- **kissbot-channel/memory_store_client.rs**（当前未启用，lib.rs 注释保留）：推 ChannelRequest 时 is_self 置 0，name 取自 IncomingMessage；同步去 is_self 引用以保编译。
- **kissbot-channel-web/messenger.rs** `send()`：
  - 填 OutgoingMessageResponse 的 `messenger_name / user_name / group_name`（sender=bound 用户身份的 user_name、group_name、messenger_name，由 cfg 查得）。
  - 填 IncomingMessage（admin_msg 与分发消息）的 name（同上，sender 身份）。
  - 移除按 recipient 计算 is_self 的逻辑（分发消息不再有 per-recipient is_self，全部相同）。
  - `notify_group_change` / `remove_user` 填 GroupChangeNotification / UserRemoveNotification 的 name。
- **kissbot-channel-client-cli / mock**：更新 IncomingMessage 构造（测试）。

### 3.3 ChannelRecord 的 name 来源

- is_self=0 record：name 取自 IncomingMessage。
- is_self=1 record：name 取自 OutgoingMessageResponse（Messenger 填写）。

## 4. 第 3 节 · memory-ego full_name + 代号限制

### 4.1 结构变更（kissbot-api/ego.rs）

- **Role**：新增 `full_name: Arc<String>`（展示文本，可填）。
- **RoleRelation**：新增 `full_name: Arc<String>`（展示文本，可填）；`relation`（关系类型）**保留为自由文本**，不受代号限制。
- **AgentMetadata / OtherRole / IndividualRecognition**：不加 full_name（客观信息）；其 `individual_name` 为代号。
- full_name 与 description 一样为可填文本。

### 4.2 修改 API

- **Role.full_name**：新增专用 `update_role_full_name`（同 `update_role_description` 模式），路由 `/role/update-full-name`。
- **RoleRelation.full_name**：经现有 `update_other_role_relation`（全量替换，含 full_name）修改，与 RoleRelation.description 同途径（不加专用 API）。
- create_role / create_role_from / rename_role 等：构造 Role / RoleRelation 时 full_name 默认空串。

### 4.3 代号限制

- **正则**：`^[A-Za-z0-9_]+$`。
- **强制点**（memory-ego 写入入口，返回错误）：create_agent / update_agent_name；create_role / create_role_from / rename_role；replace_other_roles 的 key（= OtherRole 的 role_name）；update_other_role_individual_name；replace_other_role_relations 的 key（= 关联 role 的 role_name）；individual_recognition 的 replace_individuals / rename_individual 的 key。
- memory-ego 写入入口的 name **不允许空**。
- `ChannelConfig.agent_name` **不做校验**（空串合法，见第 4 节）。
- **individual_name 不做唯一性校验**：如实际重复，name_index HashMap 直接覆盖（last wins）。

### 4.4 search.rs 重构（kissbot-memory-ego/search.rs）

#### Agent 侧

- **新增 `name_index: HashMap<String, Arc<String>>`**（`individual_name -> agent_id`，全匹配，key 为 String）。sync 时用 `SearchMetadata.value[0]`（旧 individual_name）移除旧 key、插入新 key。
- **`search_by_name`**：返回类型由 `Vec<String>` 改为 **`Option<String>`**（全匹配，最多一个）。
- **`name_completion`**：**保留**，索引 `[individual_name]`（不变）。
- **`name_descr_index`**：**保留**为 `[individual_name, description]`（不改成仅 description）。
- **`SearchMetadata`**：不变 = `[individual_name, description]`。
- **`force_sync_identity`**：在现有逻辑基础上增加 name_index HashMap 的增删（用 SearchMetadata 旧 individual_name）。

#### Role 侧（最小改动）

- **`role_name_index`**：**不改**（保持 SubstringIndex<RoleKey>，substring 搜 role_name）。
- **`role_name_completion`**：**不改**（索引 role_name）。
- **`search_role_by_name`**：**不改**（substring）。
- **`role_name_descr_index`**：索引内容由 `[role_name, description]` 改为 **`[role_name, full_name, description]`**（加 full_name）。
- **`RoleSearchMetadata`**：由 `[role_name, description]` 改为 **`[role_name, full_name, description]`**。
- **`force_sync_role`**：更新 RoleSearchMetadata 构造与对比逻辑（full_name 变化时更新 role_name_descr_index）。

#### API 路由

- `/agent/search-name`：返回 `Option<String>`（全匹配）。
- `/agent/search-description`：不变（[individual_name, description] 子串）。
- `/agent/name-completion`：**保留**（individual_name 前缀补全）。
- `/role/search-name`：不变（agent_id 仍 Option，substring）。
- `/role/name-completion`：不变（role_name 前缀补全）。
- `/role/search-description`：不变（现索引 [role_name, full_name, description] 子串）。
- 新增 `/role/update-full-name`。

### 4.5 agent 解析与 ego 加载

- agent 用 agent_name 调 `/agent/search-name`（全匹配）-> `Option<agent_id>`，取 Some 得 agent_id；None 或 ego 不可用 -> 回退 `agent_id="0"`（保留 agent）。
- ego 加载：`/agent/get(agent_id)` 取 AgentMetadata（individual_name + description）入系统提示词。

## 5. 第 4 节 · agent_name 绑定

### 5.1 配置字段（kissbot-agent/config_manager.rs）

- `ChannelConfig.agent_id` -> **`agent_name`**（`Arc<String>`，代号或空）。
- 移除 `init_agent_id` / `init_role` / `NexusRepo.default_agent_id` / `NexusRepo.default_role` 及 main.rs 的 `"Agent ID:"` 日志。
- 保留 `init_model` / `default_model`（coordinator 启动校验在用）。
- `/agent [name] [role]`：name 参数语义为 agent_name；无参默认 `""`（保留 agent）。`/role [name]`：无参默认 `""`（保留 role）。

### 5.2 保留常量与语义

- `RESERVED_AGENT_NAME = ""`（config/agent_name 层的保留标记）。
- `RESERVED_AGENT_ID = "0"`（memory-store/ego 层的保留 agent_id；`agent_name=""` 解析为 `agent_id="0"`，特判、无 HTTP）。
- `RESERVED_ROLE_NAME = ""`（原 "0"；memory-store 已兼容空串）。
- `agent_name=""` -> 保留 agent（建会话、用 default_system_prompt、不调 memory-ego 解析、memory-store 用 agent_id="0"）。
- `agent_name="0"` -> 普通代号，正常解析（与保留无关）。
- 无脱离态：`session_key_of` **去掉 Option，始终返回 SessionKey**。

### 5.3 SessionKey 与解析（option Y）

- **SessionKey.agent_id -> `agent_name`**（字段改名，持代号）。`session_key_for(ch)` 同步返回 `SessionKey { agent_name: ch.agent_name, ... }`，**无须解析**。
- **memory-store 仍用 agent_id（UUID）**：coordinator 维护 `agent_id_cache: DashMap<agent_name, Arc<String>>` + 异步 helper `resolve_agent_id(agent_name) -> Arc<String>`：
  - `agent_name == ""` -> 返回 `"0"`（特判，无 HTTP）。
  - 否则查缓存；未命中调 `/agent/search-name(agent_name)` -> `Option<agent_id>`；Some 则缓存并返回，None 回退 `"0"`。
- 所有原用 `key.agent_id`（现 `key.agent_name`）做 memory-store/ego 的调用点（load_ego_info / read_history / read_memory_struct_index / list_events / push_channel_record / ThinkRequest 等）改用 `resolve_agent_id(key.agent_name)` 的结果。
- 会话定位/比较（prune_sessions、resolve_send_channel、session_key 相等性）改用 `agent_name` 比较。
- 解析时机：首次需要 agent_id 时触发（incoming_message 推 record、send_reply 推 record、ensure_session 建会话等），结果按 agent_name 缓存（命中缓存无 HTTP）；`/agent` 改绑定后 relocate 触发新 agent_name 的解析。
- 保留 agent 判定：`session.key.agent_name == RESERVED_AGENT_NAME`（""）用 default_system_prompt，否则 `load_ego_info(resolve_agent_id(...))`。

### 5.4 load_ego_info

- `agent_name == ""`（保留）-> default_system_prompt（不调 memory-ego）。
- 否则 -> resolve agent_id -> `/agent/get(agent_id)` 取 metadata -> 系统提示词（individual_name + description）。不再用 `/agent/list`。

## 6. 第 5 节 · memory-store role 目录格式

- `kissbot-memory/data.rs` `ensure_year_role_dir`：**移除空值特判**，始终 `format!("{}-{}", year, role_name)`（year 在前）。
- 空 role_name -> `2026-`；事件模式（memory_role="-event1"）-> `2026--event1`；非空 -> `2026-admin`（不变）。
- 注：`role_name` 参数实为 `memory_role()` 输出（Role 模式=role_name，Event 模式=`{role_name}-{event}`）。

## 7. 第 6 节 · 受影响面与清理

### 7.1 crates

- **kissbot-api**：channel.rs（IncomingMessage/OutgoingMessageResponse/GroupChangeNotification/UserRemoveNotification）、memory.rs（ChannelRequest/ChannelRecord 加 name）、ego.rs（Role/RoleRelation 加 full_name + 代号校验相关请求结构）。
- **kissbot-memory**：data.rs（ensure_year_role_dir 格式、ChannelParser 透传 name）、index.rs（测试数据更新）。
- **kissbot-memory-store**：record.rs（测试更新 name 字段与新目录格式）。
- **kissbot-memory-ego**：agent.rs（代号校验）、role_play.rs（full_name、代号校验）、search.rs（name_index HashMap、search_by_name Option、role_name_descr_index+RoleSearchMetadata 加 full_name）、api.rs（路由：/agent/search-name 返回 Option、新增 /role/update-full-name）、individual_recognition.rs（代号校验）。
- **kissbot-channel**：data.rs（group_change 透传 name、去 is_self）、memory_store_client.rs（去 is_self、加 name，保编译）。
- **kissbot-channel-web**：messenger.rs（填 name、去 is_self 逻辑）。
- **kissbot-channel-client(-cli)**：terminal.rs（trait 不变）、channel_client.rs（不变）、main.rs/mock.rs（测试构造更新）。
- **kissbot-agent**：coordinator.rs（ChannelContext、msg_id 匹配、resolve_agent_id、去 is_self echo）、config_manager.rs（agent_name、移除 init_agent_id/init_role/default_agent_id/default_role）、command_router.rs（/agent /role 默认 ""）、session_manager.rs（移除 sent_contents/is_self_echo/record_sent_content）、memory_store_client.rs（ChannelRecord 加 name）、types.rs（SessionKey.agent_name、RESERVED_*、session_key_of 去 Option）、main.rs（去 "Agent ID:" 日志）。

### 7.2 配置与模板

- `config.json` / `script/config.json` / `test/workspace-template/config.json`：agent 段去 `init_agent_id` / `init_role`，保留 data_dir/mgmt_*/ws_*/default_system_prompt/init_model。
- `script/template/nexus.json` / `test/workspace-template/agent-data/nexus.json`：去 `default_agent_id` / `default_role`；channel 条目 `agent_id` -> `agent_name`。

### 7.3 数据清理

- `workspace/` 与 `test/workspace/` 清除，从模板重建，**不写迁移逻辑**。

### 7.4 文档

- `docs/spec/channel-message.md`（去 is_self、加 name）、`memory-ego.md`（full_name、search 变更）、`memory-store.md`（role 目录格式）、`memory-index.md`、`kissbot-agent-nexus.md`（agent_name 绑定）及对应 components-design 文档同步更新。

### 7.5 测试

- 各 crate 单测：record.rs/data.rs/index.rs（路径与 name 字段断言）、search.rs（search_by_name Option、name_index HashMap、role_name_descr_index full_name）、ego.rs（Role/RoleRelation full_name serde）、coordinator（ChannelContext msg_id 匹配、resolve_agent_id）、config_manager（agent_name、移除字段）、session_manager（去 echo）。测试模板初始 channel 改为保留 agent（agent_name=""），`/agent` 流程不变。

## 8. 风险与边界

- **agent_name 改名**：session_key 用 agent_name，改名后会话重定位（新 session）；memory-store 以 agent_id（UUID）为键，历史不断链。ego 的 individual_name 改名后，agent_name 缓存按新值重新解析。
- **individual_name 重复**：不校验，name_index HashMap 覆盖（last wins），解析返回最后写入者。并发 create 同名 best-effort（无额外锁）。
- **msg_id 匹配竞态**：send_reply 先入 pending 再 push record；回显经 WS 异步分发，晚于 response 返回，pending 已就位。TTL 兜底处理未回显的 msg_id。
- **ChannelContext TTL**：默认 60s，可配；超时未匹配的 pending 条目懒清理，不影响正确性（最坏情况下一条本应跳过的回显被当作 is_self=0 存入，但内容为自身发出，概率极低且 TTL 内基本能匹配）。
- **未启用代码**：kissbot-channel/memory_store_client.rs 未启用但需保编译，随本次去 is_self/加 name 同步改动。
