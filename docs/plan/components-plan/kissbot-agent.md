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
- [ ] 实现外置工具接入（整合 StationRouter 到 ToolCallDispatcher）
- [ ] 实现自主行为触发机制（空闲检测、自主目标加载）
- [ ] 完善管理 API 路由

## Station 模块

**实现状态：未开始**

- [ ] 配置 Cargo.toml，定义模块结构
- [ ] 实现 HTTP 服务器：接收 nexus 的 tool call 请求
- [ ] 实现多 nexus 并行连接管理
- [ ] 实现工具注册信息发送（station → nexus）
- [ ] 实现 ToolRegistry：工具定义、注册、查找
- [ ] 实现 ToolExecutor：同步/异步执行、错误处理
- [ ] 实现工程工具：文件操作（Read、Write、Edit）、命令执行（Bash）
- [ ] 实现网络工具：WebSearch、WebFetch
- [ ] 实现设备站支持：精简版协议、设备工具注册规范
- [ ] 完善测试：单元测试、集成测试、性能优化
