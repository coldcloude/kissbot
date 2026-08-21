# memory-ego：agent_id 手工指定 + 去除 individual_name + 精简搜索索引

## 背景与目标

当前 memory-ego 中 agent_id 由系统内部生成（UUID），agent 的"易读名称"由独立的 `individual_name` 字段承担，两者职责重叠。目标：

1. agent_id 改为调用方在 create 时手工指定的易读标识符（如 `alice`、`main`）
2. 删除 AgentMetadata 的 `individual_name` 字段（其职责由 agent_id 承接）
3. 精简搜索索引：删除 `name_index`（agent individual_name 精确索引）和 `role_name_index`（role_name 子串索引）
4. `SearchMetadata` 中的 individual_name 替换为 agent_id
5. 同步精简相关 API 与文档

## 数据模型变更（kissbot-api/src/ego.rs）

- `AgentMetadata`：删除 `individual_name` 字段 → `{ agent_id, description, created_at }`
- `CreateAgentRequest`：`{ individual_name, description }` → `{ agent_id, description }`
- `CopyAgentRequest`：`{ agent_id }` → `{ agent_id, new_agent_id }`
- 删除请求结构：`UpdateAgentNameRequest`、`RemoveRoleRequest`、`RenameRoleRequest`
- 同步更新 ego.rs 中对应的 serde 测试

## memory-ego 内部变更

### agent.rs

- `create_agent(agent_id, description)`：
  - 对 agent_id 执行 `validate_code`（`^[A-Za-z0-9_]+$`、非空，与 role_name 同规则）
  - 查重：agent 目录下 metadata.json 已存在 → 返回新错误 `AgentAlreadyExists`（不覆盖已有数据）
  - 不再生成 UUID
- 删除 `update_agent_name`（agent 创建后不可变，要换标识需重新创建）
- `copy_agent(agent_id, new_agent_id)`：校验 new_agent_id + 查重，复制 description 到新 agent
- 同步更新 agent.rs 测试

### role_play.rs

- 删除 `remove_role`、`rename_role`（role_name 创建后不可更改）
- 保留：`create_role`、`create_role_from`、`update_role_description`、`update_role_full_name` 及角色间关系（other_roles）的全部修改接口
- 同步删除对应测试

### error.rs / code.rs

- error.rs 新增 `AgentAlreadyExists` 变体
- code.rs：`validate_code` 注释更新（适用范围改为 agent_id / role_name 等代号字段）

## 搜索变更（search.rs）

- 删除 `name_index`（HashMap 精确索引）及其全部维护逻辑，删除 `search_by_name`
- 删除 `role_name_index` 及 `search_role_by_name`
- `SearchMetadata::new`：value 从 `[individual_name, description]` 改为 `[agent_id, description]`；同步简化 `force_sync_identity` 中基于旧值的新旧对比逻辑
- `name_completion`：索引对象从 individual_name 改为 agent_id（前缀补全保留）
- `role_name_completion`：不变（role_name 仍为 Role 字段）
- 同步更新 search.rs 测试

## API 变更（api.rs）

删除路由：

- `/agent/search-name`（精确查找走 `/agent/get`）
- `/agent/update-name`
- `/role/search-name`（按 role_name 搜索统一走 `/role/search-description`，其索引 `role_name_descr_index` 含 role_name）
- `/role/remove`
- `/role/rename`

更新 handler：

- `create_agent`：读取 `req.agent_id`、`req.description`
- `copy_agent`：读取 `req.agent_id`、`req.new_agent_id`
- 删除 `search_name_http_success` 测试

## 消费方变更

- `kissbot-agent/src/ego_md.rs`：`build_ego_identity_md` 中 `metadata.individual_name` 改为 `metadata.agent_id`
- 测试用例：
  - `test/tests/memory-ego-api.spec.ts`：create 请求改传 `agent_id`；删除 search-name / update-name / role rename / role remove 相关断言
  - `test/tests/agent-commands.spec.ts`：create 请求改传 `agent_id`
  - `test/tests/nexus-ego-chat-store.spec.ts`：create 请求改传 `agent_id`
- 存量 metadata.json 中的 `individual_name` 字段：serde 默认忽略未知字段，旧数据可正常读取，无需迁移

## 文档变更

- `docs/spec/memory-ego.md`：更新 Agent 搜索索引说明（删除 name_index 描述，SearchMetadata 改为 [agent_id, description]）、字段与代号说明（AgentMetadata 不再有 individual_name）
- `docs/design/components-design/kissbot-memory-ego.md`：更新核心功能与内部模块描述（手工指定 agent_id、删除 name_index/role_name_index、API 清单）
- `docs/plan/components-plan/kissbot-memory-ego.md`：如有相关条目则同步更新

## 验收标准

1. `cargo build` / `cargo test` 通过（kissbot-api、kissbot-memory-ego、kissbot-agent 相关 crate）
2. create 传 `agent_id` 成功创建；重复 agent_id 返回 AgentAlreadyExists 错误
3. `/agent/search-name`、`/agent/update-name`、`/role/search-name`、`/role/remove`、`/role/rename` 路由不再存在
4. `/agent/search-description` 可命中 agent_id 或 description；`/agent/name-completion` 按 agent_id 前缀补全
5. 自动化测试（playwright）通过
