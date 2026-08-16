# kissbot-agent-station 模块实现计划

Station 是 [kissbot-agent](kissbot-agent.md) 组件的内部模块。实现计划已按 **Station 系统嵌套化改造** 重做（2026-08-16 计划，见 [Station 系统重做实现计划](../../superpowers/plans/2026-08-16-station-rework.md)）。

## 嵌套化改造任务划分

本次改造共 8 个任务：

| 任务 | 内容 | 状态 |
|------|------|------|
| Task 1 | 配置层改造：StationRepo.toolkits/sub_stations、ToolkitConfig/McpConfig/SubStationConfig、ContextConfig.stations → toolkits 白名单、删除 StationConfig 与 NexusRepo.stations | ✅ 完成 |
| Task 2 | station.rs 嵌套化重写：全局 Station 单例（Station::get()/new()）、Toolkit 级别、内置注册表 filesystem（read）、递归平铺 tools(filter)（None=全部/空集=空/白名单=命中）、工具名整树全局唯一（本地硬约束） | ✅ 完成 |
| Task 3 | StationClient 子 Station HTTP 客户端骨架（list_tools/list_mcps/call_tool，返回未实现） | ✅ 完成 |
| Task 4 | AgentCoordinator → Nexus 重命名（coordinator.rs → nexus.rs），Nexus 委托全局 Station（tools_for_session / execute_tool_call） | ✅ 完成 |
| Task 5 | 配置模板同步（station.json 模板 toolkits/sub_stations、nexus.json 删 stations 段、README 示例同步） | ✅ 完成 |
| Task 6 | 文档同步（本组件设计、系统设计、技术规格、实现计划、遗留事项） | ✅ 完成 |
| Task 7 | 远程工具路由缓存：Station.tool_routes（工具名→直接子 id，仅 call_tool 用）、tools() 实时拉取+更新缓存（快照、冲突保留先到者）、call_tool 查缓存路由（不再遍历全部子）、骨架期子 list_tools/list_mcps 返回空集合、MCP 不建缓存表 | ✅ 完成 |
| Task 8 | 文档同步（路由缓存行为：模块设计、技术规格、roadmap、nexus/modules stale 引用） | ✅ 完成 |

## 后续遗留事项

- MCP 真实实现（本轮仅占位）
- MCP 缓存设计（MCP 与 Tool 有嵌套关系，实现 MCP 时再设计）
- 子 Station HTTP 协议实现（骨架就位：查询返回空集合、调用返回未实现，见 [kissbot-agent-station 技术规格](../../spec/kissbot-agent-station.md)）
- 配置热更新监听接入

详见 [遗留事项](../../roadmap/pending.md)。

> 注：跨进程工具名唯一性已实现（路由缓存合并时保留先到者 + warn）；子查询 Err 时旧缓存保留（快照仅覆盖拉取成功路径）。
