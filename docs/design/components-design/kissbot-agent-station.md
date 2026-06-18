# kissbot-agent-station 模块设计

## 概述
Station — Agent 组件的 Tool 执行主机模块。智能体的"行动"部分，专注于执行具体的工具操作。Station 不直接与 LLM 通信，只提供工具注册、接收 tool call、执行工具、将结果通过 HTTPS 响应返回给 nexus 的能力。Agent 启动时可选择是否启用 station 模块。

Station 可以运行在多种形态的设备上：
- **通用服务器**：运行标准工具集（文件操作、命令执行等）
- **网络设备**：读写网络配置、获取监控数据等
- **智能家电**：执行物理世界操作（开关、调节等）
- **机器人**：执行物理动作（移动、抓取等）

同进程内 station 与 nexus 通过内部调用通信，远程时通过 HTTPS。外部独立的 station 可以跨 agent 为其他 agent 的 nexus 提供工具服务。

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

### 3. HTTPServer - HTTP 服务器
- 接收来自 nexus 的 HTTPS 请求（tool call）
- 解析请求中的 tool name 和 parameters
- 将 tool result 作为 HTTP 响应返回
- 管理请求超时和并发
- 资源受限设备可实现精简版 HTTP 服务（仅实现必要的路由）

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
1. Nexus 向 Station 发起 HTTPS 请求（tool name + parameters）
2. HTTPServer 接收请求，传递给 ToolExecutor
3. ToolExecutor 从 ToolRegistry 查找对应工具
   ├─ 找到 → 用 parameters 调用工具的 handler
   └─ 未找到 → 返回错误信息
4. handler 执行完毕，将结果作为 HTTP 响应返回
```
所有记忆操作（包括 tool call 和 tool result 的记录）由 nexus 统一完成。

### Station 运行模式
```
1. 加载配置文件，初始化 ToolRegistry
2. 注册内置工具
3. 启动 HTTPServer
4. 等待 tool call 请求
```

### 工具注册信息
Station 对外提供以下信息供 nexus 发现和调用：
- station 标识（名称、版本、设备类型）
- 支持的工具列表（每个工具的名称、描述、参数 schema）
- 地址（URL）

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
| Nexus | HTTPS（请求/响应） | agentic loop 内 | nexus 发送 tool call 请求，station 返回 tool result 响应 |
| 智能体配置界面 | HTTPS | 用户操作时 | 配置 station 参数和工具 |
