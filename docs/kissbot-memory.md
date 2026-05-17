# kissbot-memory 模块设计

## 模块概述
记忆基础模块，实现记忆存储的组织结构，通过程序库方式供其他记忆模块使用

## 职责
- 定义记忆系统的文件存储目录结构
- 提供目录管理功能（DirectoryManager）
- 提供记忆索引和查询功能（MemoryIndexer）
- 通过程序库方式供其他记忆模块（memory-store、memory-struct-*、memory-ego）使用

## 文件存储目录结构
根据记忆系统的文件存储目录设计：
- **记忆系统根目录**
  - **agent ID 1目录**
    - **agent-{agent-id}**：agent存在标识文件
    - **metadata.json**：agent元数据JSON文件
    - **memory-ego**：memory-ego模块的设定信息单独存放
    - **memory-store**：memory-store收集的原始记忆片段单独存放
    - **memory-struct-\***：memory-struct-*实现产生的数据
  - **agent ID 2目录**
    - ...（结构同上）

## 核心功能
- 目录管理：创建和管理记忆存储目录（agent ID目录及其子目录），提供路径常量
- agent列表查询：通过检查agent-{agent-id}标识文件判断agent目录是否有效

## 实现决策
- 不作为独立进程运行，作为库被其他记忆模块引用
- 使用tokio作为异步运行时
- 使用serde进行JSON序列化
- 目录自动创建：当需要使用某个目录时自动创建
- 路径常量：结合字符串常量和路径构建器
- Config和DirectoryManager单例通过关联函数获取：Config::get()、DirectoryManager::get()

## DirectoryManager功能
- list_agents：查询所有有效agent列表（通过agent-{agent-id}标识文件判断）
- ensure_agent_dir：确保agent目录存在并创建agent-{agent-id}标识文件
- ensure_agent_ego_dir：确保agent的memory-ego目录存在
- ensure_agent_store_dir：确保agent的memory-store目录存在
- ensure_agent_struct_dir：确保agent的memory-struct-*目录存在

## MemoryIndexer功能
- 记忆索引管理：为记忆记录构建和维护索引，记录在文件中的位置
- 索引过期处理：区分小过期（新记录追加）和大过期（文件重写）
- 记忆查询：支持按时间范围快速查询记忆记录
- 自动索引重建：当索引过期或不存在时，自动重新构建索引

## 开发计划

### 第1阶段：基础结构搭建
- [x] 配置Cargo.toml，添加依赖（tokio、serde等）
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
- [x] 实现agent ID目录创建和agent-{agent-id}标识文件
- [x] 实现子目录创建（memory-ego、memory-store等）
- [x] 实现目录存在性检查
- [x] 实现agent列表查询（通过agent-{agent-id}标识文件）

### 第4阶段：索引和查询功能实现
- [x] 实现索引结构（IndexEntry、FileIndex、MemoryIndexer）
- [x] 实现索引过期管理（小过期、大过期）
- [x] 实现索引查询功能
- [x] 实现倒序读行支持位置记录

### 第5阶段：开发完成
- [x] 模块功能开发完成
- [x] DirectoryManager和MemoryIndexer单例通过关联函数获取
- [x] 可成功编译
