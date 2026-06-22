# kissbot-agent-nexus 模块设计

## 概述
Nexus — Agent 组件的 LLM 通信枢纽模块。智能体的"思考"部分，负责将外部输入和记忆加工为 LLM 可用的上下文，通过 agentic loop 调用 LLM 执行操作、生成回复。若 LLM 输出包含 tool call，则将 tool call 分派到同 Agent 内的 Station 模块执行。

Nexus 是 Agent 组件的一部分，通过 WS 连接与消息通道通信，通过 HTTPS 与 memory-store、memory-ego、station 通信。Agent 启动时可选择是否启用 nexus 模块。一个系统内可运行多个 agent 实例，每个实例可独立选择是否启用 nexus。

## 核心功能

1. **外部输入处理**：通过 WS 连接接收来自消息通道的消息，区分管理命令和普通消息，分别路由
2. **LLM 交互**：封装 LLM API 调用，支持多种 LLM 提供商和模型，构建完整的 LLM 上下文并调用
3. **记忆管理**：在 agentic loop 中将 think、tool call、tool result 写入记忆系统；在启动、模式切换、上下文重置时从记忆系统读取历史记录
4. **工具调用分派**（规划中，本期不实现）：解析 LLM 返回中的 tool call，分派到对应 Station 执行

## 内部模块

### 1. AgentCoordinator - 协调器
- 核心调度层，统一管理 nexus 所有入口（外部输入、定时器、REST API）
- 管理 nexus 生命周期（启动、模式切换、上下文重置）
- 将外部输入路由到命令处理或 agentic loop

### 2. ConfigManager - 配置管理
- 提供管理命令和 REST API 双入口统一修改配置
- 管理 LLM API 配置、channel 绑定列表、管理权限用户列表、当前角色、当前模式、station 地址列表
- 配置变更通知相关模块更新状态

### 3. LLMClient - LLM 客户端
- 封装 LLM API 调用（支持多种 LLM 提供商和模型）
- 维护 LLM API 配置（endpoint、API key、model、参数等）
- 支持流式/非流式输出
- 管理请求重试和超时

### 4. ContextBuilder - 上下文构建器
- 管理内存中的完整 LLM 上下文
- 在启动/切换时接收 MemoryReader 读取的历史记录构建初始上下文
- 运行时增量追加用户消息、think、tool call、tool result
- 构建时自动包含系统消息（从 memory-ego 加载的自我认知设定和角色信息）
- 保存已发送消息内容，用于 is_self=1 消息的识别丢弃
- 上下文超长时触发上下文重置

### 5. MemoryReader - 记忆读取器
- 在启动、模式切换、角色切换、上下文重置时调用
- 按当前模式（角色/事件）从 memory-store 读取最近若干条历史记录
- 角色模式下读取该角色所有记录，事件模式下只读本事件记录
- （规划，本期不实现）对接 memory-struct 的记忆搜索工具调用

### 6. MemoryWriter - 记忆写入器
- 写入队列 + 后台写入任务
- agentic loop 中 LLM 返回后，将 think、tool call、tool result 推送到写入队列
- 后台任务从队列消费，通过 HTTPS 写入 memory-store
- 写入失败不重试，记录日志
- 用户消息和 agent 的回复由消息通道组件推送到 memory-store，不由 MemoryWriter 处理

### 7. CommandRouter - 管理命令路由
- 识别以 "/" 开头的外部消息
- 检查发送者是否在管理权限用户列表中
- 解析命令类型（bind/admin/mode/role/reenter/events/reset 等）
- 调用对应处理器执行命令

### 8. ModeManager - 模式管理器
- 维护当前模式状态（角色模式/事件模式）
- 默认角色模式
- 管理命令可切换模式，切换时通知 AgentCoordinator 触发上下文重建
- 角色模式可查看所有记录，事件模式只查看本事件记录

### 9. StationRouter - Station 路由表
- 从 ConfigManager 读取已配置的 Station 列表
- 提供按 Station ID 查找地址的查询接口
- （规划，本期仅骨架）

### 10. StationClient - Station 通信客户端
- 向 Station 发起 HTTP/HTTPS 请求（tool name + parameters）
- 从响应中获取 tool call 结果
- 管理 Station 的连接超时配置
- （规划，本期仅骨架）

### 11. WSClient - WS 通信客户端
- 作为 WS 客户端连接消息通道的 WSS 服务器
- 同时支持 ws:// 和 wss://（由 URL 决定）
- 连接后获取 MessengerInfo 并发送 BindRequest 绑定用户
- 可连接多个通道（不同 messenger_id），每个连接独立
- 上行消息送入 ExternalInputHandler，下行消息按 messenger_id 选择对应连接发送
- 支持心跳检测和断线自动重连

### 12. ToolCallDispatcher - 工具调用分派器（规划，本期不实现）
- 解析 LLM 返回中的 tool call
- 区分内置工具和外置工具
- 内置工具由 nexus 自身执行，不发送 station，不记入记忆
- 外置工具通过 StationRouter 查找目标 Station，分派执行

## 功能流程

### 外部输入处理流程

```
WSClient 收到消息
  → 外部输入中检查 message_id 和 group_id 是否在绑定群组列表中
    ├─ 否 → 丢弃
    └─ 是 → 继续
  → 检查 is_self
    ├─ 1（自己发出的消息回显）→ 对比发送记录确认后丢弃
    └─ 0 → 进入命令检查
  → CommandRouter 检查是否以 "/" 开头
    ├─ 是 → 检查发送者是否在 admin_users 中
    │   ├─ 是 → 执行管理命令，回复执行结果
    │   └─ 否 → 回复无权限
    └─ 否 → 进入 agentic loop
```

### 启动流程

```
kissbot-agent 启动
  → 加载配置文件，初始化 ConfigManager
  → 初始化 ModeManager（默认角色模式）
  → 初始化 MemoryWriter（启动后台写入任务）
  → 初始化 MemoryReader
  → 初始化 ContextBuilder
  → 初始化 LLMClient
  → 初始化 WSClient（连接所有配置的 channel，依次获取 MessengerInfo 并绑定用户）
  → MemoryReader 按当前模式读取历史
  → ContextBuilder 用历史 + Ego 信息构建初始上下文
  → AgentCoordinator 进入就绪状态
```

### Agentic Loop 流程（本期）

```
普通消息 → 追加到 ContextBuilder 的当前上下文
  → LLMClient 调用 LLM API（传入当前完整上下文）
  → LLM 返回回复
    → MemoryWriter 队列推送 think
    → WSClient 发送回复到消息通道
  → 等待下一条输入
```

### 上下文重置流程

```
触发条件：/reset 命令 / 上下文超长
  → MemoryWriter flush 当前剩余内容到 memory-store
  → MemoryReader 按当前模式重新读取最近记录
  → ContextBuilder 重建上下文
  → AgentCoordinator 继续运行
```

### 模式切换流程

```
管理命令切换模式（/mode event、/mode role、/reenter）
  → ModeManager 更新当前模式
  → ConfigManager 持久化模式状态
  → 触发上下文重置流程
```

### 事件管理流程

```
/mode event → 生成新 event-id，切换为新事件模式，重建上下文
/reenter <event-id> → 切换到指定事件模式，重建该事件上下文
/events → 查询所有事件列表（通过 memory-store）
/mode role → 回到角色模式，重建上下文
```

## 关键设计

### 记忆模式隔离规则

- 角色模式：MemoryReader 读取该角色下所有记录（包括各事件期间的记录）
- 事件模式：MemoryReader 只读取本事件的记录
- 切换模式时重新读取并重建上下文

### 管理命令权限

- 管理权限用户列表（admin_users）以 (messenger_id, user_id) 标识
- 管理命令只对被 admin_users 列表中的用户发送的 "/" 前缀消息响应
- 绑定/解绑、管理权限的增删均可通过管理命令在线修改

### is_self 消息处理

- is_self=1 的消息是 agent 自己发出的消息经通道返回的回显
- 通过 ContextBuilder 保存的已发送记录对比确认后丢弃
- 相关记忆已由 MemoryWriter 写入，无需额外处理

### 通信方式

| 目标 | 协议 | 角色 | 说明 |
|------|------|------|------|
| 消息通道 | WS/WSS | 客户端 | 实时消息收发，URL 决定 ws/wss |
| Station | HTTP/HTTPS | 客户端 | tool call 分派，URL 决定 http/https |
| Memory-Store | HTTP/HTTPS | 客户端 | 记忆读写，URL 决定 http/https |
| Memory-Ego | HTTP/HTTPS | 客户端 | 自我认知读取，URL 决定 http/https |
| 管理 REST API | HTTP | 服务器 | 管理界面对接，由 kissbot-agent 启动 |

### 后期规划

- ToolCallDispatcher 实现：LLM 的多轮 tool call 处理、内置工具（记忆查询）和外置工具（Station）的分派
- AgentCoordinator 对接自主行动入口：定时器和 REST API 触发
- 事件搜索功能
