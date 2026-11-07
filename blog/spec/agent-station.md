# Station

## 实现细节

- Station 本身实现为一个完整的 MCP 服务端，采用 MCP 协议 2026-07-28 版
- 采用自定义 MCP 扩展增加所属 group 和 ancestor Station
- Station 可以作为一个通用 MCP 接入到其他 Agent，也可以接入其他通用 MCP
- SubStation 中的工具和 group 加上 stationId 以便全局唯一

- 读取工具列表时，将各子 Station 的工具记录下来，调用时直接执行不再查询

- 工具列表结果按工具名排序（同一集合多次请求顺序一致，便于客户端与提示词缓存）
- 当传入的工具列表为空时，模型请求**不带工具字段**（兼容无工具场景）

## 计划

- 接入非 Station 的通用 MCP 还未实现
