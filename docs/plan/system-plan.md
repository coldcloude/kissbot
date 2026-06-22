# 组件和流程的实现顺序规划

## 实现阶段总览

| 阶段 | 内容 | 状态 |
|------|------|------|
| 第1阶段 | 核心模块初始化 | ✅ 已完成 |
| 第2阶段 | memory 基础模块实现（含角色/事件记忆路径） | ✅ 已完成 |
| 第3阶段 | memory-store 实现 | ✅ 已完成 |
| 第4阶段 | kissbot-security 模块实现 | 🟡 待实现 |
| 第5阶段 | channel 实现 | 🟡 部分完成 |
| 第6阶段 | memory-ego 实现 | 🟡 部分完成 |
| 第7阶段 | agent 基础实现（nexus + station 模块） | ❌ 未开始 |
| 第8阶段 | memory-struct 实现 | ❌ 未开始 |
| 第9阶段 | agent 记忆模式进阶实现 | ❌ 未开始 |
| 第10阶段 | agent 工具集实现 | ❌ 未开始 |
| 第11阶段 | UI 实现 | 🟡 部分完成 |
| 第12阶段 | agent 扩展（station 设备等） | ❌ 未开始 |

## 各阶段详细说明

### 第1阶段：核心模块初始化 ✅ 已完成
- [x] kissbot-api Rust 项目初始化（6 个源文件，已完成 trait 定义和 API 类型）
- [x] kissbot-memory Rust 项目初始化（6 个源文件，已完成）
- [x] kissbot-channel Rust 项目初始化（7 个源文件，框架代码基本完成）
- [x] kissbot-memory-store Rust 项目初始化（5 个源文件，已完成）
- [x] kissbot-memory-ego Rust 项目初始化（9 个源文件，大部分完成）
- [x] kissbot-memory-struct Rust 项目初始化（骨架）
- [x] kissbot-memory-struct-abstract Rust 项目初始化（骨架）
- [x] kissbot-channel-web Rust 项目初始化（骨架）
- [x] 所有前端项目（React + Vite）初始化：agent-config、memory-manage、channel-web-ui

### 第2阶段：memory 基础模块实现 ✅ 已完成
- [x] 模块架构设计（含角色记忆/事件记忆路径构造）
- [x] 定义记忆存储目录结构（year-suffix 单段目录名）
- [x] 提供基础库（DirectoryManager、MemoryIndexer、PathBuilder）供其他记忆模块使用
- [x] 实现记忆原文按时间索引搜索

### 第3阶段：memory-store 实现 ✅ 已完成
- [x] 模块架构设计
- [x] 四种记录类型的存储（JSON Lines 格式）：channel 文本、思考内容、工具调用、工具结果
- [x] 记忆推送 HTTPS API 接口
- [x] WSS 通知服务器功能（已规划，代码中待验证是否完成）
- [x] 记忆查询 API

### 第4阶段：kissbot-security 模块实现 🟡 部分完成
- [x] 模块架构设计、Cargo.toml 配置
- [x] auth_types 模块（Error、header 常量、extract_api_key）
- [x] validator 模块（ApiKeyValidator trait、SimpleApiKeyValidator 实现）
- [x] axum_middleware 模块（auth_middleware 函数，基于 axum::middleware::from_fn）
- [x] kai-ws 集成（ApiKeyWsFilter 实现 kai-ws 的 WsHeaderFilter trait）
- [x] 各进程接入安全认证（kissbot-memory-store、kissbot-memory-ego、kissbot-channel）
- [ ] 完善文档和测试

### 第5阶段：channel 实现 🟡 部分完成
- [x] 模块架构设计
- [x] 框架 trait 定义（Messenger trait、Channel trait）
- [x] 与 agent WSS 通信（ChannelManager 建立 WSS 服务器处理 nexus 的连接、绑定、消息收发、附件传输）
- [x] memory-store 客户端通信（MemoryStoreClient，通过 HTTPS 推送消息到记忆存储）
- [x] 附件存储管理
- [x] channel-web 后台实现（具体 Messenger/Channel 实现）
- [x] channel-web-ui 前台实现

### 第6阶段：memory-ego 实现 🟡 部分完成
- [x] 模块架构设计
- [x] 实现 agent 元数据 JSON 文件存储（带读写锁）
- [x] agent 元数据管理 API（新建、查询、更新）
- [x] HTTPS API 接口
- [x] 个体识别信息、角色设定的 JSON 管理模块
- [x] 个体识别信息、角色设定的查询 API
- [x] 全文搜索实现（kai-index 库）
- [ ] 在 AgentMetadata 中增加 forbidden_items 和 autonomous_goals 字段
- [ ] 在 role-play 数据结构中增加 autonomous_goals 字段
- [ ] 实现更新禁止事项和自主运行目标的功能和 API
- [x] 文件命名从 role-id 迁移至 role-name

### 第7阶段：agent 基础实现（nexus + station 模块） 🔴 未开始
- [ ] 模块架构设计、Cargo.toml 配置
- [ ] 启动模式管理（nexus 模式/station 模式/全模式）
- [ ] **Nexus 模块**
  - [ ] LLM API 集成（LLMClient）
  - [ ] WSS 服务器与 channel 集成（ExternalInputHandler、WSSServer）
  - [ ] HTTPS 客户端集成（StationRouter）
  - [ ] 基础 agentic loop 实现
  - [ ] 内置记忆查询 tool 实现（直接调用 memory-struct）
  - [ ] ToolCallDispatcher（内置工具 vs 外置工具分派）
  - [ ] MemoryReader / MemoryWriter（角色记忆、事件记忆路径构造）
  - [ ] 与 memory-store 集成（推送/读取记忆）
  - [ ] 与 memory-ego 集成（读取自我认知设定）
- [ ] **Station 模块**
  - [ ] HTTPS 服务器实现（HTTPServer）
  - [ ] ToolRegistry 实现（工具注册和注销）
  - [ ] ToolExecutor 实现（工具查找和调用）
  - [ ] 工具注册信息协议（station → nexus 注册消息）
  - [ ] tool call / tool result 消息协议

### 第8阶段：memory-struct 实现 🔴 未开始
- [ ] 模块架构设计
- [ ] 框架 trait 定义
- [ ] memory-store 实现向 memory-struct 的 WSS 通知机制
- [ ] memory-struct-abstract 实现（摘要搜索）
- [ ] 摘要搜索记忆 HTTPS API（供 nexus 内置 tool 调用）

### 第9阶段：agent 记忆模式进阶实现 🔴 未开始
- [ ] 角色记忆模式完整实现（按 role-name 读取/写入）
- [ ] 事件记忆模式完整实现（按 role-name-event-id 读取/写入）
- [ ] 上下文压缩和重置功能
- [ ] 自主运行目标触发机制

### 第10阶段：agent 工具集实现 🔴 未开始
- [ ] station 工程工具实现（Read、Write、Edit、Bash）
- [ ] station 网络工具实现（WebSearch、WebFetch）
- [ ] station 配置加载和管理

### 第11阶段：UI 实现 🟡 部分完成
- [ ] agent-config（配置 agent 的 nexus/station 启动模式）
- [ ] memory-manage（管理记忆）
- [x] channel-web-ui 完善

### 第12阶段：agent 扩展（station 设备等） 🔴 未开始
- [ ] 轻量级 HTTPS 协议适配（资源受限设备）
- [ ] 设备工具站开发框架
- [ ] 典型设备工具实现（网络设备、智能家电、机器人原型）

## 关键流程实现状态

| 流程 | 状态 |
|------|------|
| 消息上行（外部 → nexus） | 🟡 channel-web 已实现 Web UI → WebMessenger → ChannelManager 通道，依赖 nexus 接入完成完整链路 |
| 消息下行（nexus → 外部） | 🟡 channel-web 已实现 ChannelManager → WebChannel → SSE → Web UI 通道，依赖 nexus 接入完成完整链路 |
| agentic loop | ❌ 未实现 |
| tool 调用（nexus ↔ station） | ❌ 未实现 |
| nexus 绑定 channel | ❌ 未实现 |
| 记忆存储（推送至 memory-store） | ✅ 已有推送 API，channel 的 MemoryStoreClient 已实现 |
| 内置记忆查询 tool | ❌ 未实现 |
| 自我认知读取（nexus 查询 memory-ego） | ✅ memory-ego API 已实现，但 nexus 未集成 |
| 上下文重置 | ❌ 未实现 |
| 自主触发主动行为 | ❌ 未实现 |
| Group 变化通知 | ✅ channel-web 已实现 GroupChangeHandler 回调 |
| 附件下载 | ✅ channel-web 已实现附件上传/下载/缩略图 API |
