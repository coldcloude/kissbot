# kissbot-memory-ego 模块设计

## 模块概述
- 自我认知模块，记忆模块的子模块，负责管理agent的自我认知信息，独立于agent模块，和其他记忆模块读写同一个文件系统
- 该模块通过配置文件读取，通过API配置或者通过记忆提取的方式，生成并保存agent的自我认知信息

## Agent自我认知设计

### 概述
Agent的自我认知是记忆的一部分，应具有客观、角色双重设定：
#### 一、客观设定
- 每个agent ID对应一个客观设定
- 包括agent自身的客观状态
- 包括agent和用户的客观关系
#### 二、角色设定
- 每个agent ID对应多个角色设定
- 包括agent在本次会话中扮演的角色
- 包括agent在本次会话中认为客户扮演的角色

### 具体内容
agent自我认知信息包括以下几个部分：
1. **身份标识**：关于agent身份的客观事实，比如名称、创建时间
2. **用户识别信息**：agent和各个用户的客观关系，各个用户间的客观关系
3. **角色扮演**：本次会话中对外展现的角色，包括agent对自己的称呼，用户可能对agent的各种称呼，对话采用的语气、用词风格
4. **角色扮演关系**：各个用户在本次对话中的角色，agent在本次对话中对用户角色的称呼

### 面向的场景
agent设计面向多用户环境，具体要求：
- agent需要认识每一个用户，并区分哪个用户是管理员
- agent在不同的会话中要扮演不同的角色，但这不影响agent客观上是谁
- agent在会话中不体现自身的客观信息（要求不扮演的情况除外）
- 在角色扮演的过程中，agent还需要将不同的用户也识别成不同的角色
- 用户按照名称识别，可以关联不同的channelID+userID
- 角色和用户角色也按照名称识别，用户角色可以直接关联到channelID+userID，也可以关联到用户名称，再关联推断

## 职责
- 调用memory基础模块的DirectoryManager进行目录管理
- 实现agent元数据管理（AgentManager）
- 提供管理agent元数据的HTTPS API（如新增agent接口）
- 实现用户识别信息管理、角色设定管理，并提供HTTPS API
- 通过记忆提取的方式生成agent的自我认知信息（第二阶段）
- 仅提供API，本身不封装为tool

## 架构设计
### 核心组件
- agent元数据管理器（AgentManager）
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

## 结构化设计

### 数据结构设计
身份识别信息来源于agent元数据，用户识别信息、角色扮演、角色扮演关系采用结构化设计存储与memory-ego目录。

### 数据结构实现
- `Xxx`：内部类型，是 `XxxGeneric` 的实现，包含 `Arc`、`DashMap`、`DashSet` 优化
- `SyncXxx`：内部类型约束，是 `XxxKind` 的实现，包含 `SyncString`、`SyncMap`、`SyncSet` 优化

### 管理模块设计
- agent元数据管理模块
- 用户识别信息管理模块
- 角色设定管理模块

### 存储机制
所有内容使用JSON文件存储

### 对外接口设计
查询数据JSON格式返回

## API设计

### Agent元数据管理接口
- 提供创建、查询、更新、搜索、复制等管理功能的请求结构
- 输出统一 ApiResponse 格式

### 用户识别信息管理接口
- 提供用户信息获取、更新、重命名、标识管理等功能的请求结构
- 输出 UserRecognitionEntity 或 UserEntity 等实体结构

### 角色设定管理接口
- 提供角色创建、查询、更新、搜索、关系管理等功能的请求结构
- 输出 RolePlayEntity、OtherRoleEntity 等实体结构

## 通信接口
- 管理API：通过HTTPS API接收agent、user、role相关的管理请求
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
- 用户识别信息：通过API填写user-recognition.json
- 角色设定信息：通过API填写role-play-{role-id}.json

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

### 第4阶段：必须遵守事项和自主运行目标管理
- [ ] 在AgentMetadata数据结构中增加force_items和autonomous_goals字段
- [ ] 在role-play数据结构中增加autonomous_goals字段
- [ ] 实现更新agent必须遵守事项的功能和API
- [ ] 实现更新agent自主运行目标的功能和API
- [ ] 实现更新角色自主运行目标的功能和API
- [ ] 完善metadata.json和role-play-{role-id}.json的读写

### 第5阶段：进阶模式改造
- [ ] 记忆提取器实现
- [ ] 配置信息与记忆提取结合生成客观设定、角色设定JSON文件
- [ ] 进阶模式API
