# kissbot-agent 模块设计

## 模块概述
智能体核心模块，负责将消息加工为LLM可用的消息，通过agentic loop调用LLM执行操作，返回消息。

## 职责
- 集成LLM API
- 实现agentic loop
- 作为WSS客户端连接channel
- 作为HTTPS客户端连接memory-store
- 实现tool调用机制
- 作为HTTPS客户端连接memory-struct（通过tool）
- 作为HTTPS服务器供agent-config调用修改配置
- 支持配置可信证书文件

## 架构设计
### 核心组件
- LLM API集成层
- Agentic Loop控制器
- WSS客户端（连接channel）
- HTTPS客户端（连接memory-store）
- Tool调用管理器（可以实现memory-struct的tool调用）
- 配置管理HTTPS服务器（包含证书配置）

## 通信接口
- 输入：通过WSS从channel接收消息
- 输出：通过WSS向channel发送消息
- 存储：通过HTTPS向memory-store记录消息
- 搜索：通过tool从memory-struct搜索记忆
- 配置：通过HTTPS服务器接收agent-config的配置修改

## 实现决策
- 使用tokio作为异步运行时
- 使用tokio-tungstenite实现WSS客户端
- 使用reqwest实现HTTPS客户端
- 使用axum实现配置管理HTTPS服务器
- 支持从配置文件加载证书

## 开发计划

### 第1步：基础框架搭建
- [ ] 配置Cargo.toml依赖
- [ ] 创建项目基础结构
- [ ] 实现配置文件加载（JSON格式）
- [ ] 实现证书配置加载

### 第2步：配置管理HTTPS服务器
- [ ] 实现axum HTTPS服务器
- [ ] 实现配置读取API
- [ ] 实现配置写入API
- [ ] 实现证书配置API

### 第3步：WSS客户端
- [ ] 实现tokio-tungstenite WSS客户端
- [ ] 实现与channel的连接管理
- [ ] 实现消息接收处理
- [ ] 实现消息发送功能

### 第4步：HTTPS客户端
- [ ] 实现reqwest HTTPS客户端
- [ ] 实现与memory-store的通信
- [ ] 实现消息记录功能
- [ ] 实现与memory-struct的tool调用

### 第5步：LLM API集成
- [ ] 定义LLM API接口trait
- [ ] 实现配置化LLM API调用
- [ ] 实现消息格式化

### 第6步：Tool调用机制
- [ ] 定义Tool接口trait
- [ ] 实现Tool注册机制
- [ ] 实现Tool调用执行
- [ ] 集成memory-struct的搜索tool

### 第7步：Agentic Loop
- [ ] 实现消息处理流程
- [ ] 实现LLM调用循环
- [ ] 实现Tool调用决策
- [ ] 实现结果返回

### 第8步：集成测试
- [ ] 端到端测试
- [ ] 性能测试
- [ ] 错误处理测试
