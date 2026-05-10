# kissbot-memory-ego 模块设计

## 模块概述
自我认知模块，记忆模块的子模块，独立于agent模块，和其他记忆模块读写同一个文件系统，负责管理agent的自我认知信息。

## 职责
- 调用memory基础模块的DirectoryManager进行目录管理
- 实现agent元数据管理（AgentManager）
- 提供管理agent元数据的HTTPS API（如新增agent接口）
- 每个agent ID对应一个客观设定，对应多个角色设定
- 实现用户识别信息管理、角色设定管理，并提供HTTPS API
- 通过记忆提取的方式生成agent的自我认知信息（第二阶段）
- 仅提供API，本身不封装为tool

## 架构设计
### 核心组件
- agent元数据管理器（AgentManager）
- 自我认知信息管理器（EgoManager）
- 用户识别信息管理器（UserRecognitionManager）
- 角色设定管理器（RolePlayManager）
- HTTPS API服务器
- 记忆提取器

## 数据结构
### Agent自我认知信息
- agent ID（唯一标识）
- 客观设定
  - 身份标识（JSON结构存储）
  - 用户识别信息（JSON结构存储）
- 多个角色设定
  - 角色扮演和角色扮演关系（JSON结构存储）

### Agent元数据结构
- id：agent唯一标识符（UUID）
- name：agent名称
- description：agent描述
- created_at：创建时间（格式：yyyy-MM-dd HH:mm:ss）
- force_items：agent必须遵守的事项列表（字符串数组）
- autonomous_goals：agent自主运行目标（字符串）

## Agent元数据操作函数
- create_agent：创建新的agent，需要name和description，forbidden_items可为空列表，autonomous_goals可为空字符串
- get_metadata：获取agent元数据
- update_agent_name：修改agent名称
- update_agent_description：修改agent描述
- update_agent_name_description：同时修改agent名称和描述
- update_force_items：更新agent事项列表
- update_autonomous_goals：更新agent自主运行目标

## 结构化设计

### 数据结构设计
除身份识别信息来源于agent元数据（JSON存储）外，用户识别信息、角色扮演、角色扮演关系也采用结构化设计。

### 数据结构实现
- `Xxx`：内部类型，是 `XxxGeneric` 的实现，包含 `Arc`、`DashMap`、`DashSet` 优化
- `SyncXxx`：内部类型约束，是 `XxxKind` 的实现，包含 `SyncString`、`SyncMap`、`SyncSet` 优化

### 管理模块设计
除agent元数据管理模块外，用户识别信息、角色扮演信息、角色扮演关系信息也增加对应的管理模块，包括增加和修改功能。

### 存储机制
所有内容使用JSON文件存储

### 对外接口设计
查询数据JSON格式返回

### 用户识别信息结构
用户识别信息应为一个列表，列表每项为一个用户，至少包括：
- **名称**：用户名称（外部填写，无自动生成）
- **身份**：枚举值，只有 所有者、管理员、其他用户 3项
- **关联的用户标识**：一个列表，每项标识应由用户使用的channel标识+用户在channel中的标识组成。可以从channel中获取列表后选择，也可回退为手动填写
- **和其他用户的关系**：一个列表，每项至少包括对方用户、关系字段，可以有描述字段
- **描述字段**（可选）：可以自由填写任意文本

### 角色扮演信息结构
角色扮演信息为agent在本次会话中扮演的角色，至少包括：
- **名称**：角色名称（外部填写，无自动生成）
- **描述**：可以自由填写任意文本
- **自主运行目标**：该角色的自主运行目标（字符串，可为空字符串，可以自由填写）
- 单独存放一个JSON文件

### 角色扮演关系信息结构
角色扮演关系信息为agent在本次会话中认为用户是哪个角色，至少包括：
- **名称**：角色名称（外部填写，无自动生成）
- **关联的用户名**：关联的用户名称
- **和agent角色的关系**：和agent角色的关系
- **和其他角色的关系**：一个列表，每项是一个关系，至少包括对方角色名，关系字段
- **描述字段**（可选）：可以自由填写任意文本

## API设计

### Agent元数据管理接口

#### 新增agent接口
- **接口**：新增agent
- **输入**：agent元数据（名称、描述必填，force_items可为空列表，autonomous_goals可为空字符串）
- **返回**：成功或失败状态

#### 查询agent元数据接口
- **接口**：按agent ID查询
- **返回内容**：agent元数据（名称、描述、创建时间、force_items、autonomous_goals等）

#### 查询所有agent列表接口
- **接口**：查询所有agent
- **返回内容**：agent元数据列表（并发获取）

#### 修改agent名称接口
- **接口**：修改agent名称
- **输入**：agent ID、新名称
- **返回**：成功或失败状态

#### 修改agent描述接口
- **接口**：修改agent描述
- **输入**：agent ID、新描述
- **返回**：成功或失败状态

#### 同时修改名称和描述接口
- **接口**：同时修改agent名称和描述
- **输入**：agent ID、新名称、新描述
- **返回**：成功或失败状态

#### 按名称搜索agent接口
- **接口**：按名称搜索agent
- **输入**：搜索关键词
- **返回**：匹配的agent ID列表

#### 按描述搜索agent接口
- **接口**：按描述搜索agent
- **输入**：搜索关键词
- **返回**：匹配的agent ID列表

#### 检索agent接口
- **接口**：根据ID列表检索agent
- **输入**：agent ID列表
- **返回**：agent元数据列表

#### agent名称补全接口
- **接口**：agent名称补全
- **输入**：前缀关键词
- **返回**：补全结果列表（包含key和匹配的文本）

#### 复制agent接口
- **接口**：复制agent
- **路径**：POST /agent/:agent_id/copy
- **返回**：成功或失败状态

#### 更新agent必须遵守事项列表接口
- **接口**：更新agent必须遵守事项列表
- **路径**：PUT /agent/:agent_id/force-items
- **输入**：必须遵守的事项字符串数组
- **返回**：成功或失败状态

#### 更新agent自主运行目标接口
- **接口**：更新agent自主运行目标
- **路径**：PUT /agent/:agent_id/autonomous-goals
- **输入**：自主运行目标字符串（可为空字符串）
- **返回**：成功或失败状态

### 用户识别信息管理接口

#### 获取用户识别信息接口
- **接口**：获取agent用户识别信息
- **路径**：GET /agent/:agent_id/users
- **返回内容**：用户识别信息JSON

#### 获取单个用户信息接口
- **接口**：获取单个用户信息
- **路径**：GET /agent/:agent_id/users/:user_name
- **返回内容**：用户信息JSON

#### 更新用户识别信息接口
- **接口**：替换用户列表
- **路径**：PUT /agent/:agent_id/users
- **输入**：要删除的用户名列表和要新增的用户映射
- **返回**：成功或失败状态

#### 重命名用户接口
- **接口**：重命名用户
- **路径**：PUT /agent/:agent_id/users/:user_name/name
- **输入**：新用户名
- **返回**：成功或失败状态

#### 更新用户权限接口
- **接口**：更新用户权限
- **路径**：PUT /agent/:agent_id/users/:user_name/privilege
- **输入**：用户权限
- **返回**：成功或失败状态

#### 更新用户描述接口
- **接口**：更新用户描述
- **路径**：PUT /agent/:agent_id/users/:user_name/description
- **输入**：用户描述
- **返回**：成功或失败状态

#### 替换用户标识接口
- **接口**：替换用户标识
- **路径**：PUT /agent/:agent_id/users/:user_name/identifiers
- **输入**：要删除和新增的用户标识
- **返回**：成功或失败状态

#### 替换用户关系接口
- **接口**：替换用户关系
- **路径**：PUT /agent/:agent_id/users/:user_name/relations
- **输入**：要删除和新增的用户关系
- **返回**：成功或失败状态

### 角色设定管理接口

#### 列出所有角色接口
- **接口**：列出所有角色
- **路径**：GET /agent/:agent_id/roles
- **返回内容**：角色名称列表JSON

#### 获取角色信息接口
- **接口**：获取角色信息
- **路径**：GET /agent/:agent_id/roles/:role_name
- **返回内容**：角色信息JSON（包含autonomous_goals）

#### 创建角色接口
- **接口**：创建新角色
- **路径**：POST /agent/:agent_id/roles
- **输入**：角色名称、描述必填，autonomous_goals可为空字符串
- **返回**：成功或失败状态

#### 从现有角色创建角色接口
- **接口**：从现有角色创建新角色
- **路径**：POST /agent/:agent_id/roles/:role_name/create_from
- **输入**：新角色名
- **返回**：成功或失败状态

#### 删除角色接口
- **接口**：删除角色
- **路径**：DELETE /agent/:agent_id/roles/:role_name
- **返回**：成功或失败状态

#### 重命名角色接口
- **接口**：重命名角色
- **路径**：PUT /agent/:agent_id/roles/:role_name/name
- **输入**：新角色名
- **返回**：成功或失败状态

#### 更新角色描述接口
- **接口**：更新角色描述
- **路径**：PUT /agent/:agent_id/roles/:role_name/description
- **输入**：新描述
- **返回**：成功或失败状态

#### 更新角色自主运行目标接口
- **接口**：更新角色自主运行目标
- **路径**：PUT /agent/:agent_id/roles/:role_name/autonomous-goals
- **输入**：自主运行目标字符串（可为空字符串）
- **返回**：成功或失败状态

#### 按名称搜索角色接口
- **接口**：按名称搜索角色
- **输入**：搜索关键词、可选agent ID（可选）
- **返回**：匹配的角色key列表

#### 按描述搜索角色接口
- **接口**：按描述搜索角色
- **输入**：搜索关键词、可选agent ID（可选）
- **返回**：匹配的角色key列表

#### 检索角色接口
- **接口**：根据key列表检索角色
- **输入**：角色key列表
- **返回**：角色信息列表

#### 角色名称补全接口
- **接口**：角色名称补全
- **输入**：前缀关键词
- **返回**：补全结果列表（包含key和匹配的文本）

#### 获取单个其他角色接口
- **接口**：获取单个其他角色信息
- **路径**：GET /agent/:agent_id/roles/:role_name/other_roles/:other_role_name
- **返回内容**：其他角色信息JSON

#### 替换其他角色接口
- **接口**：替换角色中的其他角色
- **路径**：PUT /agent/:agent_id/roles/:role_name/other_roles
- **输入**：要删除和新增的其他角色
- **返回**：成功或失败状态

#### 重命名其他角色接口
- **接口**：重命名其他角色
- **路径**：PUT /agent/:agent_id/roles/:role_name/other_roles/:other_role_name/name
- **输入**：新名称
- **返回**：成功或失败状态

#### 更新其他角色用户名接口
- **接口**：更新其他角色关联的用户名
- **路径**：PUT /agent/:agent_id/roles/:role_name/other_roles/:other_role_name/user_name
- **输入**：新用户名
- **返回**：成功或失败状态

#### 更新其他角色描述接口
- **接口**：更新其他角色描述
- **路径**：PUT /agent/:agent_id/roles/:role_name/other_roles/:other_role_name/description
- **输入**：新描述
- **返回**：成功或失败状态

#### 更新其他角色关系接口
- **接口**：更新其他角色的主要关系
- **路径**：PUT /agent/:agent_id/roles/:role_name/other_roles/:other_role_name/relation
- **输入**：新关系（关系名称和描述）
- **返回**：成功或失败状态

#### 替换其他角色的关系列表接口
- **接口**：替换其他角色的关系列表
- **路径**：PUT /agent/:agent_id/roles/:role_name/other_roles/:other_role_name/relations
- **输入**：要删除和新增的关系列表
- **返回**：成功或失败状态

## 通信接口
- 输入：通过HTTPS API接收agent的查询请求
- 输出：通过HTTPS API返回agent的自我认知信息（JSON格式）
- 文件系统：读取配置文件，与其他记忆模块共享文件系统

## 文件存储目录结构
根据记忆系统的文件存储目录设计：
- **记忆系统根目录**
  - **agent ID目录**
    - **agent-{agent-id}**：agent存在标识文件（由memory模块管理）
    - **metadata.json**：agent元数据JSON文件
    - **memory-ego**：memory-ego模块的设定信息单独存放
      - **user-recognition.json**：用户识别信息JSON文件
      - **role-play-{role-id}.json**：角色设定JSON文件

### JSON文件格式示例

#### metadata.json 示例
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "name": "智能助手",
  "description": "负责处理用户查询的智能agent",
  "created_at": "2024-01-01 12:00:00",
  "force_items": [
    "不要泄露用户隐私信息",
    "不要执行危险命令",
    "不要生成违法内容"
  ],
  "autonomous_goals": "定期收集最新技术资讯，整理成报告"
}
```

#### role-play-{role-id}.json 示例
```json
{
  "name": "技术顾问",
  "description": "提供专业技术建议的角色",
  "autonomous_goals": "每日分析技术趋势，给出投资建议"
}
```

## 文件生成机制

### JSON文件来源
- 身份识别信息：通过API填写metadata.json
- 用户识别信息：通过API从外部填写user-recognition.json
- 角色设定信息：通过API从外部填写role-play-{role-id}.json

### 进阶模式
- 结合配置信息和从记忆中提取的信息，生成JSON文件
- 支持自动化生成设定内容

## AgentManager实现
- 实现agent元数据的JSON文件存储（metadata.json）
- 使用DashMap实现高并发的manager_lock
- AgentMetadata共享元数据，内部使用Arc<String>减少复制开销
- 使用tokio::sync::RwLock实现读写锁防止竞争
- 使用内存缓存，首次读取后缓存到内存
- 双重锁定机制确保数据一致性
- 单例通过关联函数获取：AgentManager::get()

## SearchManager实现
- 使用本地kai-index库的DistinctIndex实现全文搜索
- 使用DashMap和DashSet实现高并发数据结构
- 使用Arc共享数据，避免克隆
- 脏标记机制（identity_dirty）：延迟更新搜索索引
- force_sync_identity：强制同步，更新搜索索引
- sync_identity：检查脏标记，仅在需要时同步
- search_by_name/search_by_description：先同步脏数据再搜索
- 启动时自动加载：get()初始化时加载所有agent到搜索索引
- 单例通过tokio::sync::OnceCell实现异步初始化

## 实现决策
- 不作为memory-struct的子模块，不实现memory-struct的接口
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 使用serde进行JSON序列化
- 支持从配置文件加载证书
- AgentManager在本模块实现，管理agent元数据的JSON文件存储
- AgentMetadata内部使用Arc<String>减少复制开销
- 使用dashmap实现高并发数据结构
- 使用本地kai-index库实现全文搜索
- 使用futures实现并发操作
- 使用Arc共享数据，减少克隆开销
- MD文件生成功能作为备用，暂不启用

## 开发计划

### 第1阶段：基础模式实现
- [x] 调用memory基础模块库（DirectoryManager）
- [x] 实现AgentManager管理agent元数据
- [x] agent元数据管理API（新增agent、查询agent元数据、修改agent名称描述）
- [x] HTTPS API接口

### 第2阶段：全文搜索实现
- [x] 使用本地kai-index库实现全文搜索
- [x] 使用dashmap实现高并发数据结构
- [x] 实现脏标记机制延迟更新
- [x] 实现name和description字段的全文搜索
- [x] 添加搜索API接口
- [x] 启动时自动加载所有agent到索引
- [x] 身份标识MD文件生成（备用）

### 第3阶段：用户识别信息、角色扮演、角色扮演关系管理
- [x] 用户识别信息、角色扮演、角色扮演关系的JSON管理模块
- [x] 用户识别信息、角色扮演、角色扮演关系的API接口
- [x] 用户识别信息、角色扮演、角色扮演关系MD文件生成（备用）
- [x] JSON查询API

### 第4阶段：进阶模式改造
- [ ] 记忆提取器实现
- [ ] 配置信息与记忆提取结合生成客观设定、角色设定JSON文件
- [ ] 进阶模式API
