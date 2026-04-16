# kissbot-memory 模块设计

## 模块概述
记忆基础模块，实现记忆存储的组织结构，以及agent元数据管理。通过程序库方式供其他记忆模块使用

## 职责
- 定义记忆系统的文件存储目录结构
- 管理agent元数据JSON文件（带读写锁防止竞争）
- 通过程序库方式供其他记忆模块（memory-store、memory-struct-*、memory-ego）使用

## 文件存储目录结构
根据记忆系统的文件存储目录设计：
- **记忆系统根目录**
  - **agent ID 1目录**
    - **memory-agent.json**：agent的元数据使用JSON文件存储
    - **memory-ego**：memory-ego模块的设定信息单独存放
    - **memory-store**：memory-store收集的原始记忆片段单独存放
    - **memory-struct-\***：memory-struct-*实现产生的数据
  - **agent ID 2目录**
    - ...（结构同上）

## 核心功能
- 目录管理：创建和管理记忆存储目录（agent ID目录及其子目录），提供路径常量
- agent元数据管理：管理agent元数据JSON文件（带读写锁防止竞争），并提供操作函数供其他记忆模块调用

## 实现决策
- 不作为独立进程运行，作为库被其他记忆模块引用
- 使用tokio作为异步运行时
- 使用serde进行JSON序列化
- 使用tokio::sync::RwLock实现读写锁防止竞争
- 目录自动创建：当需要使用某个目录时自动创建
- 路径常量：结合字符串常量和路径构建器
- 时间格式：统一使用 `yyyy-MM-dd HH:mm:ss` 格式（24小时制）
- Config和AgentManager单例通过关联函数获取：Config::get()、AgentManager::get()

## Agent元数据结构
- **id**：agent唯一标识符（UUID）
- **name**：agent名称
- **description**：agent描述（可选）
- **created_at**：创建时间（格式：yyyy-MM-dd HH:mm:ss）

## Agent元数据操作函数
- **create_agent**：创建新的agent，需要name，description可选
- **get_agent**：根据agent ID查询agent信息
- **list_agents**：查询所有agent列表
- **update_agent_name**：修改agent名称
- **update_agent_description**：修改agent描述

## 开发计划

### 第1阶段：基础结构搭建
- [x] 配置Cargo.toml，添加依赖（tokio、serde、sqlx等）
- [x] 定义模块结构（lib.rs）
- [x] 定义错误类型

### 第2阶段：路径管理实现
- [x] 定义目录名称字符串常量（MEMORY_EGO、MEMORY_STORE等）
- [x] 实现路径构建函数
- [x] 实现记忆系统根目录配置
- [x] 实现agent ID目录路径构建
- [x] 实现各子目录路径构建（memory-ego、memory-store、memory-struct-*）

### 第3阶段：目录管理实现
- [x] 实现目录自动创建功能
- [x] 实现记忆系统根目录初始化
- [x] 实现agent ID目录创建
- [x] 实现子目录创建（memory-ego、memory-store等）
- [x] 实现目录存在性检查

### 第4阶段：Agent元数据JSON文件设计
- [x] 定义Agent元数据结构（name、description、created_at）
- [x] 实现JSON文件读写功能
- [x] 实现读写锁机制防止竞争

### 第5阶段：Agent元数据操作实现
- [x] 实现新增agent函数
- [x] 实现按agent ID查询函数
- [x] 实现查询所有agent列表函数（遍历根目录下的agent目录）
- [x] 实现修改agent名称函数
- [x] 实现修改agent描述函数

### 第6阶段：开发完成
- [x] 模块功能开发完成
- [x] DirectoryManager功能已合并到AgentManager
- [x] Config和AgentManager单例通过关联函数获取
- [x] 使用tokio::sync::RwLock实现读写锁
- [x] 可成功编译并通过memory-ego模块集成测试
