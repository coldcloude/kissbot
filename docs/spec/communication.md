# 组件间通信协议

## HTTPS

用于请求-响应模式的通信。所有 API 输入参数均放在 JSON 请求体中，路径仅用于路由到具体处理函数。

### 使用场景
- nexus → memory-store：推送记忆记录
- nexus → memory-ego：读取/管理自我认知设定
- nexus → memory-struct：内置记忆查询 tool 调用（不记入记忆）
- nexus ↔ station：nexus 向 station 发送 tool call 请求，响应中携带执行结果（同进程时通过内部调用）
- channel → memory-store：推送消息记录
- agent 配置界面 → nexus / station：读取/更新配置
- 记忆管理界面 → memory-store / memory-struct-* / memory-ego：查看管理记忆
- 通道前端 → 通道后端：消息收发、群组管理、附件操作

### 消息下行流程（nexus → 外部）
- nexus 通过 WSS 发送回复 → 通道管理器接收 → 按 messenger_id + group_id + user_id 查找对应 Channel → 消息入队 → 通道管理器处理队列（推送记忆存储 + 调用 channel 发送方法）

## WSS

用于需要实时双向通信的场景：

### nexus ↔ 消息通道
nexus 作为 WSS 客户端连接 channel 的 WSS 服务器。每个 nexus 对应唯一连接。

| 消息方向 | 消息类型 |
|----------|----------|
| nexus → channel | bind / bind_ack、outgoing_message、get_channels / channels、group_change、attachment_download / attachment_data、ping / pong |
| channel → nexus | bind / bind_ack、incoming_message、get_channels / channels、group_change、attachment_data、ping / pong |

### memory-store ↔ memory-struct
memory-struct 作为 WSS 客户端连接 memory-store。memory-store 有新数据时通过 WSS 广播通知所有已连接的 memory-struct 客户端。

## SSE

- 通道后端 → 通道前端（浏览器）：实时推送新消息

## 文件系统共享

memory-store 和 memory-struct-* 共同读写同一文件系统目录。memory-store 写入记忆文件，memory-struct 读取记忆文件构建索引。

## 各组件通信总览

| 发送方 | 接收方 | 协议 | 通信内容 | 通信时机 |
|--------|--------|------|----------|----------|
| nexus（agent） | 消息通道 | WSS | 收发消息、绑定、附件下载、心跳 | 持续 |
| 消息通道 | nexus（agent） | WSS | 收发消息、绑定、附件操作、心跳 | 持续 |
| nexus（agent） | station | HTTPS | tool call 请求/响应 | agentic loop 内 |
| nexus（agent） | memory-store | HTTPS | 推送记忆记录 | 消息产生时 |
| nexus（agent） | memory-struct | HTTPS | 内置记忆查询 tool 调用 | agentic loop 内 |
| nexus（agent） | memory-ego | HTTPS | 读取自我认知设定 | 启动/重置时 |
| 消息通道 | memory-store | HTTPS | 推送消息记录 | 消息产生时 |
| memory-store | memory-struct-* | WSS | 新数据通知 | 新数据到达时 |
| memory-store | memory-struct-* | 文件系统 | 共享读取记忆文件 | 持续 |
| memory-struct-* | memory-store | 文件系统 | 读取记忆文件 | 持续 |
| memory-ego | nexus（agent） | HTTPS | 提供自我认知设定 | 启动/重置时 |
| 通道后端 | 通道前端（浏览器） | HTTPS+SSE | 消息收发、群组管理、附件操作、实时推送 | 持续 |
| agent 配置界面 | nexus / station | HTTPS | 读取/更新配置 | 用户操作时 |
| 记忆管理界面 | memory-store / memory-struct-* / memory-ego | HTTPS | 查看管理记忆 | 用户操作时 |

## 关键约束

- **Nexus 是唯一对接记忆系统的组件**：tool 执行结果由 station 返回 nexus 后，由 nexus 统一推送
- **nexus 与记忆系统的交互区分两种模式**：
  - agentic loop 外：直接调用 API 读取自我认知设定、推送记忆到存储模块
  - agentic loop 内：由 LLM 通过 nexus 内置 tool 从记忆结构模块查询记忆（不记入记忆）
- **消息通道不存储历史消息**：所有历史消息由记忆存储模块保存
