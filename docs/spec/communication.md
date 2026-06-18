# 组件间通信协议

## HTTPS

适用于 nexus、station、消息通道、记忆系统、管理界面之间的请求-响应通信。

| 通信方向 | 说明 |
|----------|------|
| nexus ↔ station | 请求-响应 |
| nexus → memory-store | 请求-响应 |
| nexus → memory-struct-* | 请求-响应 |
| nexus ↔ memory-ego | 请求-响应 |
| channel → memory-store | 请求-响应 |
| 管理界面 ↔ 各后端 | 请求-响应 |
| 通道前端 ↔ 通道后端 | 请求-响应 |

## WSS

| 通信方向 | 说明 |
|----------|------|
| nexus ↔ 消息通道 | nexus 作为客户端连接 channel，唯一连接，心跳维持。认证方式与 HTTPS 同一套 |
| memory-store ↔ memory-struct-* | memory-struct 作为客户端连接 memory-store，新数据广播通知 |

## SSE

| 通信方向 | 说明 |
|----------|------|
| 通道后端 → 通道前端（浏览器） | 实时推送新消息，支持断线自动重连 |

## 文件系统共享

| 通信方向 | 说明 |
|----------|------|
| memory-store ↔ memory-struct-* | 共享读写同一文件系统目录 |
