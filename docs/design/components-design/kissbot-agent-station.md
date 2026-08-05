# kissbot-agent-station 模块设计

## 概述
Station — Agent 组件的 Tool 执行主机模块。智能体的"行动"部分，专注于执行具体的工具操作。Station 不直接与 LLM 通信，只提供工具注册、接收 tool call、执行工具、将结果返回给 nexus 的能力。

Station 可以运行在多种形态的设备上：
- **通用服务器**：运行标准工具集（文件操作、命令执行等）
- **网络设备**：读写网络配置、获取监控数据等
- **智能家电**：执行物理世界操作（开关、调节等）
- **机器人**：执行物理动作（移动、抓取等）

Station 的工具执行按 `base_url` 区分调用方式：`base_url` 为空的本地 Station 在进程内直接执行工具；`base_url` 非空的远程 Station 通过 HTTP 调用（本轮为骨架，未实现）。外部独立的 station 可以跨 agent 为其他 agent 的 nexus 提供工具服务。

## 核心功能

1. **工具注册与管理**：Station 配置声明工具列表（工具名、描述、参数 JSON Schema），本地 Station 在启动时注册工具实现
2. **工具执行**：接收来自 nexus 的 tool call，查找并执行对应工具，返回结果

## 内部模块

### 1. Tool trait - 工具接口
- 统一参数（serde_json::Value）与返回值（serde_json::Value）
- 异步执行，实现者实现 `call` 方法

### 2. StationRuntime - 工具执行器
- 持有 Station 配置与本地工具实现表
- 工具调用按 `base_url` 分流：为空时在本地工具表查找并执行（未注册报错）；非空时走远程 HTTP 调用（本轮骨架，返回未实现错误）
- 提供工具配置聚合接口（`configured_tools`），供 nexus 收集发给 LLM 的工具定义

### 3. 内置工具
- 示例工具 **Read**：读取文本文件（参数 `path`），注册在 base_url 为空的本地 Station 上
- 路径安全校验：参数 path 先基于当前工作目录解析为绝对路径，再规范化（消解 `..` 与符号链接），校验其位于当前工作目录或其子目录内，越界拒绝，防路径穿透；返回内容限长（64KB）

### 4. HTTPServer - HTTP 服务器（远程模式）
- 接收来自 nexus 的 HTTPS 请求（tool call）
- 解析请求中的 tool name 和 parameters
- 将 tool result 作为 HTTP 响应返回
- 本轮未实现，作为远程 Station 后端预留

## 功能流程

### Tool 执行流程
Nexus 的 agentic loop 解析 LLM 返回的 tool call → 按工具名在启用 Station 的运行态中查找（base_url 为空 → 本地工具表执行；非空 → 远程 HTTP 调用骨架）→ 工具执行完毕，结果作为 tool 消息追加回上下文。

所有记忆操作（包括 tool call 和 tool result 的记录）由 nexus 统一完成。

### Station 运行模式
加载配置文件（StationConfig 含工具列表）→ 为每个 Station 构建 StationRuntime → base_url 为空的本地 Station 注册内置工具实现 → 等待 tool call 请求（本地执行或远程调用）。

## 常见 Station 类型

### 工程工具站
注册文件操作和命令执行等工程工具。绑定本地工作区目录，提供文件系统和 shell 操作。

### 网络工具站
注册网络搜索和网页抓取等工具。提供网络信息获取能力。

### 设备工具站
运行在网络设备、智能家电、机器人等物理设备上。提供设备相关的控制、读写、监控等工具。
