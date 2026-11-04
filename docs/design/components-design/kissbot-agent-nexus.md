# kissbot-agent-nexus 模块设计

## 概述
Nexus — Agent 组件的编排中心。智能体的"思考"部分：负责将外部输入与记忆加工为 LLM 可用的上下文、驱动回复生成与工具调用，并统一调度同进程内的 Station、channel 适配层与记忆系统。Nexus 本身不实现 agentic loop 的执行步骤，而是由 pipeline 体系（输入处理 / 消息发送 / 工具调用 / 输出处理 / 系统提示词）组合出每条 pipeline 的执行流程，Nexus 向它们提供记忆、Provider、Station 与回复发送能力；session 的创建与使用由 pipeline 自己决定。

Nexus 是 Agent 组件的一部分，与消息通道建立实时连接进行消息收发，与 memory-store、memory-ego 和 station 通过请求-响应方式通信。一个系统内可运行多个 agent 实例，每个实例独立的 Nexus 与 Station 单例。

一个 nexus 可同时管理多条 pipeline。nexus 按每个绑定 channel 的 agent_id（空/缺省 = 保留 agent "0"）、role_name 与运行期 event 合成项目级 `RoleEvent` 后去重，每个 agent_id + RoleEvent 组合为一条 pipeline；channel 定位到 pipeline，pipeline 再按自身需要创建 session（默认 pipeline 一个 session，摘要记忆 pipeline 每个 case 一个 session）。各 pipeline 拥有独立的记忆读写范围、配置与模式状态；session 拥有独立的 LLM 上下文。回复目标由 pipeline 的 out_channel 配置决定（未配置 = 只存不回复），而不取决于消息从哪个 channel 到达。

## 核心功能

### Epic 1：外部输入处理
- **目标**：接收来自消息通道的输入，按消息来源 channel 的绑定配置定位 pipeline，区分并路由管理命令与普通消息到对应处理流程
- **依赖**：Epic 5（消息通道通信）
- **用户故事**：
  - 作为一个用户，我要向智能体发送消息，以丢弃智能体自身发出后又回显的消息
  - 作为一个管理员，我要向智能体发送管理命令，以让智能体执行相应管理操作（无权限用户发送的命令被忽略）
  - 作为一个用户，我要向智能体发送普通消息，以触发智能体生成 LLM 回复

### Epic 2：LLM 交互
- **目标**：封装 LLM API 调用，支持多种 LLM 提供商和模型，构建完整的 LLM 上下文并调用
- **依赖**：Epic 3（上下文管理）、memory-ego（自我认知信息）
- **用户故事**：
  - 作为一个管理员，我要按 provider + model 配置 LLM 地址、密钥与参数，以控制 LLM 的行为
  - 作为一个管理员，我要为不同 (agent, role) 甚至单条 pipeline 指定不同的 provider/model 与参数
  - 作为一个管理员，我要配置 LLM 调用失败时的重试策略，以提高请求成功率
  - 作为一个用户，我要向智能体发送消息，以获取智能体调用 LLM 生成的回复，并让智能体将回复发送到消息通道

### Epic 3：上下文管理
- **目标**：按 session 管理 LLM 上下文，支持恢复、增量追加与超长自动压缩/回溯
- **依赖**：Epic 4（记忆读写）、memory-ego（自我认知信息）
- **用户故事**：
  - 作为一个管理员，我要启动智能体程序，以让智能体按 session 恢复上一次的本地上下文
  - 作为一个用户，我要向智能体发送消息，以让智能体保持对话连续性
  - 作为一个管理员，我要设置上下文 token 占用上限与压缩阈值，以防止上下文无限增长

### Epic 4：记忆读写
- **目标**：在 pipeline 创建其 session 时恢复本地上下文；在上下文超长时按配置压缩或从 memory-store 回溯打包；在 agentic loop 中把思考、tool call、tool result 与收发消息写入记忆系统；各 pipeline 按自身模式（角色/事件）隔离记忆范围
- **依赖**：memory-store（记忆读写）、memory-ego（自我认知信息）
- **用户故事**：
  - 作为一个用户，我要询问智能体过往的对话内容，以让智能体回顾对话脉络
  - 作为一个管理员，我要修改通道绑定的角色或记忆模式，以让智能体按模式隔离记忆范围
  - 作为一个用户，我要向智能体发送消息，以持久化智能体的思考过程和工具调用记录
  - 作为一个用户，我要向智能体发送消息，以持久化完整的对话收发记录

### Epic 5：消息通道通信
- **目标**：作为客户端连接消息通道的服务端，支持多通道同时连接、心跳检测和断线重连
- **依赖**：kissbot-channel（消息通道服务端与绑定协议）
- **用户故事**：
  - 作为一个管理员，我要配置通道绑定（每项含 channel 身份与 agent_id、role_name；event 由 `/mode` 在运行期设置），以让智能体启动时连接配置的通道并绑定用户
  - 作为一个外部系统（消息通道），我要与智能体保持长连接，以保持通信的稳定性
  - 作为一个外部系统（消息通道），我要向智能体推送上行消息并接收其下行消息

### Epic 6：工具调用分派
- **目标**：解析 LLM 返回中的 tool call，按工具名路由到全局 Station 执行（本级原生工具进程内执行 / 直接子 Station 经 MCP 递归）
- **依赖**：Epic 2（LLM 交互）、Station 组件
- **用户故事**：
  - 作为一个用户，我要向智能体提出需要执行工具的任务，以触发智能体执行工具调用
  - 作为一个外部系统（工具执行站），我要接收智能体发送的工具调用请求，以执行外置工具并返回工具执行结果
  - 作为一个用户，我要向智能体提出复杂的多步任务，以完成复杂的工具链调用

## 内部模块

### 1. 协调器（Nexus）
- 核心调度层：管理 nexus 的启动流程、pipeline 集合与全局单例装配
- 按消息来源 channel 的绑定配置定位 pipeline，将输入路由到管理命令处理或该 pipeline
- 装配 pipeline 时不注入 session；pipeline 自己按需要创建并持有 session，运行期直接读写自己的 session 上下文
- 提供能力接口供 pipeline 回调：Provider 调用、Station 工具分组白名单与调用、记忆写入（思考 / tool call / tool result）、记忆回溯打包、回复发送、ego 系统提示词查询
- pipeline 绑定（agent_id / role_name / event）变更走串行队列，避免写-写竞态

### 2. 配置管理器
- 从公共配置加载静态配置（数据目录、监听地址、外部服务地址），引导并持久化 nexus.json / station.json
- 管理 provider+model 参数（配置结构由 kissbot-llm 定义）、(agent, role) 级配置（LLM 参数、压缩、记忆回溯、记忆读取开关、合批、回复通道、工具分组白名单、pipeline）与 session 级值
- 分开存储 agent/role 可继承配置与 session 级值：`default_memory` / `memory_summary` 建立 session 前用 `set_session_config_if_not_exists` 从 agent → role 导入配置；`default` 不依赖 agent/role；运行期只按 session_id 读；pipeline 选择按 agent/role 合成
- 支持管理命令与管理 API 回写

### 3. LLM 客户端（provider 体系）
- provider 实现（OpenAI 兼容 / Anthropic 的请求构造、调用、重试与响应归一化）由 **kissbot-llm** 提供；Nexus 的 ProviderManager 负责按 provider 名缓存实现实例（工厂去重）、解析当前 session 的有效模型配置（超时 / 重试 / token 上限）并按 provider_type 分派
- 归一化结果包含正文、思考内容（API 字段与 `<think>` 标签各自独立）、工具调用与 token 占用，供上下文与阈值判定使用
- 支持模型列表查询（用于模型校验与管理命令）

### 4. 会话与上下文（pipeline 内部）
- session 的数量与 id 由 pipeline 决定，pipeline 自己创建、恢复并持有 session
- 单个会话的上下文本体（内存消息序列 + 当前/待定 system 消息 + 本地缓存 + 历史归档）由 **kissbot-llm** 的 Session/SessionContext 承担
- 建立 session 时正序全量回读本地缓存，并做崩溃一致性清理（丢弃悬挂的 tool_calls 尾部与开头的孤立工具响应）
- session 不单独注册到全局表；pipeline 丢弃时其 session 随 Arc 释放自然回收

### 5. pipeline 体系（执行流程）
- **AgentPipeline**：执行流程本体，自己创建并持有需要的 session。当前三实现：`default`（无记忆的 agentic loop，一个 session）、`default_memory`（带记忆读写的 agentic loop，一个 session；事件模式只写不读，角色模式按 session 开关决定是否读取）、`memory_summary`（摘要记忆 pipeline，按 channel 攒批 + 每个 case 一个 session）。
- **消息接入与合批**：pipeline 自己维护批次生产者与消费者。`default` / `default_memory` 一个 pipeline 一个合批队列；`memory_summary` 为每个 channel（messenger + 接收方用户 + 群组）维护独立的批次生产者，各 channel 互不干扰。消费者在最后一条消息超过合批间隔后把整批交给 pipeline。
- **case 拆分与执行**：`memory_summary` 的批次先做一次定位并写入本批上行记录，然后按消息所属 case 并入该 case 的待执行任务，批次处理随即结束，下一批可以接着处理。待执行任务由另一个队列串行执行：worker 一次只取一个 case，每个 case 一次 run；每个 case 维护独立 session，run 开头用该 case 的摘要与结尾原文副本重置该 session 的上下文，再把本次新增消息打包为一条 user 消息进入 agentic loop。还没开始执行的 case 继续等待，期间到达的同 case 消息并入同一次 run；正在执行的 case 的新消息形成下一轮 run。
- **agentic loop**：`default_memory` 与 `memory_summary` 各自实现自己的循环；两者都按轮次发送 LLM 请求、执行工具调用、把回复发到 out_channel 并写记忆，超过 token 阈值时按配置压缩或记忆回溯。`default_memory` 在角色模式且 `load_memory=false` 时，每批 run 前先用 LLM 判断新消息与完整会话是否同一件事；不同则归档旧上下文、清空 session、只保留新消息后运行。摘要记忆 pipeline 复用发送/工具/提示词能力，但批次接入、定位与 case 上下文重建在自己的实现里。
- **记忆写入位置**：上下行通道记录的构造是纯函数，写入与回复发送都在 pipeline 内完成。`default_memory` 在收到上行消息时立即写入，回复经回复发送流程发送并写入（meta 为 None）；`memory_summary` 在定位之后写入上行记录（带消息所属 case 的 meta），回复同样带本次 run 所属 case 的 meta；定位失败时直接丢弃本批消息（不写记忆、也不进入待执行任务）。
- **channel 解绑**：AgentPipeline 提供 channel 解绑入口。channel 切换 agent/role/mode 时，通道侧对旧 pipeline 调用它；摘要记忆 pipeline 据此丢弃该 channel 全部分组的生产者与待处理消息。Nexus 把「解绑旧 channel + 清理无绑定 pipeline/session + 装配新 pipeline」合成为一个重绑定流程。
- **PipelineManager**：按 pipeline 标识（agent + role_event）持有 pipeline，提供装配、消息投递、channel 解绑通知与随绑定清理；装配哪些 session 由 pipeline 自己决定，清理时直接丢弃 pipeline，session 随 Arc 释放。pipeline 实现由 agent/role 配置的 `pipeline_name` 选择。

### 6. 记忆读写
- 上下行消息、思考内容、tool call / tool result 统一经 memory-store 客户端写入（由 pipeline 在消息接入/回复发送时触发）；写入失败仅记录日志、不重试
- 上下文超长时的处理：事件模式恒用压缩；角色模式按 `memory_pipeline.load_memory` 选择压缩或回溯。回溯由 memory_recover 消息发送器触发：按 (agent_id, role_event) 两次查询并集（最近 N 条 ∪ 时间段）打包为交替的对话消息，归档旧上下文后重建
- 角色模式且 `load_memory=false` 时不查询记忆；每批 run 前用两套提示词询问 LLM 是否同一件事（YES/NO）：第一套失败或无法解析后，第二套仍先要求判断是否同一件事、再强制只回答 YES/NO；两次都失败按同一件事处理。判定为不同时归档旧上下文、清空 session，只保留本批新消息
- 记忆回溯配置（时间窗、条数）、记忆读取开关与 LLM 参数独立，在建立 session 时一并固化到 session 配置

### 7. 管理命令处理
- 识别以 `/` 开头的外部消息（且发送者在来源 channel 的 admins 中）
- 先做默认命令检测与执行：绑定/解绑通道用户、管理权限增删、切换 agent/role、切换模式
- 默认层未命中的命令交给来源 channel 的 pipeline 自行识别执行：设置/清除回复通道、切换 pipeline 模型；pipeline 不支持则回复未知命令
- 命令与命令回复不入记忆、不进上下文

### 8. channel 运行态与 pipeline 定位
- 维护各 channel 的运行态 event（缺省 = 角色模式，不回写配置）与运行态资源
- 按 channel 绑定合成 pipeline 标识（agent_id + role_event），由 Nexus 定位/装配 pipeline
- 绑定信息变化时清理无绑定 pipeline；其 session 随 pipeline 释放自然回收
- 事件模式按事件标识隔离 pipeline、上下文与记忆

### 9. Station 运行态
- 全局 Station 单例（每 agent 一个）从配置构建：本级 group 集合（工具元数据 + 原生实现，由内置注册表按 group 名填充）+ 直接子 Station 集合（仅连接信息，MCP 客户端）
- 内置示例工具 Read：读取文本文件，路径经绝对路径规范化后校验位于当前工作目录内（防穿透），返回内容限长；配置显式声明 filesystem group 名才注册
- 工具名 group 内唯一；本级构建时的重名静默保留先到者，级联合并时的跨子站重名保留先到者并记 warn 日志（对外仍给出唯一名字）

### 10. Station 工具执行
- 工具定义由 nexus 按当前 session 的工具分组白名单聚合（平铺递归），并在边界处转换为 LLM 侧工具定义，随 LLM 请求发送
- 本级工具：按工具名在本级工具索引中查找并执行（注册的工具必带原生实现）；子 Station 工具：本级未命中时按工具名路由到归属的直接子 Station，经 MCP 调用（不遍历全部子）

### 11. 边界组件（非 Nexus 内部模块）
- **通道适配层（channel_manager）**：实现 Terminal（回显过滤 + 业务消息处理：默认命令 / pipeline 命令 / 普通消息）、每 channel 运行态（pending msg_id、event、client、断线通知）、连接/重连/发送，并持有 channel 变更串行队列（其执行体之一即 pipeline 重定位，内联在 apply_channel_key）
- **管理 API（http_server）**：管理界面用的配置 CRUD，直接操作配置管理器
- **station MCP 服务端（station_http）**：把全局 Station 作为 MCP 处理器对外提供 server/discover、tools/list、tools/call，供其他 station 作为 sub 使用

## 功能流程

### Epic 1 外部输入处理流程

```
通道适配层收到消息
  → 按 msg_id 查所属 channel 的未回显 outgoing 集合
    ├─ 命中 → 识别为自身发出的消息回显，丢弃（发送时已写入记忆）
    └─ 未命中 → 通道适配层直接处理业务：
        → 默认命令（文本以 "/" 开头且在 admins 中）→ 默认层执行 → 回复发回来源通道
        → 默认命令未命中 → 定位 pipeline → pipeline 命令检测执行 → 回复发回来源通道
        → 系统事件（群组变更 / 用户移除）→ 不进 pipeline
        → 普通消息 → 定位 pipeline（按来源 channel 的绑定）→ 该 pipeline（记忆写入也在 pipeline 内）
```

### pipeline 执行流程（agentic loop）

```
普通消息 → pipeline 接入（合批：连续消息等待合批间隔后打包为一条 user 消息；摘要记忆：按 channel 独立攒批，先 locate 并按 case 拆分，各 case 以自身摘要与结尾原文重建自己的 session 后分别运行）
  → AgentPipeline：自己持有 session（发送器 / 工具调用器 / 输出处理器，运行期直接读写该 session 上下文）
  → 循环（上限防死循环）：
      1. 发送器：上下文超阈值时先压缩或记忆回溯（reducer 一次重置会话）→ 拼接新消息 → 调用 LLM
      2. 输出处理器：
           ├─ 有 tool_calls → 逐个执行（全局 Station 经适配层）→ 结果作为工具消息追加 → 继续下一轮
           └─ 有正文 → 追加助手消息 → 经回复通道发送（并写入记忆）
  → 轮次结束：把本轮新增消息追加进会话上下文
```

### 启动流程

```
kissbot-agent 启动
  → 配置管理器：加载公共配置的 agent 段，引导 nexus.json / station.json
  → Station：读 station.json 构建全局单例（内置注册表 + 配置声明 + 唯一性校验 + 子 Station 连接信息）
  → Nexus：装配运行时组件并注册单例；同时启动 channel 变更队列消费者
  → 后台启动管理 API 服务与 station MCP 服务端
  → 主循环：对每个 channel 绑定确保 pipeline 已装配（pipeline 自行创建 session、设置系统提示词）
  → 主循环：连接全部 enabled channel（创建客户端 + 重连循环 + 绑定身份）
  → 保持进程，等待通道回调驱动
```

### 上下文超长处理流程

```
触发条件：本次请求前，上一次模型响应的 usage.total_tokens 超过
          当前 session 模型 max_tokens_usage × 压缩阈值（max_tokens_usage = 0 表示不启用）
  → 该 session 的消息发送器按配置执行一种处理（均只重置一次会话）：
    ├─ 压缩：只读读取上下文（不应用待定系统消息、不触发系统消息变更重建）并只从中弹出最后一条 assistant（含）及之后的消息
    │        → 剩余部分 + 压缩指令 → LLM 总结 → 归档并重建为「压缩指令 + 总结 + 弹出的消息 + 新消息」（重建时应用待定系统消息）
    └─ 记忆回溯：从 memory-store 两次查询并集打包，拼接当前会话中最后一条 assistant（含）及之后的消息
             → 归档清空 → 重建（新消息随后追加；重建时应用待定系统消息）
  → 记录本次请求的 token 占用，供下次判定
```

### 事件模式流程

```
修改 channel 绑定为事件模式（/mode event）→ 通道运行态生成/指定事件标识，定位到对应 pipeline
  → 新 pipeline 由自己的 session 按本地缓存恢复上下文（空则仅系统消息），记忆读写按「role-event」编码隔离
重新进入指定事件（/mode event <event-id>）→ 切回该事件的 pipeline 与其 session
回到角色模式（/mode role）→ 消息进入角色模式 pipeline
```

## 关键设计

### pipeline 与 session 的划分

- pipeline 由（agent_id、role_event）唯一标识；nexus 将所有绑定 channel 的信息去重生成 pipeline 集合，多个绑定相同三元组的 channel 共享同一 pipeline
- 记忆读写用 agent_id 直接定位（配置即 agent_id，无需二次解析），role 编码 = 角色名（角色模式）或「角色名-事件 ID」（事件模式）
- 角色模式 pipeline：记忆读写覆盖该角色下所有记录；事件模式 pipeline：只覆盖本事件的记录
- session 是 pipeline 内部的执行资源：默认 pipeline 一个 session，摘要记忆 pipeline 每个 case 一个 session；session 上下文按各自缓存恢复与重建
- 绑定信息变化产生新 pipeline 时，由新 pipeline 重新恢复/构建其 session 上下文

### 回复通道

- 回复目标由 pipeline 的 out_channel 配置决定，经 `/bind-outgoing` 设置、`/unbind-outgoing` 清除；未配置 = 只存不回复
- out_channel 存放在 pipeline 自己的 session 配置（摘要记忆 pipeline 用基准 session）；agent/role 级配置只作为建立 session 时的初始值
- 发送前校验该身份仍在目标 channel 的绑定集合中；已解绑（配置悬空）则清理配置并跳过发送

### 管理命令说明

| 命令类别 | 归属 | 用途 |
|---------|------|------|
| 绑定/解绑 | 默认层 | 绑定或解绑指定通道的用户 |
| 管理权限管理 | 默认层 | 添加或移除用户的管理权限 |
| agent/角色切换 | 默认层 | 修改指定通道绑定的 agent 与角色 |
| 模式切换 | 默认层 | 修改指定通道绑定的记忆模式（角色/事件） |
| pipeline 配置 | 默认层 | 修改 (agent, role) 或 agent 级 pipeline（`/pipeline`、`/agent-pipeline`）；名称变化的已装配 pipeline 失效 |
| 回复通道 | default_memory / memory_summary | 设置或清除 pipeline 的回复通道 |
| 记忆读取开关 | default_memory | `/load-memory <on\|off>` 设置是否读取记忆（事件模式固定不读取） |
| 模型切换 | 三条 pipeline | 切换当前 pipeline 使用的 provider/model（校验后生效） |

### 自身发送消息识别

- agent 自己发出的消息（下行）经通道回显时，其 msg_id 与发送时响应返回的 msg_id 一致
- 发送成功后把返回的 msg_id 记入对应 channel 的未回显集合（TTL 懒清理）
- 收到消息时按 msg_id 查该集合：命中则移除并丢弃（发送时已写入记忆）；未命中则视为普通上行消息写入记忆（is_self=0）

### 自动上下文处理

最近一次模型响应的 token 占用超过「当前 session 模型上限 × 压缩阈值」时，下次请求前由消息发送器触发处理：压缩（LLM 总结后重建上下文）或记忆回溯（从 memory-store 打包重建），随后更新占用记录，防止上下文无限增长。事件模式只用压缩；角色模式 `load_memory=false` 只用压缩，并在每批 run 前判断是否同一件事，不同则先归档清空。阈值为 0（或上限为 0）时不启用该机制。

### 初始上下文构建

- session 创建时从本地缓存全量恢复（缓存生命周期 = 当前上下文，超长即压缩，天然有界），不读取记忆
- 系统消息由 pipeline 的系统提示词重置设置（固定提示词 / memory-ego 自我认知 markdown），先存为待定，发送前对比应用；系统消息随消息一并持久化，恢复时回到当前系统消息
- 运行时每次收到合批后的用户消息和 LLM 回复（含工具调用与结果）后增量追加到该 session 的上下文并同步写入缓存

### 记忆写入

- 上行消息（is_self=0）、下行回复（is_self=1，内容取发送响应）写入 memory-store；思考内容（reasoning_content / thinking 任一有值）与 tool call / tool result 作为下行通道记录的内嵌内容一起写入
- 写入失败仅记录日志、不重试，避免阻塞后续操作

### 通道连接管理

- 启动时按配置中的 enabled channel 建立连接，连接后按 bind_users 逐个绑定身份；每个连接独立运行，支持心跳与断线自动重连
- 上行消息按来源 channel 定位 pipeline 后送入该 pipeline；下行消息经 pipeline 的回复通道发送，并记录未回显 msg_id

### 工具调用分派

- LLM 返回 tool call 时解析工具名与参数
- 工具定义按当前 session 的工具分组白名单聚合（平铺递归），转换为 LLM 侧工具定义后随每次 LLM 请求发送
- 工具调用按工具名路由：本级原生实现在进程内执行，未命中查工具路由（工具名 → 直接子 station_id）经 MCP 调对应子；路由未命中返回错误结果
- 工具结果作为工具消息追加回上下文，支持多轮嵌套（上限防死循环）
- tool call 与 tool result 按在通道记录 `Multi` 中的先后顺序配对，经通道时间线可见工具调用锚点

### LLM 调用重试

- LLM 调用失败时按 provider/model 配置的重试次数与超时自动重试（指数退避），提高请求成功率
