# 组件间通信协议

## HTTPS

用于请求-响应模式的通信。所有 API 输入参数均放在 JSON 请求体中，路径仅用于路由到具体处理函数。

### 使用场景
- nexus → memory-store：推送记忆记录
- channel → memory-store：推送消息记录
- nexus → memory-ego：读取自我认知设定
- nexus ↔ station：nexus 向 station 发送 tool call 请求，响应中携带执行结果（同进程时通过内部调用）
- nexus → memory-struct：内置记忆查询 tool 调用（不记入记忆）
- 前端 UI → 后端：配置管理、记忆查看管理

### 消息下行流程（nexus → 外部）
- nexus 通过 WSS 发送回复 → 通道管理器接收 → 按 messenger_id + group_id + user_id 查找对应 Channel → 消息入队 → 通道管理器处理队列（推送记忆存储 + 调用 channel 发送方法）

## WSS

用于需要实时双向通信的场景，共有两组 WSS 连接：

### nexus ↔ channel
nexus 作为 WSS 客户端连接 channel 的 WSS 服务器。每个 nexus 对应唯一连接。

| 消息方向 | 消息类型 |
|----------|----------|
| nexus → channel | bind / bind_ack、outgoing_message、get_channels / channels、group_change、attachment_download / attachment_data、ping / pong |
| channel → nexus | bind / bind_ack、incoming_message、get_channels / channels、group_change、attachment_data、ping / pong |

### memory-store ↔ memory-struct
memory-struct 作为 WSS 客户端连接 memory-store。memory-store 有新数据时通过 WSS 广播通知所有已连接的 memory-struct 客户端。

## 文件系统共享

memory-store 和 memory-struct-* 共同读写同一文件系统目录。memory-store 写入记忆文件，memory-struct 读取记忆文件构建索引。

## 关键约束

- **Nexus 是唯一对接记忆系统的组件**：tool 执行结果由 station 返回 nexus 后，由 nexus 统一推送
- **nexus 与记忆系统的交互区分两种模式**：
  - agentic loop 外：直接调用 API 读取自我认知设定、推送记忆到存储模块
  - agentic loop 内：由 LLM 通过 nexus 内置 tool 从记忆结构模块查询记忆（不记入记忆）
- **消息通道不存储历史消息**：所有历史消息由记忆存储模块保存
