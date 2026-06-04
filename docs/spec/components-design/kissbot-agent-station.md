# kissbot-agent-station 模块设计

## 概述
Station — Agent 组件的 Tool 执行主机模块。智能体的"行动"部分，专注于执行具体的工具操作。Station 不直接与 LLM 通信，只提供工具注册、接收 tool call、执行工具、将结果通过 WSS 返回给 nexus 的能力。Agent 启动时可选择是否启用 station 模块。

Station 可以运行在多种形态的设备上：
- **通用服务器**：运行标准工具集（文件操作、命令执行等）
- **网络设备**：读写网络配置、获取监控数据等
- **智能家电**：执行物理世界操作（开关、调节等）
- **机器人**：执行物理动作（移动、抓取等）

同一 agent 内部的 station 与 nexus 通过内部 WSS 通信。外部独立的 station 可以跨 agent 为其他 agent 的 nexus 提供工具服务。

## 内部模块

### 1. ToolExecutor - 工具执行器
- 接收来自 nexus 的 tool call（tool name + parameters）
- 查找并调用已注册的工具
- 捕获工具执行结果或错误
- 支持同步和异步工具执行

### 2. ToolRegistry - 工具注册表
- 管理本 station 支持的工具列表
- 提供工具注册和注销接口
- 每个工具包含：名称、描述、参数 schema、执行函数
- 启动时从配置文件加载内置工具
- 支持运行时动态注册工具

### 3. WSSServer - WSS 服务器
- 作为 WSS 服务器，接受 nexus 的连接
- 连接建立后发送工具注册信息给 nexus
- 接收来自 nexus 的 tool call 请求
- 发送 tool call 结果回 nexus
- 管理多 nexus 并行连接
- 支持心跳检测
- 通信协议保持统一，资源受限设备可实现精简版 WSS 客户端（仅实现必要的消息类型）

## 工具定义

每个工具包含以下信息：
```
tool_name:    工具名称（LLM 在 tool call 中使用的名称）
description:  工具描述（LLM 理解工具用途）
parameters:   参数 schema（JSON Schema 格式，描述参数结构）
handler:      执行函数（接受 parameters，返回结果）
```

## 工作方式

### Tool 执行流程
```
1. Nexus 通过 WSS 发送 tool call（tool name + parameters）
2. WSSServer 接收请求，传递给 ToolExecutor
3. ToolExecutor 从 ToolRegistry 查找对应工具
   ├─ 找到 → 用 parameters 调用工具的 handler
   └─ 未找到 → 返回错误信息
4. handler 执行完毕，返回结果
5. WSSServer 将结果通过 WSS 返回给 nexus
```
所有记忆操作（包括 tool call 和 tool result 的记录）由 nexus 统一完成。

### Station 启动流程
```
1. 加载配置文件，初始化 ToolRegistry
2. 注册内置工具
3. 启动 WSSServer
4. 等待 nexus 连接
5. 有 nexus 连接时，发送工具注册信息（station 标识、工具列表）
6. 进入工具调用监听状态
```

### 工具注册信息
Station 连接 nexus 时发送的工具注册信息包含：
- station 标识（名称、版本、设备类型）
- 支持的工具列表（每个工具的名称、描述、参数 schema）
- 可选的 capacity/负载信息

## 常见 Station 类型

### 工程工具站（kissbot-station-project）
注册工具：Read、Write、Edit（文件操作）、Bash（命令执行）
绑定本地工作区目录，提供文件系统和 shell 操作

### 网络工具站（kissbot-station-network）
注册工具：WebSearch（网络搜索）、WebFetch（网页抓取）
提供网络信息获取能力

### 设备工具站（任意名称）
运行在网络设备、智能家电、机器人等物理设备上
提供设备相关的控制、读写、监控等工具

## 外部通信

| 对端 | 协议 | 通信时机 | 内容 |
|------|------|----------|------|
| Nexus | WSS | 持续 | 接收 tool call、返回结果、发送注册信息、心跳 |
| 智能体配置界面 | HTTPS | 用户操作时 | 配置 station 参数和工具 |
