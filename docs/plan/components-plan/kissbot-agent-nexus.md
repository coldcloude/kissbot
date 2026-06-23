# kissbot-agent-nexus 模块实现计划

Nexus 是 [kissbot-agent](kissbot-agent.md) 组件的内部模块，实现计划已合并至 agent 计划。

详见 [kissbot-agent 实现计划](kissbot-agent.md) 第 2~9 阶段（Nexus 相关）。

## 实现状态

### 第1阶段：基础结构搭建 ✅ 已完成
- [x] 配置 Cargo.toml，添加依赖
- [x] 定义模块结构（nexus/、station/）
- [x] 定义错误类型和核心数据结构

### 第2阶段：Nexus — LLM 集成 ✅ 已完成
- [x] 实现 LLMClient
- [x] LLM API 调用封装（多种提供商支持）
- [x] 非流式/流式输出封装
- [x] 请求重试和超时管理

### 第3阶段：Nexus — 通道通信 ✅ 已完成
- [x] 实现 WSClient（作为客户端连接消息通道）
- [x] 实现绑定协议（MessengerInfo → BindRequest）
- [x] 实现上行消息接收和下行消息发送
- [x] 心跳检测
- [x] WS/WSS 支持（根据 URL 决定）

### 第4阶段：Nexus — Station 通信 ✅ 已完成（骨架）
- [x] 实现 HTTP 客户端（连接 Station 模块）
- [x] 实现 StationRouter（工具路由表）
- [ ] 实现 tool call 发送和 tool result 接收协议（待 ToolCallDispatcher 接入）
- [ ] 多 Station 连接管理（待 ToolCallDispatcher 接入）
- [ ] Station 断开检测和路由表更新（待动态注册）

### 第5阶段：Nexus — ToolCallDispatcher 🔲 未开始
- [ ] 内置工具识别（记忆查询等）
- [ ] 外置工具分派到 Station 模块
- [ ] 与 StationRouter 集成
- [ ] 工具调用超时处理

### 第6阶段：Nexus — Agentic Loop ✅ 已完成
- [x] 实现 ContextBuilder（系统消息构建、记忆记录整合）
- [x] 完整的 agentic loop 流程
- [x] 上下文超长检测和自动重置

### 第7阶段：Nexus — 记忆交互 ✅ 已完成
- [x] 实现 MemoryReader（角色记忆/事件记忆路径构造，读取顶层记忆索引）
- [x] 与 memory-store 集成读取
- [x] 实现 MemoryWriter（推送 tool call、tool result、思考内容）
- [ ] 内置记忆查询 tool（通过 tool call 调用 memory-struct，待 ToolCallDispatcher）

### 第8阶段：Nexus — 自我认知集成 ✅ 已完成
- [x] 与 memory-ego 的 HTTP 通信
- [x] 启动时读取自我认知设定
- [x] 系统消息中整合客观设定和角色设定

### 第9阶段：Nexus — 高级功能 ✅ 已完成
- [x] 上下文重置流程
- [x] 管理命令（bind/admin/role/mode/reenter/events/reset）
- [x] 配置管理器（JSON 配置文件加载/持久化/变更通知）
- [x] 模式管理器（角色/事件模式切换）
- [x] 协调器（核心调度、生命周期管理）
- [ ] 自主行为触发机制（空闲检测、自主目标加载）🔲 未开始
- [ ] 事件搜索 🔲 未开始
- [ ] 管理 API 路由完善（当前为骨架）🔲 未开始

### 第10阶段：Station — 基础框架 🔲 未开始
- [ ] 实现 HTTPServer（处理 nexus 的 tool call 请求）
- [ ] 多 nexus 并行连接管理
- [ ] 工具注册信息发送（station → nexus）
- [ ] 实现 ToolRegistry（工具定义、注册、查找）
- [ ] 实现 ToolExecutor（同步/异步执行、错误处理）

### 第11阶段：Station — 工程工具 🔲 未开始
- [ ] 文件操作工具（Read、Write、Edit）
- [ ] 命令执行工具（Bash）
- [ ] 工作区目录绑定
- [ ] 安全限制（路径白名单、命令黑名单）

### 第12阶段：Station — 网络工具 🔲 未开始
- [ ] Web 搜索工具（WebSearch）
- [ ] 网页抓取工具（WebFetch）

### 第13阶段：Station — 设备站支持 🔲 未开始
- [ ] 精简版 HTTPS 协议（资源受限设备适配）
- [ ] 设备工具注册规范

### 第14阶段：测试和完善 🔲 未开始
- [ ] 单元测试
- [ ] 集成测试（与 memory-store、memory-ego、memory-struct、channel）
- [ ] 性能优化
