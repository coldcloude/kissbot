# kissbot-agent 组件内功能实现顺序

Agent 组件包含 nexus 和 station 两个内部模块，启动时可选择启用 nexus 模式、station 模式或全模式。

## Nexus 模块

**实现状态：全部完成**

- [x] 配置 Cargo.toml，定义模块结构和错误类型
- [x] 实现配置管理器：JSON 配置文件加载/持久化/变更通知
- [x] 实现 LLM 客户端：支持多种 LLM 提供商，请求重试和超时控制
- [x] 实现 WS 通信客户端：多通道连接、绑定、心跳重连
- [x] 实现 Station 路由表和通信客户端（骨架）
- [x] 实现上下文构建器：内存管理、超长检测和自动重置
- [x] 实现完整的 agentic loop 流程
- [x] 实现记忆读取器：从 memory-store 读取历史记录和事件列表
- [x] 实现记忆写入器：将思考、工具调用、工具结果写入 memory-store
- [x] 实现自我认知集成：启动时和上下文重置时读取 memory-ego
- [x] 实现管理命令路由器：bind/unbind/admin/unadmin/role/mode/reenter/events/reset
- [x] 实现模式管理器：角色模式/事件模式切换
- [x] 实现协调器：核心调度、生命周期管理
- [x] 实现管理 HTTP 服务器（骨架）
- [ ] 实现 ToolCallDispatcher：内置工具识别和外置工具分派
- [ ] 实现内置记忆查询 tool（通过 tool call 调用 memory-struct）
- [ ] 实现外置工具接入（子 Station HTTP 协议实现后，经 StationClient 递归接入）
- [ ] 实现自主行为触发机制（空闲检测、自主目标加载）
- [ ] 完善管理 API 路由

## Station 模块

**实现状态：已完成嵌套化改造**（2026-08-16 Station 系统重做，见 [kissbot-agent-station 实现计划](kissbot-agent-station.md)）

Station 采用嵌套结构：全局单例（每 agent 一个）+ Toolkit 级别 + 子 Station 递归平铺。已实现：

- [x] 配置层：StationRepo.toolkits/sub_stations（station.json），ToolkitConfig/McpConfig/SubStationConfig
- [x] 全局 Station 单例：`Station::get()`/`new()`，内置注册表 filesystem（read 工具），工具名整树全局唯一（本地硬约束）
- [x] 递归平铺查询：`tools(filter)` 本地 toolkit 白名单过滤 + 直接子 Station HTTP 递归（骨架）
- [x] 工具执行：`call_tool` 本地实现表命中执行 / 直接子 HTTP 递归（骨架）
- [x] StationClient 子 Station HTTP 客户端骨架（list_tools/list_mcps/call_tool）
- [ ] MCP 真实实现（当前仅占位，见 [遗留事项](../../roadmap/pending.md)）
- [ ] 子 Station HTTP 协议实现（骨架已就位，见 [kissbot-agent-station 技术规格](../../spec/kissbot-agent-station.md)）
