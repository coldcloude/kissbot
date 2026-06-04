# kissbot-memory-ego 组件设计

## 概述
自我认知模块，管理 agent 的双重自我认知设定。独立于 nexus 模块，与其他记忆模块共享同一文件系统。

## 内部模块

### 1. AgentManager - Agent 元数据管理器
- 管理 agent 元数据的 JSON 文件存储（metadata.json）
- 使用内存缓存降低 IO 开销
- 使用读写锁防止竞争
- 支持 agent 的创建、查询、更新、复制、搜索

### 2. UserRecognitionManager - 用户识别信息管理器
- 管理 user-recognition.json 的读写
- 提供用户信息的增删改查
- 管理用户标识（messenger_id + user_id + group_id 组合）、权限、用户间关系

### 3. RolePlayManager - 角色设定管理器
- 管理 role-play-{role-name}.json 的读写
- 提供角色设定的创建、查询、更新、搜索
- 管理角色与用户的关系、角色间关系

### 4. SearchManager - 全文搜索
- 使用倒排索引实现全文搜索
- 脏标记机制延迟更新搜索索引
- 启动时自动加载所有 agent 到搜索索引

### 5. HTTPS API 服务器
- Agent 元数据管理 API
- 用户识别信息管理 API
- 角色设定管理 API

## Agent 自我认知数据模型

### 客观设定（每个 agent ID 对应一份）
- **身份标识**：id、name、description、created_at、禁止事项、自主运行目标
- **用户识别信息**：用户列表，每项包含名称、身份（所有者/管理员/其他用户）、关联的用户标识（messenger_id+user_id+group_id）、与其他用户的关系、描述

### 角色设定（每个 agent ID 对应多份）
- **角色扮演**：role-name、描述、自主运行目标
- **角色扮演关系**：role-name、关联的用户名、与 agent 角色的关系、与其他角色的关系、描述

## 文件存储结构
```
{agent-id}/memory-ego/
├── user-recognition.json              # 用户识别信息
└── role-play-{role-name}.json         # 角色设定
```

## 数据来源
- 基础模式：通过 HTTPS API 手动填写 JSON 文件
- 进阶模式（规划中）：结合配置信息和从记忆中提取的信息，自动生成设定

## 外部通信

| 对端 | 协议 | 通信时机 | 内容 |
|------|------|----------|------|
| nexus | HTTPS | 启动/重置时 | 提供自我认知设定 |
| 智能体配置界面 | HTTPS | 用户操作时 | 管理 agent 元数据 |
| 记忆管理界面 | HTTPS | 用户操作时 | 查看/管理 |
