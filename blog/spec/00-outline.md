# kissbot 概要

## 目标
- 一个持续存在、持续积累的赛博空间原生物种
- 一个跨时间、跨空间的宏观个体
- 下文一般使用 agent 称呼，也可能使用 机器人 等其他称呼

## 基本形式

- 以一个 agent 为活动单元，用 agent_id 作为标识
- 每个 agent 可以有多个 role，用 role_name 作为标识

## 关键组成部分

### 记忆

- **设计思路** 只要将给 agent 全部记忆都原样保存，即使当前缺少足够技术手段从中挖掘重要信息，随着技术进步，最终总会有一种可以更高效处理记忆的手段。因此将记忆的存储和索引设计为两部分：
  1. 存储：将全部记忆原样保存，不做任何处理。 —— memory store
  2. 索引：设计一个通用接口，可以高效读取记忆，并按照自定义的结构重新存储，提供自定义的查询。 —— memory-struct

#### 自我认知（memory-ego）

- 自我认知使 agent 知道自己是谁、面对什么个体、扮演什么角色

- 自我认知和记忆一样，为 agent 提供先验的信息，因此作为记忆的一部分

- **组成** 可以分为客观设定和角色定位两部分，每个 agent 对应一个可短认知，每个 role 对应一个角色定位
  1. **客观设定** 对 agent 的客观描述，比如名称，创建时间，创建时的目的（对于创造者是主观的，但对于 agent 自身是客观的）等
    - 客观设定还包括对其他个体（人或 agent）的客观认知，比如一个个体使用哪个社交账号，有谁创造/和谁有什么样的关系，agent 是否从属于/服从于哪个人等。
  2. **角色定位** agent 要扮演什么角色。一个个体可以扮演多个角色，用于不同的场景，如陪伴/工作，虚拟/现实等
    - 角色定位还包括个体在作为该角色时的可以接触到的其他个体扮演的角色的信息，例如在共同游戏场景下，个体要了解自己是什么角色、对方是什么角色

- 客观设定和角色定位中均可以包含禁止事项

- **来源** 可以由 agent 创造者直接配置设定，也可以从 agent 的原始记忆中提取得到

#### 记忆摘要（memory-struct-summary）

- 一种记忆索引的设计方式，将全部原始记忆划分为事件，对每个事件保留所有原始记忆，并总结摘要事件的内容
- 事件的树状设计：当一个事件包含的记录太多时，天然可以划分为多个子事件。此时，将原事件转化为一个复合事件，不再包含原始记忆，仅包含子事件，保留事件摘要
- 记忆入库时 meta 字段可以包含已经定位过的 case 名，从而跳过定位。

- 详见 [memory-struct-summary](./memory-struct-summary.md)

### Channel

- Channel 是 agent 所有即时通讯消息传输的通道，agent 作为 Channel 的客户端，使用 WebSocket 和 Channel 的服务端通信
- Channel 的服务端负责连接具体的外部系统（如 Web 页面、QQ 等），可以是仅作为对接层接入现有消息工具（如 QQ 机器人），也可以是包含消息工具和对接层的完整实现（如 Web 前后端）
- 一个 Channel 的实例代表了一个即时通信工具的聊天框，因此包含的信息有：用的什么工具（messenger）、自己是谁（user）、对方是哪个用户或群组（group）
- Channel 除了消息收发、附件上传下载等基础功能外，还可以实现用户管理、群组管理、附件管理等功能

- 详见 [channel](./channel.md)
- 详见 [channel-web](./channel-web.md)

### Agent

- 这里 Agent 指 kissbot 这个 agent 的核心运行组件。
- kissbot 的设想是可以实现跨空间的 agent，因此设计上将 agent 的 大脑 和 手脚 拆分成了不同的组件。
  - Nexus：agent 的大脑，和 LLM 通信，并对接 Channel 和 记忆系统
  - Station：agent 的手脚，负责执行 LLM 发起的 Tool Call
- 一个 Agent 实例可以只启动 Station 不启动 Nexus，也可以只启动不包含工具的 Station
- 一个 agent 可以包含多个 Agent 实例，Nexus 也支持多个，但共用一个记忆

#### Station

- Station 可以直接包含工具，直接在本进程调用，也可以配置 SubStation，远程调用其他 Station 的工具，SubStation 可以级联，但不可成环
- Station 可以将其包含的工具划分多个 group，SubStation 和通用 MCP中的工具自动形成单独的 group

#### Nexus

##### 基本流程

1. ChannelManager 实现 Terminal trait，从各 Channel 实例中接收消息
2. Channel 绑定 agent_id + role_name，并可以选择使用 Role 模式，或 Event 模式
  - Role 模式只有一套运行环境
  - Event 模式中，每个 Event 有一套运行环境
3. 根据 Channel 绑定的 agent_id 和 role 找到对应的 Pipeline 实例，Pipeline管理整个消息处理流程

##### 默认 Pipeline
- 默认 Pipeline 实现标准的 agentic loop 工作流：将输入发送至 LLM，LLM 返回文本回复，或者 Tool Call。收到 Tool Call 时，执行后将结果发回 LLM，等待回复；否则不再循环，回复文本回复
- 收到 Channel 消息时按时间等待，一个批次启动一次循环
- 一个 Pipeline 实例对应一个 session
- 当上下文长度超过阈值时，发送压缩提示词至 LLM 进行压缩

##### 默认记忆 Pipeline
- 默认记忆 Pipeline 在默认 Pipeline 的基础上，额外处理记忆相关的逻辑：
  1. 收到 Channel 消息时记录到 memory-store
  2. 收到 LLM 回复文本消息时，将回复记录到 memory-store
  3. LLM 回复带思考消息时，将思考消息合并到回复消息，随回复记录到 memory-store
  4. 收到 LLM Tool Call 时，执行 Tool Call，将 Tool Call 请求和结果都合并到回复消息，随回复消息记录到 memory-store
  5. （可选）当新建上下文，或者上下文长度超过阈值时，从 memory-store 中提取记忆，替换当前上下文

##### 记忆摘要 Pipeline

- 执行流程
  1. 收到 Channel 消息时，按照 Channel 合并批次，Channel 批次超时后处理一次
  2. 首先将 Channel 的批次消息发送到记忆摘要组件，记忆摘要组件把每条消息都归属到一个事件中，返回消息和事件的对应关系
  3. 对一一个批次消息的每个事件，启动一次 agentic loop，其中产生的消息均归到该事件
    - 如果没有对应 session，从记忆摘要中读取摘要和最近消息，作为 session 上下文
    - 如果有对应 session 且没有超过上下文长度阈值，则继续使用原 session 上下文，仅追加新消息

## 系统内通信

- 【WS】   Nexus 和 Channel 间双向传递消息和附件
- 【HTTP】 Nexus 向 memory-store 发送记忆
- 【HTTP】 Nexus 向 memory-ego 查询自我认知
- 【HTTP】 Nexus 从 memory-store 和 memory-struct-* 查询记忆
- 【HTTP】 Station 向子 Station 发送工具查询和工具调用
- 【WS】   memory-store 通知 memory-struct-* 有新记忆

## 路线图

### 笔记工具
- 可以将事件记录到工作区的MD文档

### 文件索引工具
- 记录 agent 运行过程中涉及的文件的信息，并分类索引。
  - 包括：agent 在工作区中写入的文件（通过工具调用生成、下载、复制取得的文件）；
  - 包括：用户复制进来，要求 agent 访问的文件
  - 不包括：其他程序运行过程中产生的临时文件
  - 不包括：agent 的 session 文件或记忆文件

### 主动 agent
- 在 agent 的自我认知中配置 agent 的目标，定时向 Nexus 发送 agent 目标，触发 agent 主动行动

### 多实例MCP服务
- 不同实例不同沙盒

### 专用客户端
- **起因** channel以及channel-web的功能更多的是在多人、多agent的环境中按人的方式收发消息，而不适合当前将agent作为生产力提升工具的方案。
- **目标** 实现 Channel 不具备的能力
  1. agent流式输出、打断、思考、工具调用等非人类回复者功能
  2. 各项配置的管理功能

### Station 工作区
- Station 可以用于替代工作区概念，甚至可以用于替代沙盒概念
- 插件平台架构：一个 Station 可以作为一个平台，在其上可以添加插件，这些插件可以实现新的工具，并使用 Station 平台提供的 SDK

### 触发流程
- 扩展 agentic loop 的启动机制：从单一的 Channel 触发，改为订阅不同的消息源
