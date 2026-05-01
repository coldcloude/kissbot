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
    ├── common.rs       # 通用类型定义
    └── ego.rs          # ego 模块相关 API
```

## 模块分类

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

#### 输出响应结构体（Response）
用于服务端返回数据：
- 简化版的数据结构，去除 Arc、DashMap 等优化
- 使用 HashMap 替代 DashMap，便于序列化和跨语言兼容

## 设计原则

### 数据结构分离原则
1. **内部优化结构**：各模块内部使用优化的数据结构，可能包含：
   - `Arc<T>`：用于共享所有权，减少克隆开销
   - `DashMap`：并发安全的哈希表，支持多线程访问
   - `DashSet`：并发安全的集合

2. **API 通信结构**：API 层使用标准化的简化结构：
   - 不使用 `Arc<T>`，直接使用值类型
   - 使用 `HashMap` 替代 `DashMap`
   - 使用 `Vec` 替代 `DashSet`
   - 确保可跨语言序列化和反序列化

### 类型转换职责
- 各模块内部负责在「优化结构」和「API 结构」之间转换
- kissbot-api 模块只提供标准的 API 数据结构，不负责转换逻辑

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
