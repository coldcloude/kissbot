# 自我认知模块

> 对应设计文档：`docs/design/components-design/kissbot-memory-ego.md`

## 读写方式

- 使用缓存降低 IO 开销
- 使用锁防止数据竞争

## 搜索索引

- 使用倒排索引实现全文搜索
- 脏标记机制延迟更新搜索索引
- 启动时自动加载所有 agent 到搜索索引

### Agent 搜索

- **name_index**：`individual_name -> agent_id` 全匹配（HashMap 实现），`search_by_name` 返回 `Option<String>`（最多一个）；兼作 agent 按 individual_name 解析 agent_id 的入口
- **name_completion**：individual_name 前缀补全（保留）
- **name_descr_index**：`[individual_name, description]` 子串搜索

### Role 搜索

- **role_name_index**：role_name 子串搜索（不变）
- **role_name_completion**：role_name 前缀补全（不变）
- **role_name_descr_index**：`[role_name, full_name, description]` 子串搜索（full_name 为 Role 的展示文本字段）

## 字段与代号

- **Role**：`role_name`（代号）、`full_name`（展示文本，可填）、`description`（文本）
- **RoleRelation**：`relation`（关系类型，自由文本）、`full_name`（展示文本，可填）、`description`（文本）
- **AgentMetadata / OtherRole / IndividualRecognition**：不加 full_name（客观信息）
- **代号限制**：`role_name`、`individual_name` 仅允许字母、数字、下划线（`^[A-Za-z0-9_]+$`），在写入入口强制校验（非空）；individual_name 不校验唯一性（重复时全匹配索引直接覆盖）

