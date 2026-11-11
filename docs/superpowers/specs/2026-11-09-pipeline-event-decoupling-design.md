# event 与 channel 解耦、pipeline 维护当前事件 设计

> 日期：2026-11-09
> 范围：`kissbot-agent` 的 `channel_manager`、`pipelines`、`nexus`、`types`、测试与文档。
> 不改 `blog/`（人工文档，禁止修改）。

## 背景

当前 event 是 channel 的运行态：

- `Channel` 保存运行态 `event`；
- channel 按 `agent_id + role_name + event` 合成 `MemoryKey`，每个 `MemoryKey` 一条 pipeline；
- `/mode event`、`/mode role` 属默认管理命令，由 `ChannelManager` 修改 channel 运行态 event，再触发 pipeline 重定位；
- `default` / `default_memory` / `memory_summary` 都只持有一个固定 `RoleEvent`。

这带来两个问题：

1. event 属于 agent 的记忆/会话范围，却绑定在 channel 上；同一 agent + role 的不同 event 被拆成多条 pipeline，pipeline 选择实际上由 event 参与了。
2. 切换 event 会重建整条 pipeline（含合批队列与 session 集合），而 `memory_summary` 的 session 本来就只和 case 绑定，不需要随 event 重建。

本次改造把 event 收进 pipeline：

- pipeline 身份只由 agent + role 决定；
- event 由 pipeline 在内存中维护，新建 pipeline 一律从角色模式开始；
- event 切换是 pipeline 命令，按 pipeline 自己的语义处理；
- channel 只切换 agent / role，负责查找或新建 pipeline。

## 标识与职责

| 标识 | 含义 | 使用位置 |
|------|------|----------|
| `RoleKey`（`kissbot_api`，agent_id + role_name） | pipeline 身份与选择键 | `PipelineManager`、channel 定位 pipeline、`/pipeline` / `/agent-pipeline` 失效判断 |
| `MemoryKey`（agent_id + `RoleEvent`） | 记忆范围（role + 可选 event） | memory-store、memory-summary、session_id 推导 |
| pipeline 当前 event（`Option<ArcStr>`，`None` = 角色模式） | 该 pipeline 当前使用的记忆/会话范围 | 各 pipeline 实例，仅内存保存，不落盘 |

`RoleKey` 使用现有类型，不新增 pipeline 专用键。`MemoryKey` 继续保留，但不再参与 pipeline 的装配与清理。

## channel 层

- `Channel` 删除运行态 `event` 字段；`ChannelManager` 的变更任务只携带 `agent_id` / `role_name` 两个可选字段（None = 保持当前值）。
- channel 定位 pipeline 只读 `ChannelConfig.agent_id` / `role_name`，拼 `RoleKey`。
- `/agent`、`/role` 仍是默认命令，经变更队列串行写配置并重定位 pipeline：
  - 旧 `RoleKey` 的 pipeline 对不再绑定的 channel 调用 `channel_unbound`；
  - `retain` 只保留仍被 channel 引用的 `RoleKey`；
  - 为新 `RoleKey` 装配 pipeline。
- event 切换不触碰 channel 配置、不触发 pipeline 重定位与 `retain`。
- 换 agent / role 会得到新 pipeline，新 pipeline 从角色模式开始；原 pipeline 的当前 event 不迁移。

## 管理命令

- `/mode event [event-id]`、`/mode role` 从默认命令层移到 pipeline 命令层；三条 pipeline 都支持，解析与回复文案由 `pipelines.rs` 共享：
  - 无 event-id 时生成 UUID；
  - 回复 `✅ 新事件 ID: {id}` / `✅ 已切换为角色模式`。
- 默认层不再识别 `/mode`；管理员校验、回复到来源 channel、命令不入记忆等通用规则不变。
- `/model`、`/bind-outgoing`、`/unbind-outgoing`、`/load-memory` 的归属与文案不变。

## default / default_memory：只保留当前 session

两条 pipeline 各自维护当前 session，不共用运行态对象：

```text
DefaultAgenticLoopPipeline / DefaultAgenticLoopPipelineWithMemory
  role_key: RoleKey
  current: ArcSwap<EventSession>
```

```text
EventSession
  memory_key: Arc<MemoryKey>（含当前 event_id）
  session_id: ArcStr
  session: Arc<Session>
  last_total_tokens: AtomicU64
```

会话创建、切换、系统提示词重置与 `/mode` 处理分别实现在两个 pipeline 中；
`default` 的创建逻辑不导入 agent/role 配置、使用固定提示词，`default_memory` 导入配置并使用 ego 提示词。

行为：

1. pipeline 创建时当前 event = `None`，创建角色模式 session；`default` 设置固定系统提示词，`default_memory` 先从 agent/role 导入 session 配置，再设置 ego 系统提示词。
2. 内存中只保留 `current` 一个 session；不缓存其他 event 的 session。
3. 切换 event：
   - 与当前相同 → 直接复用；
   - 不同 → 释放旧 session，按 `pipeline_session_id()` 新建 session 并**每次从本地缓存重新读取**；新建时同样导入配置、设置系统提示词。
4. session_id 规则：角色模式 `{agent_id}-{role}`，事件模式 `{agent_id}-{role}-{event}`。
5. 消息批次开始时读取一次当前 `EventSession`，整批使用它的 session、memory_key 与 token 计数；同一批不会被中途切换影响。
6. `last_total_tokens` 属于 `EventSession`；切换 event 后新读取的 session 从 0 开始。
7. 当前 event 的 session 是 pipeline 命令的作用域：`/model`、`/bind-outgoing`、`/unbind-outgoing`、`/load-memory` 只写当前 session。

### 命令与批次串行队列

`default` / `default_memory` 各自维护一条 mpsc 任务队列，命令与批次进入同一队列按序执行：

```text
PipelineTask
  Command { channel_id, channel, content, done }
  Batch   { events, done }
```

- `AgentPipeline::execute_command` 只把命令入队并等待 `done`；
- `IncomingMessageBatchProcessor::incoming_message_batch` 把批次入队并等待 `done`，保证批次消费者看到的完成顺序与队列一致；
- 队列 worker 按到达顺序串行执行：命令处理（含 `/mode` 切换 session）与批次处理（读取并运行当前 session）不会并发；
- pipeline 销毁时队列关闭，worker 退出。

### 命令的来源通道

`AgentPipeline::execute_command` 增加来源通道三元组参数 `channel: Arc<ChannelKey>`（messenger + 接收方用户 + 群组）。`channel_id` 仍表示 agent 侧连接的 channel；`channel` 供按来源通道维护状态的 pipeline 使用。

`memory_summary` 不适用这一节。

## memory_summary

- pipeline 维护 **channel（messenger + 接收方用户 + 群组）→ 当前 event** 的映射，而不是单一 event。
- `/mode event [event-id]` / `/mode role` 按命令来源的 `ChannelKey` 修改映射，只影响该 channel；其他 channel 的映射保持不变。
- 批次处理时用该批次的 `ChannelKey` 查映射，合成当次 `MemoryKey`；`locate`、上行记录写入、下行记录写入都使用这份快照。
- 切换 event 等价于切换该 channel 下一次 locate 的参数：不重建 pipeline、不切换 case session。
- case session 仍只和 case 绑定（session_id 不含 event）；切换 event 不影响已加载的 case session。
- 批次进入时捕获的 `MemoryKey` 随 case 待执行任务保存；case run 的摘要查询、结尾原文查询与下行记录都使用该快照，避免 run 期间切 event 改变写入范围。
- 同一 case 的待执行任务合并时保留先到任务的 `MemoryKey`。
- `channel_unbound` 清理该 channel 的生产者与 event 映射。
- `/model` 仍写基准 session 与已有 case session；outgoing 仍写基准 session。

## PipelineManager

- `pipeline_map` 的键从 `Arc<MemoryKey>` 改为 `Arc<RoleKey>`。
- `ensure_pipeline` 按 `RoleKey` 读取 `PipelineConfig` 并装配；同一 agent + role 的多个 channel、多个 event 共享一条 pipeline。
- `retain`、`channel_unbound`、`incoming_message`、`contains` 全部改用 `RoleKey`。
- `set_pipeline_config`：受影响的已装配 pipeline 按 agent + role 过滤；写入新配置后重新合成，pipeline 名称变化的键从注册表移除，下次按新配置装配。
- `PipelineNotFound` 携带 `Arc<RoleKey>`。

## 边界

- 切换 event 前已进入合批队列、切换后才处理的消息，按处理时的当前 event 运行（批次开始时取当前 `EventSession` / `MemoryKey`）。
- `default_memory` 的上行记忆写入仍发生在收到消息时；随后批次若在切换 event 后才运行，批次本身的会话与记忆范围取切换后的 event。
- `/mode` 参数非法（缺模式、未知模式、`event` 多参数、`role` 带参数）返回 `InvalidCommand`，回复文案由共享解析给出。
- 切换 event 不需要 channel 配置存在；命令仍要求来源 channel 的管理员身份，由 channel 层统一校验。
- pipeline 丢弃时其当前 session 随 Arc 释放；session 不单独清理。

## 测试

- `RoleKey` 去重：同一 agent + role 的不同 channel 只装配一条 pipeline；不同 role 各自一条。
- event 不再是 pipeline 身份：同一 agent + role 切 event 后 `PipelineManager` 不新增 pipeline，旧 pipeline 不被清理。
- `/mode`：默认命令层返回 `Ok(None)`；三条 pipeline 均能识别；`event` 无参生成 UUID、`role` 回到角色模式、参数错误返回 `InvalidCommand`；回复文案保持不变。
- `default` / `default_memory`：
  - 角色模式与事件模式使用不同 session_id；
  - 内存中只保留一个 session；切换 event 时释放旧 session，切回旧 event 每次从本地缓存重新读取；
  - 命令与批次在同一队列中串行执行，命令等待执行结果、批次等待处理完成；
  - 事件模式下 `load_memory` 恒为 false，角色模式仍按开关判断；
  - `/model` / outgoing / `/load-memory` 只影响当前 event 的 session。
- `memory_summary`：
  - 切换 event 只修改来源 channel 的映射，其他 channel 不变；
  - 批次按自身 `ChannelKey` 的映射携带 event 调用 locate；
  - case session_id 不含 event，切换前后继续复用；
  - 在途 case run 使用进入任务时捕获的 `MemoryKey`。
- e2e：现有 `/mode event` / `/mode role` 用例保持文案不变，命令改由 pipeline 返回。

## 文档

- 新增本设计文档。
- 更新 `docs/spec/agent-nexus.md`：pipeline 标识、命令归属、单 session 与串行队列、memory_summary 的 channel→event 映射、session 清理。
- 更新 `docs/spec/agent-modules.md`：channel 运行态、PipelineManager 键、pipeline 装配与上下文管理。
- 更新 `docs/design/components-design/kissbot-agent-nexus.md`：核心功能与流程中的 event 描述、命令表、事件模式流程。
