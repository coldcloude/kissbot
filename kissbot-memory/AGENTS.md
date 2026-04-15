# kissbot-memory 模块设计

## 模块概述
记忆基础模块，实现记忆存储的组织结构，包括记忆文件的存储目录规划和读写方法，为其他记忆模块提供基础库支持。

## 职责
- 定义记忆系统的文件存储目录结构
- 提供记忆文件的读写方法
- 为其他记忆模块（memory-store、memory-struct、memory-ego）提供基础库支持

## 文件存储目录结构
根据记忆系统的文件存储目录设计：
- **memory-agent**：记忆系统中的agent元数据单独存放
- **memory-ego**：memory-ego模块的其他设定信息单独存放
- **memory-store**：memory-store收集的原始日志单独存放
- **memory-struct-abstract**：memory-struct-abstract实现产生的数据

## 核心功能
- 目录管理：创建和管理记忆存储目录
- 文件读写：提供统一的文件读写接口
- 路径管理：提供各模块目录的路径访问方法

## 实现决策
- 不作为独立进程运行，作为库被其他记忆模块引用
- 提供trait定义，供其他模块实现
- 使用tokio作为异步运行时
- 使用serde进行JSON序列化
