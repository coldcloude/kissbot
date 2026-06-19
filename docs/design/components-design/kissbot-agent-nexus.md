# kissbot-agent-nexus 模块设计

## 概述
Nexus — Agent 组件的 LLM 通信枢纽模块。智能体的"思考"部分，负责将外部输入和记忆加工为 LLM 可用的上下文，通过 agentic loop 调用 LLM 执行操作、生成回复。若 LLM 输出包含 tool call，则将 tool call 分派到同 Agent 内的 Station 模块执行。

Nexus 是 Agent 组件的一部分，通过内部调用与同进程 station 通信，通过 HTTPS 与远程 station 通信。Agent 启动时可选择是否启用 nexus 模块。一个系统内可运行多个 agent 实例，每个实例可独立选择是否启用 nexus。

## 核心功能

1. **LLM 交互**：封装 LLM API 调用，支持多种 LLM 提供商和模型
2. **上下文管理**：根据记忆模式从记忆系统读取历史记录，构建完整的 LLM 上下文（系统消息 + 记忆 + 输入）
3. **Tool 调用分派**：解析 LLM 返回中的 tool call，区分内置工具和外置工具，将外置工具通过 StationRouter 分派到对应 Station 执行，执行结果送回 LLM 继续 loop

## 内部模块

### 1. LLMClient - LLM 客户端
- 封装 LLM API 调用（支持多种 LLM 提供商和模型）
- 维护 LLM API 配置（endpoint、API key、model、参数等）
- 支持流式/非流式输出
- 管理请求重试和超时

### 2. ContextBuilder - 上下文构建器
- 根据记忆模式（角色记忆/事件记忆）从记忆系统读取历史记录
- 构建完整的 LLM 上下文：
  - 系统消息（从 memory-ego 加载或由配置指定）
  - 记忆记录（从 memory-store 读取的近期历史）
  - 当前输入消息（来自外部通道或自主触发）
- 上下文超长时执行压缩或裁剪

### 3. ToolCallDispatcher - Tool 调用分派器
- 解析 LLM 返回中的 tool call
- 区分两类工具：
  - **内置工具**：由 nexus 自身执行，不发送 station，不记入记忆（如记忆查询工具）
  - **外置工具**：通过 StationRouter 查找目标 Station，分派执行
- 接收 Station 返回的执行结果
- 将结果送回 LLM 继续 agentic loop

### 4. MemoryReader - 记忆读取器
- 从 memory-store 读取近期历史记录
- 拼接记忆标识后缀，调用记忆基础模块的路径构造接口获取完整路径后读取

### 5. MemoryWriter - 记忆写入器
- 在 agentic loop 外，将交互记录推送到 memory-store
- 推送内容：外部输入、思考内容、tool call 调用记录、tool result 记录
- 内置工具（如记忆查询）的调用和结果不记入记忆

### 6. ExternalInputHandler - 外部输入处理器
- 通过 WSS 接收来自消息通道的消息
- 接收来自定时器/API 的自主触发输入
- 将输入加入处理队列
- 管理并行输入的有序处理

### 7. StationRouter - Station 路由表
- 维护已连接 Station 的列表和每 Station 注册的工具列表
- 收到 Station 的连接/断开通知时更新路由表
- 提供按工具名称查找 Station 的查询接口
- 内置工具不经过路由，由 ToolCallDispatcher 直接处理

### 8. StationClient - Station 通信客户端
- 向 Station 发起 HTTPS 请求（tool name + parameters）
- 从响应中获取 tool call 结果
- 管理 Station 的地址和超时配置

### 9. WSSServer - WSS 服务器（连接外部）
- 作为 WSS 服务器，接受消息通道的连接
- 接收来自通道的上行消息
- 发送下行消息（回复、绑定请求等）
- 支持心跳检测

## 功能流程

### 记忆模式
Nexus 确定使用哪种记忆模式，这决定了上下文的构建方式和记忆的存储路径：

- **角色记忆** — 按角色标识组织所有历史记录
  - 所有该角色的历史记录按年月日组织在统一目录下
  - 用于需要持久身份的自主场景
  - nexus 读取该角色最近的若干条记录构建上下文
- **事件记忆** — 按事件标识隔离上下文
  - 每次事件（对话/工程任务）拥有独立的存储目录
  - 用于离散任务场景
  - nexus 读取该角色该事件的全部记录构建上下文

### Agentic Loop 流程
```
1. 外部输入到达（来自通道或定时器）
2. MemoryReader 根据记忆模式读取历史记录
3. ContextBuilder 构建 LLM 上下文（系统消息 + 记忆 + 当前输入）
4. LLMClient 调用 LLM API
5. 处理 LLM 返回：
   ├─ 有 tool call → 按 Tool 调用流程处理，完成后回到步骤 3（继续 LLM 交互）
   └─ 无 tool call →
         1. MemoryWriter 推送思考内容到 memory-store
         2. 回复发送到外部通道
6. 等待下一条输入
```
目的：将输入加工为 LLM 可用的上下文，循环调用 LLM 执行操作直至生成回复。过程中 tool call 分派到 Station 执行，所有记忆操作（包括 tool 结果）由 nexus 统一推送到记忆系统。

### Tool 调用流程
```
nexus 收到 LLM 返回中的 tool call
  → MemoryWriter 推送 tool call 调用记录到 memory-store
  → ToolCallDispatcher 判断工具类型：
     ├─ 内置工具（如记忆查询）→
     │     1. Nexus 直接调用记忆结构实现组件的 API
     │     2. 不记入记忆
     │     3. 将结果加入上下文
     └─ 外置工具 →
           1. StationRouter 查找目标 Station
           2. 向 Station 发送请求（tool name + parameters）
           3. Station 接收请求，执行工具
           4. 将执行结果作为响应返回
           5. Nexus 收到响应，MemoryWriter 推送 tool result 记录到 memory-store
           6. 将结果加入上下文
  → 继续步骤 3（继续 LLM 交互）
```
目的：nexus 将 LLM 需要的工具调用分派到对应 station 执行，station 的响应中携带执行结果。tool call 和 tool result 各自保存一条记忆记录。

### 记忆查询流程（nexus 内置 tool 查询记忆结构）
```
nexus 在 agentic loop 内
  → LLM 生成 tool call（调用记忆结构组件的搜索工具）
  → ToolCallDispatcher 识别为内置工具（不发送 station，不记入记忆）
  → Nexus 直接调用记忆结构实现组件的 API
  → 记忆结构实现组件从自己的索引中检索记忆
  → 返回结构化的记忆片段
  → nexus 将记忆片段加入当前上下文继续处理
```
记忆查询是 nexus 的内置工具，不经由 station 执行，且调用和结果均不写入记忆存储组件。
目的：nexus 在对话过程中自主检索历史记忆，用于辅助当前对话。

### 自我认知读取流程
nexus 启动或上下文重置时 → （在 agentic loop 外）查询自我认知组件 → 读取 agent 的客观设定（身份标识、用户识别信息）→ 如果配置了角色设定，按 role-name 读取对应的角色扮演信息 → 将获取的自我认知信息构建到系统消息中 → 进入 agentic loop。

### 上下文重置流程
触发条件：上下文超长 / 长时间无消息 / 长时间未重置 → MemoryWriter 将当前所有消息存入 memory-store → 清除当前上下文 → MemoryReader 根据当前记忆模式重新读取近期记忆 → ContextBuilder 用读取到的记忆重建上下文 → 继续 agentic loop。

### 自主行为触发流程
无外部输入的空闲状态 → 空闲计时器超过配置的超时时间 → 加载自主运行目标（从 memory-ego 获取）→ 根据目标进行信息收集或输出 → 将结果推送到记忆存储模块 / 发送到消息通道 → 回到空闲状态。
