# kissbot-memory-ego 模块设计

## 模块概述
自我认知模块，独立的记忆系统模块，和其他记忆模块读写同一个文件系统，负责管理agent的基础信息。

## 职责
- 调用memory基础模块的DirectoryManager进行目录管理
- 实现agent元数据管理（AgentManager）
- 提供管理agent元数据的HTTPS API（如新增agent接口）
- 通过配置文件读取agent的基础信息
- 通过记忆提取的方式生成agent的基础信息
- 保存agent的基础信息
- 每个agent ID对应一个客观设定，对应多个角色设定
- 仅使用API，本身不封装为tool
- 支持配置可信证书文件

## 架构设计
### 核心组件
- 配置文件读取器
- 记忆提取器
- 基础信息管理器（EgoManager）
- agent元数据管理器（AgentManager，本模块实现）
- HTTPS API服务器
- 配置管理器（包含证书配置）

## 数据结构
### Agent基础信息
- agent ID（唯一标识）
- 客观设定（纯文本，可用MD）
- 多个角色设定（纯文本，可用MD）

### Agent元数据结构
- id：agent唯一标识符（UUID）
- name：agent名称
- description：agent描述
- created_at：创建时间（格式：yyyy-MM-dd HH:mm:ss）

## Agent元数据操作函数
- create_agent：创建新的agent，需要name和description
- get_metadata_clone：获取agent元数据的克隆副本
- get_metadata：通过回调函数读取agent元数据
- update_agent_name：修改agent名称
- update_agent_description：修改agent描述
- update_agent_name_description：同时修改agent名称和描述

## API设计

### Agent元数据管理接口

#### 新增agent接口
- **接口**：新增agent
- **输入**：agent元数据（名称、描述）
- **返回**：成功或失败状态

#### 查询agent元数据接口
- **接口**：按agent ID查询
- **返回内容**：agent元数据（名称、描述、创建时间等）

#### 查询所有agent列表接口
- **接口**：查询所有agent
- **返回内容**：agent元数据列表

#### 修改agent名称接口
- **接口**：修改agent名称
- **输入**：agent ID、新名称
- **返回**：成功或失败状态

#### 修改agent描述接口
- **接口**：修改agent描述
- **输入**：agent ID、新描述
- **返回**：成功或失败状态

### 客观设定查询接口
- **接口**：按agent ID查询
- **返回内容**：
  1. 身份标识
  2. 用户识别信息

### 角色设定查询接口
- **接口**：按agent ID + 角色ID查询
- **返回内容**：
  1. 角色扮演
  2. 角色扮演关系

## 通信接口
- 输入：通过HTTPS API接收agent的查询请求
- 输出：通过HTTPS API返回agent的基础信息
- 文件系统：读取配置文件，与其他记忆模块共享文件系统

## 文件存储目录结构
根据记忆系统的文件存储目录设计：
- **记忆系统根目录**
  - **agent ID目录**
    - **agent-{agent-id}**：agent存在标识文件（由memory模块管理）
    - **metadata.json**：agent元数据JSON文件
    - **memory-ego**：memory-ego模块的设定信息单独存放

## MD文件结构

### 客观设定
每个agent ID对应2个MD文件（存放在agent ID目录下的memory-ego子目录）：
1. **身份标识**：关于agent身份的客观事实，比如名称、创建时间（信息来源于metadata.json）
2. **用户识别信息**：agent和各个用户的客观关系，各个用户间的客观关系

### 角色设定
每个agent ID + 角色ID对应2个MD文件（存放在agent ID目录下的memory-ego子目录）：
1. **角色扮演**：本次会话中对外展现的角色，包括agent对自己的称呼，用户可能对agent的各种称呼，对话采用的语气、用词风格
2. **角色扮演关系**：各个用户在本次对话中的角色，agent在本次对话中对用户角色的称呼

## MD文件来源模式

### 基础模式
- 直接手动放置MD文件
- 每个设定内容对应一个文本MD文件

### 进阶模式
- 结合配置信息和从记忆中提取的信息生成MD文件
- 支持自动化生成设定内容

## AgentManager实现
- 实现agent元数据的JSON文件存储（metadata.json）
- 使用tokio::sync::RwLock实现读写锁防止竞争
- 使用内存缓存，首次读取后缓存到内存
- 双重锁定机制确保数据一致性
- 单例通过关联函数获取：AgentManager::get()

## 开发计划

### 第1阶段：基础模式实现
- [x] 调用memory基础模块库（DirectoryManager）
- [x] 实现AgentManager管理agent元数据
- [x] agent元数据管理API（新增agent、查询agent元数据、修改agent名称描述）
- [x] MD文件基础模式实现（身份标识MD文件自动生成，其他MD文件手动放置）
- [x] HTTPS API接口
- [x] 客观设定和角色设定的读取API

### 第2阶段：进阶模式改造
- [ ] 记忆提取器实现
- [ ] 配置信息与记忆提取结合生成MD文件
- [ ] 进阶模式API

## 实现决策
- 不作为memory-struct的子模块，不实现memory-struct的接口
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 使用serde进行JSON序列化
- 支持从配置文件加载证书
- AgentManager在本模块实现，管理agent元数据的JSON文件存储
