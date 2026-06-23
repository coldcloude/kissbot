# 组件和流程的实现顺序规划

## 实现状态总览

| 组件 | 状态 | 说明 |
|------|------|------|
| kissbot-api | 全部完成 | API 类型和 trait 定义 |
| kissbot-memory | 全部完成 | 目录管理、索引、记录类型 |
| kissbot-memory-store | 全部完成 | 记录管理器、API 服务器 |
| kissbot-security | 全部完成 | HTTP/WS 认证中间件 |
| kissbot-channel | 全部完成 | Messenger 接口、ChannelManager |
| kissbot-channel-web | 全部完成 | 后端 + 前端完整实现 |
| kissbot-channel-web-ui | 全部完成 | React 前端 |
| kissbot-memory-ego | 全部完成 | Agent 元数据、角色设定、搜索 |
| kissbot-agent（Nexus） | 全部完成 | LLM 集成、agentic loop、通道通信、记忆读写 |
| kissbot-memory-struct | 未开始 | 记忆索引框架 |
| kissbot-memory-struct-abstract | 未开始 | 摘要搜索实现 |
| kissbot-memory-manage | 未开始 | 记忆管理前端 |
| kissbot-agent-config | 未开始 | 配置管理前端 |
| kissbot-agent（Station） | 未开始 | Tool 执行主机 |

## 关键流程实现状态

| 流程 | 状态 | 说明 |
|------|------|------|
| 消息上行（外部 → nexus） | 已完成 | channel-web → ChannelManager → nexus 完整链路 |
| 消息下行（nexus → 外部） | 已完成 | nexus → ChannelManager → channel-web 完整链路 |
| agentic loop | 已完成 | ContextBuilder + LLMClient 完整实现 |
| nexus 绑定 channel | 已完成 | WSClient 绑定协议 |
| 记忆存储推送 | 已完成 | memory-store API、MemoryStoreClient、MemoryWriter |
| 自我认知读取 | 已完成 | Nexus 启动时从 memory-ego 加载设定 |
| 上下文重置 | 已完成 | 管理命令和自动超长触发 |
| Group 变化通知 | 已完成 | channel-web → nexus |
| 附件上传下载 | 已完成 | channel-web 附件系统 |
| tool 调用（nexus ↔ station） | 未开始 | Station 模块待实现 |
| 内置记忆查询 tool | 未开始 | 依赖 memory-struct 和 ToolCallDispatcher |
| 自主触发主动行为 | 未开始 | 空闲检测、自主目标加载 |
