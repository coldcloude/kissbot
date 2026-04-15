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
- 使用rusqlite或sqlx进行SQLite数据库操作
- 目录自动创建：当需要使用某个目录时自动创建
- 路径常量：结合字符串常量和路径构建器

## 开发计划

### 第1阶段：基础结构搭建
- [ ] 配置Cargo.toml，添加依赖（tokio、serde、rusqlite/sqlx等）
- [ ] 定义模块结构（lib.rs）
- [ ] 定义错误类型

### 第2阶段：路径管理实现
- [ ] 定义目录名称字符串常量（MEMORY_EGO、MEMORY_STORE等）
- [ ] 实现路径构建器（PathBuilder）
- [ ] 实现记忆系统根目录配置
- [ ] 实现agent ID目录路径构建
- [ ] 实现各子目录路径构建（memory-ego、memory-store、memory-struct-*）

### 第3阶段：目录管理实现
- [ ] 实现目录自动创建功能
- [ ] 实现记忆系统根目录初始化
- [ ] 实现agent ID目录创建
- [ ] 实现子目录创建（memory-ego、memory-store等）
- [ ] 实现目录存在性检查

### 第4阶段：Agent元数据数据库设计
- [ ] 设计SQLite数据库表结构（agents表）
- [ ] 定义Agent元数据结构（name、created_at）
- [ ] 实现数据库连接管理
- [ ] 实现数据库初始化（创建表）

### 第5阶段：Agent元数据操作实现
- [ ] 实现新增agent函数
- [ ] 实现按agent ID查询函数
- [ ] 实现查询所有agent列表函数

### 第6阶段：集成测试
- [ ] 编写单元测试
- [ ] 编写集成测试
- [ ] 测试目录创建和路径构建
- [ ] 测试数据库操作
