# kissbot-agent 组件内功能实现顺序

## 实现状态：🔴 未开始

Agent 组件包含 nexus 和 station 两个内部模块，启动时可选择启用 nexus 模式、station 模式或全模式。

### 第1阶段：基础结构搭建 ❌ 未开始
- [ ] 配置 Cargo.toml，添加依赖
- [ ] 定义模块结构（nexus/、station/）
- [ ] 定义错误类型和核心数据结构
- [ ] 启动模式管理（nexus 模式 / station 模式 / 全模式）

### 第2阶段：Nexus — LLM 集成 ❌ 未开始
- [ ] 实现 LLMClient
- [ ] LLM API 调用封装（多种提供商支持）
- [ ] 支持流式/非流式输出
- [ ] 请求重试和超时管理

### 第3阶段：Nexus — 通道通信 ❌ 未开始
- [ ] 实现 WSSServer（通道连接）
- [ ] 实现绑定协议（bind/bind_ack）
- [ ] 实现上行消息接收和下行消息发送
- [ ] 实现附件下载
- [ ] 心跳检测

### 第4阶段：Nexus — Station 通信 ❌ 未开始
- [ ] 实现 WSSClient（连接 Station 模块）
- [ ] 实现 StationRouter（工具路由表）
- [ ] 实现 tool call 发送和 tool result 接收协议
- [ ] 多 Station 连接管理
- [ ] Station 断开检测和路由表更新

### 第5阶段：Nexus — ToolCallDispatcher ❌ 未开始
- [ ] 内置工具识别（记忆查询等）
- [ ] 外置工具分派到 Station 模块
- [ ] 与 StationRouter 集成
- [ ] 工具调用超时处理

### 第6阶段：Nexus — Agentic Loop ❌ 未开始
- [ ] 实现 ContextBuilder（系统消息构建、记忆记录整合）
- [ ] 完整的 agentic loop 流程
- [ ] 上下文超长检测和压缩

### 第7阶段：Nexus — 记忆交互 ❌ 未开始
- [ ] 实现 MemoryReader（角色记忆/事件记忆路径构造）
- [ ] 与 memory-store 集成读取
- [ ] 实现 MemoryWriter（推送 tool call、tool result、思考内容）
- [ ] 内置记忆查询 tool（直接 HTTPS 调用 memory-struct，不记入记忆，不经由 station）

### 第8阶段：Nexus — 自我认知集成 ❌ 未开始
- [ ] 与 memory-ego 的 HTTPS 通信
- [ ] 启动时读取自我认知设定
- [ ] 系统消息中整合客观设定和角色设定

### 第9阶段：Nexus — 高级功能 ❌ 未开始
- [ ] 上下文重置流程
- [ ] 自主行为触发机制（空闲检测、自主目标加载）

### 第10阶段：Station — 基础框架 ❌ 未开始
- [ ] 实现 WSSServer（接收 nexus 的 tool call）
- [ ] 多 nexus 并行连接管理
- [ ] 工具注册信息发送（station → nexus）
- [ ] 实现 ToolRegistry（工具定义、注册、查找）
- [ ] 实现 ToolExecutor（同步/异步执行、错误处理）

### 第11阶段：Station — 工程工具 ❌ 未开始
- [ ] 文件操作工具（Read、Write、Edit）
- [ ] 命令执行工具（Bash）
- [ ] 工作区目录绑定
- [ ] 安全限制（路径白名单、命令黑名单）

### 第12阶段：Station — 网络工具 ❌ 未开始
- [ ] Web 搜索工具（WebSearch）
- [ ] 网页抓取工具（WebFetch）

### 第13阶段：Station — 设备站支持 ❌ 未开始
- [ ] 精简版 WSS 协议（资源受限设备适配）
- [ ] 设备工具注册规范

### 第14阶段：测试和完善 ❌ 未开始
- [ ] 单元测试
- [ ] 集成测试（与 memory-store、memory-ego、memory-struct、channel）
- [ ] 性能优化
