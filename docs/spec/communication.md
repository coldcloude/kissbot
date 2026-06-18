# 组件间通信协议

## HTTPS

用于请求-响应模式的通信。所有 API 输入参数均放在 JSON 请求体中，路径仅用于路由到具体处理函数。

### 认证要求
所有 HTTPS 请求必须携带认证 header，认证方式见 [authentication.md](authentication.md)。

### 请求路径约定
- 路径仅用于路由到具体处理函数，不含动态参数
- 所有输入参数放在 JSON 请求体中传递

## WSS

用于需要实时双向通信的场景：

### nexus ↔ 消息通道
nexus 作为 WSS 客户端连接 channel 的 WSS 服务器。每个 nexus 对应唯一连接，通过心跳维持连接。

WSS 握手阶段的 HTTP Upgrade 请求与 HTTPS 使用同一套认证机制，在握手阶段完成认证后建立 WebSocket 连接。

| 消息方向 | 消息类型 |
|----------|----------|
| nexus → channel | bind / bind_ack、outgoing_message、get_channels / channels、group_change、attachment_download / attachment_data、ping / pong |
| channel → nexus | bind / bind_ack、incoming_message、get_channels / channels、group_change、attachment_data、ping / pong |

### memory-store ↔ memory-struct-*
memory-struct 作为 WSS 客户端连接 memory-store。memory-store 有新数据时广播通知所有已连接的 memory-struct 客户端。

## SSE

- 通道后端 → 通道前端（浏览器）：实时推送新消息，支持断线自动重连

## 文件系统共享

- memory-store 和 memory-struct-* 共同读写同一文件系统目录
- memory-store 写入记忆文件
- memory-struct 读取记忆文件构建索引
