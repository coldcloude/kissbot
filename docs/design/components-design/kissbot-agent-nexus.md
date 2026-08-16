# kissbot-agent-nexus 模块设计

## 概述
Nexus — Agent 组件的 LLM 通信枢纽模块。智能体的"思考"部分，负责将外部输入和记忆加工为 LLM 可用的上下文，通过agentic loop调用 LLM 执行操作、生成回复。若 LLM 输出包含 tool call，则将 tool call 分派到同 Agent 内的 Station 模块执行。

Nexus 是 Agent 组件的一部分，与消息通道建立实时连接进行消息收发，与 memory-store、memory-ego 和 station 通过请求-响应方式通信。Agent 启动时可选择是否启用 nexus 模块。一个系统内可运行多个 agent 实例，每个实例可独立选择是否启用 nexus。

一个 nexus 可同时管理多个会话。nexus 为每个绑定的 channel 配置 agent_name、role_name、mode，并将所有绑定项的信息去重，每个 agent_name+role_name+mode 组合为一个会话；各会话拥有独立的 LLM 上下文、记忆读取范围与模式状态，并从绑定它的多个 channel 中选定一个作为回复 channel，回复消息通过该 channel 发送。

## 核心功能

### Epic 1：外部输入处理
- **目标**：接收来自外部消息、定时器和 API 三种入口的输入，按消息来源 channel 对应的绑定配置定位会话，区分并路由管理命令和普通消息到对应处理流程
- **依赖**：Epic 5（消息通道通信）
- **用户故事**：
  - 作为一个用户，我要向智能体发送消息，以丢弃来自非绑定群组或智能体自身发出的无效消息
  - 作为一个管理员，我要向智能体发送管理命令，以让智能体执行相应管理操作（无权限用户发送的命令被忽略）
  - 作为一个用户，我要向智能体发送普通消息，以触发智能体生成 LLM 回复

### Epic 2：LLM 交互
- **目标**：封装 LLM API 调用，支持多种 LLM 提供商和模型，构建完整的 LLM 上下文并调用
- **依赖**：Epic 3（上下文管理）、memory-ego（自我认知信息）
- **用户故事**：
  - 作为一个管理员，我要配置 LLM 调用的地址、密钥、模型和参数，以控制 LLM 的行为
  - 作为一个管理员，我要切换使用的 LLM 提供商，以使用不同的 AI 模型
  - 作为一个管理员，我要配置 LLM 调用失败时的重试策略，以提高请求成功率
  - 作为一个用户，我要向智能体发送消息，以获取智能体调用 LLM 生成的回复，并让智能体将回复发送到消息通道

### Epic 3：上下文管理
- **目标**：按会话管理内存中的 LLM 上下文，支持构建、增量追加和超长自动重置
- **依赖**：Epic 4（记忆读写）、memory-ego（自我认知信息）
- **用户故事**：
  - 作为一个管理员，我要启动智能体程序，以让智能体按会话恢复近期对话记忆并构建初始 LLM 上下文
  - 作为一个管理员，我要发送重置上下文命令，以让智能体重建对应会话的初始 LLM 上下文
  - 作为一个用户，我要向智能体发送消息，以让智能体保持对话连续性
  - 作为一个管理员，我要设置上下文消息数量上限，以防止上下文无限增长

### Epic 4：记忆读写
- **目标**：在会话创建或重置时按会话模式读取上下文来源（event 模式从本地缓存恢复、role 模式从 memory-store 读取最近对话消息）并从 memory-struct 读取顶层记忆索引；在agentic loop中将思考、tool call、tool result 写入记忆系统；将收发的通道消息写入记忆系统；各会话按自身模式（角色/事件）隔离记忆范围
- **依赖**：memory-store（记忆读写）、memory-struct（记忆索引）、memory-ego（自我认知信息）
- **用户故事**：
  - 作为一个用户，我要询问智能体过往的对话内容，以让智能体回顾对话脉络并获取完整的历史信息
  - 作为一个管理员，我要修改通道绑定的记忆模式（角色/事件），以让智能体按模式隔离记忆范围并重建对应会话上下文
  - 作为一个用户，我要向智能体发送消息，以持久化智能体的思考过程和工具调用记录
  - 作为一个用户，我要向智能体发送消息，以持久化完整的对话收发记录
  - 作为一个管理员，我要通过管理命令查询所有事件列表，以了解已记录的事件概况

### Epic 5：消息通道通信
- **目标**：作为客户端连接消息通道的服务端，支持多通道同时连接、心跳检测和断线重连
- **依赖**：kissbot-channel（消息通道服务端和绑定协议）
- **用户故事**：
  - 作为一个管理员，我要配置通道绑定列表（每个绑定项指定 agent_name、role_name、mode），以让智能体启动时连接配置的通道并绑定用户、建立对应会话，实现与多个消息通道的通信
  - 作为一个外部系统（消息通道），我要与智能体保持长连接，以保持通信的稳定性
  - 作为一个外部系统（消息通道），我要向智能体推送上行消息并接收其下行消息，以让智能体收发消息并获取发送结果

### Epic 6：工具调用分派
- **目标**：解析 LLM 返回中的 tool call，按工具名路由到全局 Station 执行（本地 Toolkit 进程内执行 / 直接子 Station HTTP 递归）
- **依赖**：Epic 2（LLM 交互）、Station 组件
- **用户故事**：
  - 作为一个用户，我要向智能体提出需要执行工具的任务，以触发智能体执行工具调用
  - 作为一个外部系统（工具执行站），我要接收智能体发送的工具调用请求，以执行外置工具并返回工具执行结果，完成工具调用
  - 作为一个用户，我要向智能体提出复杂的多步任务，以完成复杂的工具链调用

## 内部模块

### 1. 协调器
- 核心调度层，统一管理 nexus 所有入口（外部消息、定时器、API）
- 管理 nexus 生命周期（启动、会话上下文重置）
- 按消息来源 channel 对应的绑定配置定位会话，将外部输入路由到管理命令处理或该会话的agentic loop
- 在会话创建或重置时按会话的 agent_name 解析出 agent_id（UUID）后，按该 agent_id 和 role_name 从 memory-ego 加载自我认知信息设置为系统消息

### 2. 配置管理器
- 从配置文件加载所有配置
- 管理 LLM API 配置、通道绑定列表（每项含 channel 身份与 agent_name、role_name、mode）、管理权限用户列表、memory 组件地址（station 连接信息在 station.json 的 sub_stations，见 [kissbot-agent-station 技术规格](../../spec/kissbot-agent-station.md)）
- 支持运行时通过管理命令修改配置，修改后自动保存并通知监听器

### 3. LLM 客户端
- 封装 LLM API 调用（支持多种 LLM 提供商）
- 维护 LLM API 配置（地址、密钥、模型、参数等）
- 支持请求重试和超时控制
- 支持运行时热更新配置

### 4. 上下文构建器
- 按会话管理内存中的 LLM 上下文，消息为 OpenAI 格式的 Message 枚举（role 即枚举变体：system/user 含 content，assistant 含 content/reasoning_content/tool_calls，tool 含 tool_call_id/name/content）
- 每会话持有 SessionContext（内存消息序列 + system 消息），并同步写入本地缓存（`<data_dir>/context/` 下按 session_key 编码的 JSONL，追加不截断）；重置/压缩前的缓存快照归档到 `<data_dir>/context-history/`（本轮只写不读）
- 在会话创建/重置时按模式构建初始上下文：event 模式从缓存全量恢复（空则为仅 system），role 模式从记忆打包一条 user 消息
- 运行时按会话增量追加用户消息、助手回复与工具调用/结果消息，每步同步写入缓存
- 每个 channel 维护运行时 ChannelContext，记录「已发未收到回显的 outgoing msg_id 集合」（TTL 懒清理），用于识别自身发送的消息
- channel 合批：合批状态分为生产侧与消费侧——生产侧（BatchProducer，会话持有）含数据发送端与触发发送端（channel 在绑定会话时取得同源 clone）、合批截止时间（deadline，ArcSwapOption 无锁）与退出通知；收到普通消息后推入数据队列（元素为 IncomingMessageEvent）并更新 deadline 与发送触发时间（Trigger::At，绝对时刻）；消费侧（BatchConsumer，数据/触发接收端与定时队列）由 trigger 任务独占，随会话创建 move 进任务、任务内直接访问（零锁）；trigger 任务 select! 并行等待触发到达 / 定时到期 / 会话销毁通知，到期后按 deadline 判断（非强制）或直接（强制）从数据队列一次性读出全部，打包为一条 user 消息（仅保留 name 与 content）进入 agentic loop；上下文重置末尾发送强制触发（Trigger::Forced），重置期间到达的消息即刻并入新上下文；触发器队列与定时队列随会话生命周期，会话销毁时通知任务退出（任务经会话上的升级槽升级会话与协调器引用）
- 会话上下文消息数量超上限时按模式触发重置（event 压缩、role 归档重建）

### 5. 记忆读取器
- 在会话创建或重置时调用（role 模式）
- **两级读取**：
  - 第一级：从 memory-store 读取最近对话消息——两次查询并集：① RecentQuery（agent_id + role_name + count，无时间参数）取最近 N 条（不足 N 条直接返回全部），T_N = 最旧一条时间；② QueryRequest 时间范围 [M, T_N]（M = min(时间窗起点, T_N)，无 limit，M == T_N 时退化为单点取同时间组）取该段全部；结果 = ① ∪ ②（按 (time, sn) 去重后时间正序）
  - 第二级：从 memory-struct 读取顶层记忆索引（如摘要列表），提供长期记忆概况
- 读取结果打包为一条 user 消息（仅保留 name 与 content），作为 role 模式会话的首条内容
- 支持查询事件列表

### 6. 记忆写入器
- agentic loop中 LLM 返回后，将思考内容、工具调用指令、工具结果写入 memory-store
- 收到上行消息时，按 msg_id 查所属 channel 的未回显集合：命中（自身回显）则丢弃不写，未命中（非自身发送）后写入 memory-store（is_self=0）
- 发送下行消息后，用发送的返回值补充、替换发送消息的内容，再写入 memory-store
- 写入失败不重试，记录日志

### 7. 管理命令路由器
- 识别以特定前缀开头的外部消息（管理命令）
- 检查发送者是否在管理权限用户列表中
- 解析命令类型：绑定/解绑通道（携带 agent_name、role_name、mode）、管理权限管理、修改通道绑定的角色或模式、重新进入事件、列出事件、重置会话上下文
- 调用对应处理器执行命令

### 8. 会话管理器
- 汇总所有绑定 channel 的（agent_name、role_name、mode）信息并去重，维护会话集合
- 按消息来源 channel 对应的绑定配置定位会话
- 为每个会话从绑定它的多个 channel 中选定回复 channel，绑定信息变化时更新
- 维护各会话的模式状态（角色模式/事件模式），默认角色模式
- 绑定信息变化时更新会话集合，通知协调器创建或销毁会话上下文
- 事件模式会话自动生成新事件标识，支持按事件标识重新进入指定事件

### 9. Station 运行态
- 全局 Station 单例（每 agent 一个，`Station::get()`/`new()`）从 station.json 构建：本地 Toolkit 集合（工具实现表 + MCP 占位）+ 直接子 Station 集合（仅连接信息）
- 内置示例工具 Read：读取文本文件，路径经绝对路径规范化后校验位于当前工作目录内（防穿透），返回内容限长；配置显式声明 filesystem toolkit 名才注册
- 工具名整树全局唯一（本地硬约束 + 跨进程部署保证）

### 10. Station 工具执行
- 工具定义（name/description/parameters JSON Schema）存于 ToolkitConfig.tools（工具名 → 配置），由 nexus 按会话 context 配置的启用 toolkit 白名单聚合（`tools(filter)` 平铺递归）后随 LLM 请求发送
- 本地工具：按工具名在本地 Toolkit 工具表查找并执行（工具名全局唯一，未注册报错）；子 Station 工具：本地未命中查 tool_routes 路由缓存（工具名 → 直接子 station_id）调对应子（本轮骨架，返回未实现错误；不遍历全部子）

### 11. 通信客户端
- 作为客户端连接消息通道的服务端
- 连接后获取通道信息并发送绑定请求绑定用户
- 可连接多个通道（不同 messenger ID），每个连接独立维护
- 上行消息送入协调器，下行消息通过该会话回复 channel 对应的连接发送，发送结果返回给协调器
- 支持心跳检测和断线自动重连

### 12. 工具调用分派器
- 解析 LLM 返回中的 tool call（工具名 + 参数）
- 工具定义按会话 context 配置的启用 toolkit 白名单聚合（`Station::tools(filter)` 平铺递归），随 LLM 请求发送
- 工具调用经 `Station::call_tool` 按工具名执行（本地实现表 → tool_routes 路由缓存 → 对应子 HTTP），结果作为 tool 消息追加回上下文
- 支持多轮嵌套：LLM 返回 tool_calls 时执行后继续调用，直至返回无 tool_calls 的回复（上限防死循环）

### 13. 管理 API 服务器
- 绑定指定端口等待连接
- 供管理界面通过 API 对接配置管理

## 功能流程

### Epic 1 外部输入处理流程

```
通信模块收到消息
  → 检查消息来源和所属群组是否在绑定群组列表中
    ├─ 否 → 丢弃
    └─ 是 → 继续
  → 按 msg_id 查所属 channel 的未回显 outgoing 集合
    ├─ 匹配已发送记录 → 识别为自身发出的消息，丢弃（发送时已写入记忆，无需重复处理）
    └─ 非自身发送 → 记忆写入器将消息推送到写入队列，进入命令检查
  → 按消息来源 channel 对应的绑定配置（agent_name、role_name、mode）定位会话
  → 管理命令路由器检查是否以管理命令前缀开头
    ├─ 是 → 检查发送者是否在管理员列表中
    │   ├─ 是 → 对该会话执行管理命令，回复执行结果，如需要则触发该会话上下文重置
    │   └─ 否 → 忽略（不回复也不进入agentic loop）
    └─ 否 → 进入该会话的合批队列（channel 推入消息数据并发送触发时间），合批间隔后由会话触发器打包为一条 user 消息进入 agentic loop
```

### Epic 2+3 agentic loop流程

```
普通消息 → 合批队列（同一会话连续消息等待合批间隔后打包为一条 user 消息，仅保留 name 与 content）
  → 追加到该会话的上下文，每步同步写入本地缓存
  → LLM 客户端调用 LLM API（传入该会话的完整上下文 + 会话 toolkit 白名单聚合的工具定义）
  → LLM 返回 tool_calls
    → 逐个执行工具调用（全局 Station：本地 Toolkit 进程内执行 / tool_routes 路由缓存 → 对应子 HTTP 骨架），结果作为 tool 消息追加回上下文
    → 再次调用 LLM，循环直至返回无 tool_calls 的回复（上限防死循环）
  → LLM 返回最终回复
    → 记忆写入器推送思考内容到写入队列
    → 通信模块通过该会话的回复 channel 发送回复到消息通道
    → 记忆写入器推送回复到写入队列
  → 检查上下文消息数量是否超过上限（模型配置的 max_context_messages）
    ├─ event 模式 → 压缩：归档当前缓存，LLM 总结后重写为 system+user(压缩指令)+assistant(总结)
    └─ role 模式 → 重置：归档当前缓存，按记忆打包重建
  → 等待下一条输入
```

### 启动流程

```
kissbot-agent 启动
  → 加载配置文件，初始化配置管理器
  → 初始化会话管理器（汇总绑定信息去重，生成会话集合）
  → 初始化记忆写入器（启动后台写入任务）
  → 初始化记忆读取器
  → 初始化上下文构建器（含本地缓存与历史归档目录）
  → 初始化 LLM 客户端
  → 构建全局 Station 单例（读 station.json：本地 Toolkit + 直接子 Station 连接信息）
  → 初始化通信模块（连接所有配置的通道，获取通道信息并发送绑定请求）
  → 对每个会话按模式构建初始上下文：event 从本地缓存全量恢复（空则为仅 system）；role 从 memory-store 读取最近消息打包为一条 user 消息
  → 记忆读取器从 memory-struct 读取顶层记忆索引（memory-struct 未实现时跳过）
  → 上下文构建器用恢复/打包结果 + Ego 信息构建各会话的初始上下文
  → 协调器进入就绪状态
```

### Epic 3 上下文重置流程

```
触发条件：对某会话执行重置命令 / 该会话上下文超长 / 新 session_key（会话创建或重新进入）
  → 当前缓存快照归档到历史目录（context-history/，复制缓存文件）并清空缓存
  → 协调器按该会话的 agent_id 和 role_name 从 memory-ego 重新加载自我认知信息
  → 按模式重建：
    ├─ event：从缓存全量恢复（超长时先压缩：LLM 总结，新上下文为 system+user(压缩指令)+assistant(总结)）
    └─ role：从记忆打包一条 user 消息作为首条内容
  → 等待 channel 消息继续构造上下文
```

### Epic 4 事件管理流程

```
修改 channel 绑定为事件模式 → 会话管理器创建事件模式会话并自动生成新事件标识
  → 触发该会话上下文构建，读取当前事件上下文（空）且只记录本事件
重新进入指定事件 → 事件模式会话切换到指定事件标识
  → 从该事件会话的缓存恢复记录（若已有内容则以 assistant 消息结尾），重建该会话上下文
列出事件 → 通过 memory-store 查询所有事件列表
修改 channel 绑定回角色模式 → 消息进入角色模式会话，重建上下文
```

## 关键设计

### 会话定义与记忆模式隔离规则

- 会话由（agent_name、role_name、mode）唯一标识（agent_name 即绑定代号；memory-store/ego 读写用 agent_name 解析出的 agent_id UUID），nexus 将所有绑定 channel 的信息去重生成会话集合，多个绑定相同三元组的 channel 共享同一会话
- 每个会话从绑定它的多个 channel 中选定一个作为回复 channel，回复消息通过该 channel 发送
- 角色模式会话：记忆读取器读取该角色下所有记录（包括各事件期间的记录）
- 事件模式会话：记忆读取器只读取本事件的记录
- 绑定信息变化产生新会话时重新读取并重建上下文

### 管理命令说明

| 命令类别 | 用途 |
|---------|------|
| 绑定/解绑 | 绑定或解绑指定通道的用户，绑定携带 agent_name、role_name、mode |
| 管理权限管理 | 添加或移除用户的管理权限 |
| 角色切换 | 修改指定通道绑定的角色 |
| 模式切换 | 修改指定通道绑定的记忆模式（角色/事件） |
| 重新进入事件 | 让事件模式会话按事件标识进入指定事件 |
| 列出事件 | 列出所有事件 |
| 重置 | 重置指定会话的上下文 |

### 自身发送消息识别

- agent 自己发出的消息（下行）经通道再次返回（回显）时，其 msg_id 与发送时 OutgoingMessageResponse 返回的 msg_id 一致
- nexus 在发出 OutgoingMessage 拿到 response 后，把 response.msg_id 记入对应 channel 的 ChannelContext 未回显集合（TTL 懒清理，默认 60s）
- 收到 IncomingMessage 时按 msg_id 查该集合：命中则移除并丢弃（发送时已写入记忆，无需重复处理）；未命中则视为普通上行消息写入记忆（is_self=0）

### 自动上下文重置

上下文消息数量超过上限时自动触发重置：event 模式压缩（归档当前缓存 → LLM 总结 → 重写为 system+user(压缩指令)+assistant(总结)），role 模式归档后按记忆打包重建，防止 LLM token 占用无限增长。

### 初始上下文构建

- 会话创建或重置时按模式构建：event 模式从本地缓存全量恢复（缓存生命周期 = 当前上下文，超长即压缩，天然有界）；role 模式从 memory-store 两次查询并集读取最近消息（最近 N 条 ∪ [M, T_N] 时间段，M = min(时间窗起点, T_N)）打包为一条 user 消息
- 上下文缓存存放于 `<data_dir>/context/`，按 session_key 编码存储，追加不截断；重置/压缩前归档到 `<data_dir>/context-history/`（本轮只写不读）
- 结合 memory-ego 按该 agent_id 和 role_name 读取的自我认知信息设置为系统消息
- 运行时每次收到合批后的用户消息和 LLM 回复（含工具调用与结果）后增量追加到该会话的上下文并同步写入缓存，保持对话连续性

### agentic loop 记忆写入

- agentic loop 中，LLM 返回后将思考内容、工具调用指令、工具结果写入 memory-store
- 写入失败仅记录日志、不重试，避免阻塞后续操作

### 通道连接管理

- 启动时读取配置中的通道绑定列表（每项含 channel 身份与 agent_name、role_name、mode），为每个通道建立连接，连接后自动获取通道信息并发送绑定请求绑定指定用户
- 每个连接独立运行，支持心跳检测和断线自动重连
- 上行消息按消息来源 channel 对应的绑定配置定位会话后送入协调器处理，下行消息通过该会话回复 channel 对应的连接发送，并接收通道返回的发送结果

### 工具调用分派

- LLM 返回 tool call 时解析工具名称和参数
- 工具定义（name/description/parameters）按会话 context 配置的启用 toolkit 白名单聚合（`Station::tools(filter)` 平铺递归），随每次 LLM 请求发送
- 工具调用按工具名在全局 Station 中路由：本地 Toolkit 实现表（工具名整树全局唯一）在进程内执行，未命中查 tool_routes 路由缓存（工具名 → 直接子 station_id）调对应子（骨架期返回未实现错误）；路由未命中返回错误结果
- 工具结果作为 tool 消息（tool_call_id/name/content）追加回上下文，再次触发 LLM tool call 时支持多轮嵌套处理（上限防死循环）
- 每个工具调用生成 UUID key，写 channel 占位记录（Content::ToolCall(key) / Content::ToolResult(key)），ToolCallRequest 与 ToolResultRequest 详情记录用同一 key 关联（think 同款机制），经 channel 时间线可见工具调用锚点

### LLM 调用重试

- LLM 调用失败时按管理员配置的策略自动重试，提高请求成功率
