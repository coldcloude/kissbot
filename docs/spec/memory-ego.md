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

- **name_completion**：agent_id 前缀补全（保留）
- **name_descr_index**：`[agent_id, description]` 子串搜索

### Role 搜索

- **role_name_completion**：role_name 前缀补全（不变）
- **role_name_descr_index**：`[role_name, full_name, description]` 子串搜索（full_name 为 Role 的展示文本字段）；按 role_name 搜索统一走该全文索引

## 字段与代号

- **Role**：`role_name`（代号）、`full_name`（展示文本，可填）、`description`（文本）
- **RoleRelation**：`relation`（关系类型，自由文本）、`full_name`（展示文本，可填）、`description`（文本）
- **AgentMetadata / OtherRole / IndividualRecognition**：不加 full_name（客观信息）
- **代号限制**：`agent_id`、`role_name` 仅允许字母、数字、下划线（`^[A-Za-z0-9_]+$`），在写入入口强制校验（非空）；agent_id 在 create 时手工指定且创建后不可变，重复创建返回 AgentAlreadyExists

