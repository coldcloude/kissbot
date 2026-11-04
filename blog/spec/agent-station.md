# Station

## 实现细节

- Station 本身实现为一个完整的 MCP 服务端，采用 MCP 协议 2026-07-28 版
- 采用自定义 MCP 扩展增加所属 group 和 ancestor Station
- Station 可以作为一个通用 MCP 接入到其他 Agent，也可以接入其他通用 MCP
- SubStation 中的工具和 group 加上 stationId 以便全局唯一

## 计划

- 接入非 Station 的通用 MCP 还未实现
