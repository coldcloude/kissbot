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
    └── ego.rs          # ego 模块相关 API
```

## 核心设计理念

### 数据结构一致性检查
通过 trait 来确保两种数据结构在编译时一致：
- **泛型类型**：使用 `XxxGeneric` 命名，定义数据结构的字段和类型约束
- **trait（类型约束）**：使用 `XxxKind` 命名，定义数据结构的字段和类型约束
- **内部类型**：使用 `Xxx` 命名，内部模块使用，包含 `Arc`、`DashMap`、`DashSet` 优化。内部类型由各实现模块内部定义
- **内部类型约束**：使用 `SyncXxx` 明明，实现 `XxxKind`，用于内部模块的类型检查。内部类型约束由各实现模块内部定义
- **API 类型**：使用 `XxxEntity` 命名，与内部类型结构完全一致但使用标准类型
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

#### 输入请求结构体（Request）
用于客户端发送请求：
- Agent 管理相关请求
- 用户识别信息管理相关请求
- 角色设定管理相关请求

#### 数据结构和类型
通过泛型 trait 实现，包含两种类型别名：
- `XxxGeneric`：定义数据结构的字段和类型约束
- `XxxEntity`（API 类型）：使用 `LocalString`、`LocalMap`、`LocalSet`
- `XxxKind`：对应的 trait（类型约束）
- `LocalXxx`：API 类型约束（`String`、`HashMap`、`HashSet`）

## memory-ego API 详细说明

### Agent 元数据 API
| API 路径 | 方法 | 说明 | 输入 | 输出 |
|---------|------|------|------|------|
| /agent/create | POST | 创建新 Agent | `CreateAgentRequest` | `ApiResponse<()>` |
| /agent/list | GET | 列出所有 Agent | 无 | `ApiResponse<Vec<AgentMetadata>>` |
| /agent/get | POST | 获取 Agent 详情 | `GetAgentRequest` | `ApiResponse<AgentMetadata>` |
| /agent/update-name | PUT | 更新 Agent 名称 | `UpdateAgentNameRequest` | `ApiResponse<()>` |
| /agent/update-description | PUT | 更新 Agent 描述 | `UpdateAgentDescriptionRequest` | `ApiResponse<()>` |
| /agent/copy | POST | 复制 Agent | `CopyAgentRequest` | `ApiResponse<()>` |
| /agent/search-name | POST | 按名称搜索 Agent | `SearchRequest` | `ApiResponse<Vec<AgentMetadata>>` |
| /agent/search-description | POST | 按描述搜索 Agent | `SearchRequest` | `ApiResponse<Vec<AgentMetadata>>` |

### 用户识别信息 API
| API 路径 | 方法 | 说明 | 输入 | 输出 |
|---------|------|------|------|------|
| /user/get-all | POST | 获取所有用户信息 | `GetUsersRequest` | `ApiResponse<UserRecognition>` |
| /user/get | POST | 获取单个用户信息 | `GetUserRequest` | `ApiResponse<User>` |
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
| /role/get | POST | 获取角色详情 | `GetRoleRequest` | `ApiResponse<RolePlay>` |
| /role/create | POST | 创建新角色 | `CreateRoleRequest` | `ApiResponse<()>` |
| /role/create-from | POST | 从现有角色创建 | `CreateRoleFromRequest` | `ApiResponse<()>` |
| /role/remove | DELETE | 删除角色 | `RemoveRoleRequest` | `ApiResponse<()>` |
| /role/rename | PUT | 重命名角色 | `RenameRoleRequest` | `ApiResponse<()>` |
| /role/update-description | PUT | 更新角色描述 | `UpdateRoleDescriptionRequest` | `ApiResponse<()>` |
| /role/other/get | POST | 获取其他角色信息 | `GetOtherRoleRequest` | `ApiResponse<OtherRole>` |
| /role/other/replace | PUT | 替换其他角色列表 | `ReplaceOtherRolesRequest` | `ApiResponse<()>` |
| /role/other/rename | PUT | 重命名其他角色 | `RenameOtherRoleRequest` | `ApiResponse<()>` |
| /role/other/update-user-name | PUT | 更新其他角色关联的用户名 | `UpdateOtherRoleUserNameRequest` | `ApiResponse<()>` |
| /role/other/update-description | PUT | 更新其他角色描述 | `UpdateOtherRoleDescriptionRequest` | `ApiResponse<()>` |
| /role/other/update-relation | PUT | 更新其他角色与当前角色的关系 | `UpdateOtherRoleRelationRequest` | `ApiResponse<()>` |
| /role/other/replace-relations | PUT | 替换其他角色与其他角色的关系 | `ReplaceOtherRoleRelationsRequest` | `ApiResponse<()>` |

## 未来扩展
随着项目的发展，kissbot-api 模块可以进一步扩展：
- 添加 kissbot-memory-store 模块的 API 定义
- 添加 kissbot-memory-struct 模块的 API 定义
- 添加 kissbot-agent 模块的 API 定义
- 添加 kissbot-channel 模块的 API 定义
