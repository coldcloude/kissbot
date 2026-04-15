# kissbot-memory 模块设计

## 模块概述
记忆基础模块，实现记忆存储的组织结构，以及agent元数据管理。通过程序库方式供其他记忆模块使用

## 职责
- 定义记忆系统的文件存储目录结构
- 管理agent元数据SQLite数据库
- 通过程序库方式供其他记忆模块（memory-store、memory-struct-*、memory-ego）使用

## 文件存储目录结构
根据记忆系统的文件存储目录设计：
- **记忆系统根目录**
  - **memory-agent.db**：记忆系统中的agent元数据使用SQLite数据库管理
  - **agent ID 1目录**
    - **memory-ego**：memory-ego模块的设定信息单独存放
    - **memory-store**：memory-store收集的原始记忆片段单独存放
    - **memory-struct-\***：memory-struct-*实现产生的数据
  - **agent ID 2目录**
    - ...（结构同上）

## 核心功能
- 目录管理：创建和管理记忆存储目录（agent ID目录及其子目录），提供路径常量
- agent元数据管理：管理agent元数据SQLite数据库，并提供操作函数供其他记忆模块调用

## 实现决策
- 不作为独立进程运行，作为库被其他记忆模块引用
- 使用tokio作为异步运行时
- 使用serde进行JSON序列化
