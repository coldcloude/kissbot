# 组件间通信协议

## HTTP

适用于 kissbot-agent（Nexus 与内嵌 Station、管理 API、station MCP 服务端）、消息通道、记忆系统、管理界面之间的请求-响应通信。

| 通信方向 | 说明 |
|----------|------|
| kissbot-agent ↔ station（子 Station） | 请求-响应（本地同进程为内部调用，远程子 Station 走 MCP：Streamable HTTP 单端点） |
| kissbot-agent → memory-store | 请求-响应（推记录 + 读记忆） |
| kissbot-agent → memory-struct-* | 预留（记忆搜索工具未实现） |
| kissbot-agent ↔ memory-ego | 请求-响应（自我认知信息读取与校验） |
| channel → memory-store | 请求-响应 |
| 管理界面 ↔ kissbot-agent / 各后端 | 请求-响应 |
| 通道前端 ↔ 通道后端 | 请求-响应 |

## WS

| 通信方向 | 说明 |
|----------|------|
| kissbot-agent ↔ 消息通道 | agent 作为客户端连接 channel（经 kissbot-channel-client），支持多通道同时连接，心跳维持。认证方式与 HTTP 同一套 |
| memory-store ↔ memory-struct-* | memory-struct 作为客户端连接 memory-store，新数据广播通知 |

## SSE

| 通信方向 | 说明 |
|----------|------|
| 通道后端 → 通道前端（浏览器） | 实时推送新消息，支持断线自动重连 |

## 文件系统共享

| 通信方向 | 说明 |
|----------|------|
| memory-store ↔ memory-struct-* | 共享读写同一文件系统目录 |

## 对外部署

各组件内部使用 HTTP 和 WS 通信。对外提供服务时（如浏览器访问通道后端、远程 station 连接），需使用代理服务器（如 nginx）将流量转换为 HTTPS 和 WSS，并管理 TLS 证书。
