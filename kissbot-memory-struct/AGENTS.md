# kissbot-memory-struct 模块设计

## 模块概述
记忆结构框架模块，定义memory-struct的trait接口，供具体的memory-struct实现模块使用。

## 职责
- 定义memory-struct trait接口
- 定义记忆搜索tool接口
- 与memory-store共同读写文件系统
- 支持配置可信证书文件

## 架构设计
### 核心trait
- MemoryStruct trait：定义记忆结构的基本接口
- SearchTool trait：定义记忆搜索tool的接口

### 核心组件
- HTTPS API服务器（提供tool接口）
- 文件系统访问层
- 配置管理器（包含证书配置）

## 通信接口
- 输入：通过HTTPS接收memory-store的通知
- 输出：通过HTTPS向agent提供搜索tool
- 文件系统：与memory-store共享文件

## 实现决策
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 支持从配置文件加载证书
