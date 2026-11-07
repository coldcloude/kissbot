# Nexus

## Pipeline

### 默认 Pipeline
- 默认 Pipeline 实现标准的 agentic loop 工作流：将输入发送至 LLM，LLM 返回文本回复，或者 Tool Call。收到 Tool Call 时，执行后将结果发回 LLM，等待回复；否则不再循环，回复文本回复
- 收到 Channel 消息时按时间等待，一个批次启动一次循环
- 一个 Pipeline 实例对应一个 session
- 当上下文长度超过阈值时，发送压缩提示词至 LLM 进行压缩

### 默认记忆 Pipeline
- 默认记忆 Pipeline 在默认 Pipeline 的基础上，额外处理记忆相关的逻辑：
  1. 收到 Channel 消息时记录到 memory-store
  2. 收到 LLM 回复文本消息时，将回复记录到 memory-store
  3. LLM 回复带思考消息时，将思考消息合并到回复消息，随回复记录到 memory-store
  4. 收到 LLM Tool Call 时，执行 Tool Call，将 Tool Call 请求和结果都合并到回复消息，随回复消息记录到 memory-store
  5. Role 模式下，收到 Channel 消息时先判断是否与上下文处于同一事件，非同一事件时清空上下文
  6. （可选）当新建上下文，或者上下文长度超过阈值时，从 memory-store 中提取记忆，替换当前上下文
    - 从 memory-store 中读取记忆时，采用 最近时间 和 最后条数 取并集
    - 最近时间早于上次上下文重置（含超长重置和换事件重置）时，以上下文重置时间为准

### 记忆摘要 Pipeline

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
  - 分为全局管理命令，和pipeline管理命令
  - 判断为命令后，回复信息发送到来源 Channel
  - 非命令是否发送回复消息、往哪个 Channel 发送消息，均由pipeline决定

- 向 Channel 发送消息后，应保留回复的msg_id一段时间，如果从 Channel 收到同一条，则跳过入库并移除msg_id记录

- 上下文是否超长在发送消息前检查
