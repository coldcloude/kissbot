# Nexus

## Channel 消息接收细节

- 未知指令当作一般消息处理
- 只有第一个pipeline可以正确绑定，剩下的均跳过，可以加个错误日志
- input_channels 不继承，role 不配置 input_channels 时按空集合处理

## Pipeline

### 默认 Pipeline 和 默认记忆 Pipeline 共通设计细节

- Pipeline 执行指令（包括但不限于切换模式/事件）和处理消息，放到同一个mpsc队列中排队执行，避免多个变更session间竞争，或者变更session和处理session间竞争
- 但同一时间仅能处于一种模式或一个事件，因此内存中仅有一个当前 session，而磁盘中有多个 session 的缓存文件
- 切换模式/事件时，从缓存文件中读取 session

### 默认 Pipeline
- 默认 Pipeline 实现标准的 agentic loop 工作流：将输入发送至 LLM，LLM 返回文本回复，或者 Tool Call。收到 Tool Call 时，执行后将结果发回 LLM，等待回复；否则不再循环，回复文本回复
- 收到 Channel 消息时按时间等待，一个批次启动一次循环
- 【Role 模式】和【Event 模式下的每个事件】均对应一个单独的 session
- 当上下文长度超过阈值时，发送压缩提示词至 LLM 进行压缩

### 默认记忆 Pipeline
- 默认记忆 Pipeline 在默认 Pipeline 的基础上，额外处理记忆相关的逻辑：
  1. 收到 Channel 消息时记录到 memory-store
  2. 收到 LLM 回复文本消息时，将回复记录到 memory-store
  3. LLM 回复带思考消息时，将思考消息合并到回复消息，随回复记录到 memory-store
  4. 收到 LLM Tool Call 时，执行 Tool Call，将 Tool Call 请求和结果都合并到回复消息，随回复消息记录到 memory-store
  5. 收到 Channel 消息时按时间等待，一个批次启动一次循环
    - Role 模式下，收到消息时先判断是否与上文处于同一事件，非同一事件时新建一个 session
    - Event 模式下，每个事件对应一个 session
  6. （可选）当新建上下文，或者上下文长度超过阈值时，从 memory-store 中提取记忆，替换当前上下文
    - 从 memory-store 中读取记忆时，采用 最近时间 和 最后条数 取并集
    - 最近时间早于上次上下文重置（含超长重置和换事件重置）时，以上下文重置时间为准
  7. 切换到 Role 模式或者新建 Pipeline 进入 Role 模式时，查找最近一次 session，如果无 session 则新建一个，否则读取最近一次 session
  7. Role 模式切换事件在两次 agentic loop 之间，而可选记忆读取只作用于一次 agentic loop 的压缩过程，因此二者互不干扰

### 记忆摘要 Pipeline

- 同时维持多个 session，维持 Channel 和模式/事件的映射关系，当收到变更 Role 或 Event 的指令时，修改这个映射

- 执行流程
  1. 收到 Channel 消息时，按照 Channel 合并批次，Channel 批次超时后处理一次
  2. 首先将 Channel 的批次消息发送到记忆摘要组件，记忆摘要组件把每条消息都归属到一个事件中，返回消息和事件的对应关系
  3. 对于一个批次消息的每个事件，启动一次 agentic loop，其中产生的消息均归到该事件
    - 如果没有对应 session，从记忆摘要中读取摘要和最近消息，作为 session 上下文
    - 如果有对应 session 且没有超过上下文长度阈值，则继续使用原 session 上下文，仅追加新消息

- 异步设计
  - Channel 批次合并后发送到队列执行，同时解决并发和等待的问题
  - 一个批次划分为多个 case 后，每个 case 发送到队列执行，同时解决并发和等待的问题

## 设计细节

- 管理命令
  - 分为全局管理命令，和 Pipeline 管理命令
  - 判断为命令后，回复信息发送到来源 Channel
  - 非命令是否发送回复消息、往哪个 Channel 发送消息，均由pipeline决定

- 向 Channel 发送消息后，应保留回复的msg_id一段时间，如果从 Channel 收到同一条，则跳过入库并移除msg_id记录

- 上下文是否超长在发送消息前检查
