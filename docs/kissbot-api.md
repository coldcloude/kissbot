# kissbot-api 模块设计文档

## 概述
kissbot-api 是 kissbot 项目的核心 API 定义模块，负责：
- 定义各模块间通信的标准数据结构
- 提供统一的 API 输入（Request）和输出（Response）类型
- 确保模块间数据交换的一致性和兼容性

## 模块架构
```
kissbot-api/
├── Cargo.toml
└── src/
    ├── lib.rs          # 模块入口，导出公共 API
    ├── kinds.rs        # 基础 trait 定义
    ├── common.rs       # 通用类型定义
    ├── ego.rs          # ego 模块相关 API
    └── store.rs        # store 模块相关 API
```

## 核心设计理念

### 数据结构一致性检查
通过 trait 来确保两种数据结构在编译时一致：
- **泛型类型**：使用 `XxxGeneric` 命名，定义数据结构的字段和类型约束
- **trait（类型约束）**：使用 `XxxKind` 命名，定义数据结构的字段和类型约束
- **内部类型**：使用 `Xxx` 或 `XxxResult` 命名，内部模块使用，包含 `Arc`、`DashMap`、`DashSet` 优化。内部类型由各实现模块内部定义
- **内部类型约束**：使用 `SyncXxx` 命名，实现 `XxxKind`，用于内部模块的类型检查。内部类型约束由各实现模块内部定义
- **API 类型**：使用 `XxxDTO` 命名，与内部类型结构完全一致但使用标准类型
- **API 类型约束**：使用 `LocalXxx` 明明，实现 `XxxKind`，用于 API 模块的类型检查

### 直接序列化方案
`Arc`、`DashMap`、`DashSet` 都可以被 `serde` 直接序列化和反序列化，因此：
- API 直接返回内部类型，无需转换
- 零复制开销，性能最优
- 保持数据结构一致性检查

## 基础 Trait 定义

### StringKind - 字符串类型抽象
```rust
trait StringKind {
    type Type: Clone + Sized;
}
```
- `SyncString`：使用 `Arc<String>`
- `LocalString`：使用 `String`

### MapKind - 映射类型抽象
```rust
trait MapKind {
    type Map<K, V>: where K: Eq + Hash;
}
```
- `SyncMap`：使用 `DashMap<K, V>`
- `LocalMap`：使用 `HashMap<K, V>`

### SetKind - 集合类型抽象
```rust
trait SetKind {
    type Set<T>: where T: Eq + Hash;
}
```
- `SyncSet`：使用 `DashSet<T>`
- `LocalSet`：使用 `HashSet<T>`

## 模块分类

### kinds 模块
包含所有基础 trait 定义：
- `StringKind`、`SyncString`、`LocalString`
- `MapKind`、`SyncMap`、`LocalMap`
- `SetKind`、`SyncSet`、`LocalSet`

### common 模块
包含通用的 API 相关类型：
- `ApiResponse<T>`: 统一的 API 响应结构

### ego 模块
包含 kissbot-memory-ego 模块的所有 API 定义，分为：
- Agent 管理相关请求
- 用户识别信息管理相关请求
- 角色设定管理相关请求
- 数据结构和类型：通过泛型 trait 实现，包含 `XxxDTO`（API 类型）

### store 模块
包含 kissbot-memory-store 模块的所有 API 定义，分为：
- 输入请求结构体（Request）：用于客户端发送请求
- 查询请求结构体（Query Request）：用于查询记忆
- 查询响应结构体（Query Response）：用于返回查询结果
- 数据结构和类型：通过泛型 trait 实现，包含 `XxxDTO`（API 类型）

## memory-ego API 详细说明

### Agent 元数据 API
| API 路径 | 方法 | 说明 | 输入 | 输出 |
|---------|------|------|------|------|
| /agent/create | POST | 创建新 agent | `CreateAgentRequest` | `ApiResponse<()>` |
| /agent/list | GET | 列出所有 agent | 无 | `ApiResponse<Vec<AgentMetadataGeneric>>` |
| /agent/get | POST | 获取 agent 详情 | `GetAgentRequest` | `ApiResponse<AgentMetadataGeneric>` |
| /agent/update-name | PUT | 更新 agent 名称 | `UpdateAgentNameRequest` | `ApiResponse<()>` |
| /agent/update-description | PUT | 更新 agent 描述 | `UpdateAgentDescriptionRequest` | `ApiResponse<()>` |
| /agent/copy | POST | 复制 agent | `CopyAgentRequest` | `ApiResponse<()>` |
| /agent/search-name | POST | 按名称搜索 agent | `SearchRequest` | `ApiResponse<Vec<String>>` |
| /agent/search-description | POST | 按描述搜索 agent | `SearchRequest` | `ApiResponse<Vec<String>>` |
| /agent/retrieve | POST | 按 ID 列表检索 agent | `RetrieveAgentsRequest` | `ApiResponse<Vec<AgentMetadataGeneric>>` |
| /agent/name-completion | POST | agent 名称补全 | `NameCompletionRequest` | `ApiResponse<Vec<CompletionResult<String>>>` |

### 用户识别信息 API
| API 路径 | 方法 | 说明 | 输入 | 输出 |
|---------|------|------|------|------|
| /user/get-all | POST | 获取所有用户信息 | `GetUsersRequest` | `ApiResponse<UserRecognitionGeneric>` |
| /user/get | POST | 获取单个用户信息 | `GetUserRequest` | `ApiResponse<UserGeneric>` |
| /user/replace | PUT | 替换用户列表 | `ReplaceUsersRequest` | `ApiResponse<()>` |
| /user/rename | PUT | 重命名用户 | `RenameUserRequest` | `ApiResponse<()>` |
| /user/update-privilege | PUT | 更新用户权限 | `UpdateUserPrivilegeRequest` | `ApiResponse<()>` |
| /user/update-description | PUT | 更新用户描述 | `UpdateUserDescriptionRequest` | `ApiResponse<()>` |
| /user/replace-identifiers | PUT | 替换用户标识 | `ReplaceUserIdentifiersRequest` | `ApiResponse<()>` |
| /user/replace-relations | PUT | 替换用户关系 | `ReplaceUserRelationsRequest` | `ApiResponse<()>` |

### 角色设定 API
| API 路径 | 方法 | 说明 | 输入 | 输出 |
|---------|------|------|------|------|
| /role/list | POST | 列出所有角色 | `ListRolesRequest` | `ApiResponse<Vec<String>>` |
| /role/get | POST | 获取角色详情 | `GetRoleRequest` | `ApiResponse<RolePlayGeneric>` |
| /role/create | POST | 创建新角色 | `CreateRoleRequest` | `ApiResponse<()>` |
| /role/create-from | POST | 从现有角色创建 | `CreateRoleFromRequest` | `ApiResponse<()>` |
| /role/remove | DELETE | 删除角色 | `RemoveRoleRequest` | `ApiResponse<()>` |
| /role/rename | PUT | 重命名角色 | `RenameRoleRequest` | `ApiResponse<()>` |
| /role/update-description | PUT | 更新角色描述 | `UpdateRoleDescriptionRequest` | `ApiResponse<()>` |
| /role/search-name | POST | 按名称搜索角色 | `SearchRoleRequest` | `ApiResponse<Vec<RoleKey>>` |
| /role/search-description | POST | 按描述搜索角色 | `SearchRoleRequest` | `ApiResponse<Vec<RoleKey>>` |
| /role/retrieve | POST | 按 key 列表检索角色 | `RetrieveRolesRequest` | `ApiResponse<Vec<RoleGeneric>>` |
| /role/name-completion | POST | 角色名称补全 | `RoleNameCompletionRequest` | `ApiResponse<Vec<CompletionResult<RoleKey>>>` |
| /role/other/get | POST | 获取其他角色详情 | `GetOtherRoleRequest` | `ApiResponse<OtherRoleGeneric>` |
| /role/other/replace | PUT | 替换其他角色 | `ReplaceOtherRolesRequest` | `ApiResponse<()>` |
| /role/other/rename | PUT | 重命名其他角色 | `RenameOtherRoleRequest` | `ApiResponse<()>` |
| /role/other/update-user-name | PUT | 更新其他角色关联的用户名 | `UpdateOtherRoleUserNameRequest` | `ApiResponse<()>` |
| /role/other/update-description | PUT | 更新其他角色描述 | `UpdateOtherRoleDescriptionRequest` | `ApiResponse<()>` |
| /role/other/update-relation | PUT | 更新其他角色关系 | `UpdateOtherRoleRelationRequest` | `ApiResponse<()>` |
| /role/other/replace-relations | PUT | 替换其他角色关系列表 | `ReplaceOtherRoleRelationsRequest` | `ApiResponse<()>` |

## memory-ego API 数据结构和格式说明

### Agent 管理请求结构

#### CreateAgentRequest
```json
{
  "name": "agent-name",
  "description": "agent-description"
}
```

#### GetAgentRequest
```json
{
  "agent_id": "agent-id"
}
```

#### UpdateAgentNameRequest
```json
{
  "agent_id": "agent-id",
  "name": "new-agent-name"
}
```

#### UpdateAgentDescriptionRequest
```json
{
  "agent_id": "agent-id",
  "description": "new-agent-description"
}
```

#### CopyAgentRequest
```json
{
  "agent_id": "agent-id"
}
```

#### SearchRequest
```json
{
  "keyword": "search-keyword"
}
```

#### RetrieveAgentsRequest
```json
{
  "agent_ids": ["agent-id-1", "agent-id-2"]
}
```

#### NameCompletionRequest
```json
{
  "prefix": "name-prefix"
}
```

### 用户识别信息请求结构

#### GetUsersRequest
```json
{
  "agent_id": "agent-id"
}
```

#### GetUserRequest
```json
{
  "agent_id": "agent-id",
  "user_name": "user-name"
}
```

#### ReplaceUsersRequest
```json
{
  "agent_id": "agent-id",
  "remove_user_names": ["user-name-1", "user-name-2"],
  "insert_users": {
    "user-name": {
      "privilege": "Owner",
      "identifiers": [
        {
          "channel_id": "channel-id",
          "user_id": "user-id"
        }
      ],
      "relations": {
        "other-user": {
          "relation": "friend",
          "description": "good friend"
        }
      },
      "description": "user-description"
    }
  }
}
```

#### RenameUserRequest
```json
{
  "agent_id": "agent-id",
  "user_name": "old-user-name",
  "new_name": "new-user-name"
}
```

#### UpdateUserPrivilegeRequest
```json
{
  "agent_id": "agent-id",
  "user_name": "user-name",
  "privilege": "Admin"
}
```

#### UpdateUserDescriptionRequest
```json
{
  "agent_id": "agent-id",
  "user_name": "user-name",
  "description": "new-description"
}
```

#### ReplaceUserIdentifiersRequest
```json
{
  "agent_id": "agent-id",
  "user_name": "user-name",
  "remove_identifiers": [
    {
      "channel_id": "old-channel-id",
      "user_id": "old-user-id"
    }
  ],
  "insert_identifiers": [
    {
      "channel_id": "new-channel-id",
      "user_id": "new-user-id"
    }
  ]
}
```

#### ReplaceUserRelationsRequest
```json
{
  "agent_id": "agent-id",
  "user_name": "user-name",
  "remove_relations": ["relation-1", "relation-2"],
  "insert_relations": {
    "other-user": {
      "relation": "new-relation",
      "description": "new-description"
    }
  }
}
```

### 角色设定请求结构

#### ListRolesRequest
```json
{
  "agent_id": "agent-id"
}
```

#### GetRoleRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name"
}
```

#### CreateRoleRequest
```json
{
  "agent_id": "agent-id",
  "name": "role-name",
  "description": "role-description"
}
```

#### CreateRoleFromRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "existing-role-name",
  "new_name": "new-role-name"
}
```

#### RemoveRoleRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name"
}
```

#### RenameRoleRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "old-role-name",
  "new_name": "new-role-name"
}
```

#### UpdateRoleDescriptionRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name",
  "description": "new-description"
}
```

#### SearchRoleRequest
```json
{
  "agent_id": "agent-id",
  "keyword": "search-keyword"
}
```

#### RetrieveRolesRequest
```json
{
  "role_keys": [
    {
      "id": "agent-id",
      "name": "role-name"
    }
  ]
}
```

#### RoleNameCompletionRequest
```json
{
  "agent_id": "agent-id",
  "prefix": "name-prefix"
}
```

#### GetOtherRoleRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name",
  "other_role_name": "other-role-name"
}
```

#### ReplaceOtherRolesRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name",
  "remove_other_roles": ["other-role-1", "other-role-2"],
  "insert_other_roles": {
    "other-role-name": {
      "user_name": "user-name",
      "role_relation": {
        "relation": "assistant",
        "description": "helpful assistant"
      },
      "other_role_relations": {
        "another-role": {
          "relation": "colleague",
          "description": "work together"
        }
      },
      "description": "other-role-description"
    }
  }
}
```

#### RenameOtherRoleRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name",
  "other_role_name": "old-other-role-name",
  "new_name": "new-other-role-name"
}
```

#### UpdateOtherRoleUserNameRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name",
  "other_role_name": "other-role-name",
  "new_user_name": "new-user-name"
}
```

#### UpdateOtherRoleDescriptionRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name",
  "other_role_name": "other-role-name",
  "new_description": "new-description"
}
```

#### UpdateOtherRoleRelationRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name",
  "other_role_name": "other-role-name",
  "new_relation": {
    "relation": "new-relation",
    "description": "new-description"
  }
}
```

#### ReplaceOtherRoleRelationsRequest
```json
{
  "agent_id": "agent-id",
  "role_name": "role-name",
  "other_role_name": "other-role-name",
  "remove_relations": ["relation-1", "relation-2"],
  "insert_relations": {
    "another-role": {
      "relation": "new-relation",
      "description": "new-description"
    }
  }
}
```

### 实体响应结构

#### AgentMetadataGeneric
```json
{
  "id": "agent-id",
  "name": "agent-name",
  "description": "agent-description",
  "created_at": "2024-01-01 12:00:00"
}
```

#### UserRecognitionGeneric
```json
{
  "id": "user-recognition-id",
  "user_map": {
    "user-name": {
      "privilege": "Owner",
      "identifiers": [
        {
          "channel_id": "channel-id",
          "user_id": "user-id"
        }
      ],
      "relations": {
        "other-user": {
          "relation": "friend",
          "description": "good friend"
        }
      },
      "description": "user-description"
    }
  }
}
```

#### RolePlayGeneric
```json
{
  "role": {
    "id": "role-id",
    "name": "role-name",
    "description": "role-description"
  },
  "other_roles": {
    "other-role-name": {
      "user_name": "user-name",
      "role_relation": {
        "relation": "assistant",
        "description": "helpful assistant"
      },
      "other_role_relations": {
        "another-role": {
          "relation": "colleague",
          "description": "work together"
        }
      },
      "description": "other-role-description"
    }
  }
}
```

## memory-store API 详细说明

### 记忆推送 API
| API 路径 | 方法 | 说明 | 输入 | 输出 |
|---------|------|------|------|------|
| /store/channel | POST | 推送Channel文本记录 | `ChannelRequests` | `ApiResponse<()>` |
| /store/think | POST | 推送思考内容记录 | `ThinkRequests` | `ApiResponse<()>` |
| /store/tool-call | POST | 推送工具调用记录 | `ToolCallRequests` | `ApiResponse<()>` |
| /store/tool-result | POST | 推送工具调用结果记录 | `ToolResultRequests` | `ApiResponse<()>` |

### 记忆查询 API
| API 路径 | 方法 | 说明 | 输入 | 输出 |
|---------|------|------|------|------|
| /store/query/channel | POST | 查询Channel文本记录 | `QueryChannelRequest` | `ApiResponse<Vec<ChannelRecordGeneric>>` |
| /store/query/think | POST | 查询思考内容记录 | `QueryRequest` | `ApiResponse<Vec<ThinkRecordGeneric>>` |
| /store/query/tool-call | POST | 查询工具调用记录 | `QueryRequest` | `ApiResponse<Vec<ToolCallRecordGeneric>>` |
| /store/query/tool-result | POST | 查询工具调用结果记录 | `QueryRequest` | `ApiResponse<Vec<ToolResultRecordGeneric>>` |

## memory-store API 数据结构和格式说明

### 统一响应格式 ApiResponse
所有 API 响应都使用统一的格式：
```json
{
  "success": true,
  "data": null,
  "error": null
}
```

### 记忆推送 API 请求结构

#### ChannelRequests
```json
{
  "requests": [
    {
      "agent_id": "agent-001",
      "role_name": "role-001",
      "channel_id": "channel-001",
      "user_id": "user-001",
      "time": "2025-05-17 10:30:00",
      "msg_type": "text",
      "content": "你好"
    }
  ],
  "force": 0
}
```

#### ThinkRequests
```json
{
  "requests": [
    {
      "agent_id": "agent-001",
      "role_name": "role-001",
      "content": "这是一段思考内容",
      "key": "think-key-001",
      "time": "2025-05-17 10:30:01"
    }
  ],
  "force": 0
}
```

#### ToolCallRequests
```json
{
  "requests": [
    {
      "agent_id": "agent-001",
      "role_name": "role-001",
      "tool_name": "tool-name",
      "tool_params": {"key": "value"},
      "key": "tool-key-001",
      "time": "2025-05-17 10:30:02"
    }
  ],
  "force": 0
}
```

#### ToolResultRequests
```json
{
  "requests": [
    {
      "agent_id": "agent-001",
      "role_name": "role-001",
      "tool_result": {"result": "data"},
      "key": "tool-key-001",
      "time": "2025-05-17 10:30:03"
    }
  ],
  "force": 0
}
```

### 记忆查询 API 请求结构

#### QueryChannelRequest
```json
{
  "agent_id": "agent-001",
  "role_name": "role-001",
  "channel_id": "channel-001",
  "start_time": "2025-05-17 00:00:00",
  "end_time": "2025-05-17 23:59:59"
}
```

#### QueryRequest (think/tool-call/tool-result)
```json
{
  "agent_id": "agent-001",
  "role_name": "role-001",
  "start_time": "2025-05-17 00:00:00",
  "end_time": "2025-05-17 23:59:59"
}
```

### 记忆查询 API 响应结构

#### 查询Channel文本记录响应
```json
{
  "success": true,
  "data": [
    {
      "agent_id": "agent-001",
      "role_name": "role-001",
      "channel_id": "channel-001",
      "user_id": "user-001",
      "is_self": 1,
      "msg_type": "text",
      "content": "你好",
      "time": "2025-05-17 10:30:00",
      "sn": 1
    }
  ],
  "error": null
}
```

#### 查询思考内容记录响应
```json
{
  "success": true,
  "data": [
    {
      "agent_id": "agent-001",
      "role_name": "role-001",
      "content": "这是一段思考内容",
      "key": "think-key-001",
      "time": "2025-05-17 10:30:01",
      "sn": 1
    }
  ],
  "error": null
}
```

#### 查询工具调用记录响应
```json
{
  "success": true,
  "data": [
    {
      "agent_id": "agent-001",
      "role_name": "role-001",
      "tool_name": "tool-name",
      "tool_params": {"key": "value"},
      "key": "tool-key-001",
      "time": "2025-05-17 10:30:02",
      "sn": 1
    }
  ],
  "error": null
}
```

#### 查询工具调用结果记录响应
```json
{
  "success": true,
  "data": [
    {
      "agent_id": "agent-001",
      "role_name": "role-001",
      "tool_result": {"result": "data"},
      "key": "tool-key-001",
      "time": "2025-05-17 10:30:03",
      "sn": 1
    }
  ],
  "error": null
}
```

## 未来扩展
随着项目的发展，kissbot-api 模块可以进一步扩展：
- 添加 kissbot-memory-struct 模块的 API 定义
- 添加 kissbot-agent 模块的 API 定义
- 添加 kissbot-channel 模块的 API 定义
