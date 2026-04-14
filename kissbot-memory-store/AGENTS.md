# kissbot-memory-store 模块设计

## 模块概述
记忆存储模块，负责收集agent接收和产生的消息，作为一切agent活动的记录。

## 职责
- 文件系统存储消息
- 提供HTTPS API接口
- 实现订阅/通知机制
- 与memory-struct共同读写文件系统
- 有新数据时通过HTTPS通知注册的各memory-struct
- 支持配置可信证书文件

## 架构设计
### 核心组件
- 文件系统存储管理器
- HTTPS API服务器
- 订阅/通知管理器
- 配置管理器（包含证书配置）

## 存储结构
- 按会话组织消息文件
- JSON格式存储消息
- 支持消息索引

## 通信接口
- 输入：通过HTTPS接收agent的消息
- 输出：通过HTTPS向注册的memory-struct发送通知
- 文件系统：与memory-struct共享文件

## 实现决策
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 使用serde进行JSON序列化
- 支持从配置文件加载证书
