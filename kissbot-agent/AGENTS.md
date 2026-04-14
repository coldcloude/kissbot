# kissbot-agent 模块设计

## 模块概述
智能体核心模块，负责将消息加工为LLM可用的消息，通过agentic loop调用LLM执行操作，返回消息。

## 职责
- 作为WSS客户端连接channel
- 作为HTTPS客户端连接memory-store
- 作为HTTPS客户端连接memory-struct（通过tool）
- 集成LLM API
- 实现agentic loop
- 实现tool调用机制
- 支持配置可信证书文件

## 架构设计
### 核心组件
- LLM API集成层
- Agentic Loop控制器
- Tool调用管理器
- WSS客户端（连接channel）
- HTTPS客户端（连接memory-store和memory-struct）
- 配置管理器（包含证书配置）

## 通信接口
- 输入：通过WSS从channel接收消息
- 输出：通过WSS向channel发送消息
- 存储：通过HTTPS向memory-store记录消息
- 搜索：通过HTTPS从memory-struct搜索记忆

## 实现决策
- 使用tokio作为异步运行时
- 使用tokio-tungstenite实现WSS客户端
- 使用reqwest实现HTTPS客户端
- 支持从配置文件加载证书
