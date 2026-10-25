# kissbot-agent 组件调用关系

> 说明 kissbot-agent（后台 Rust 实现）内部各模块之间的调用关系，以及与外部的交互边界。
> 代码位置：`kissbot-agent/src/`。整体技术栈与部署形态见 [technical-architecture.md](technical-architecture.md)。

## 一、组件清单

| 模块 | 关键类型 | 职责 | 主要调用方 / 依赖 |
|------|----------|------|-------------------|
| main | — | 启动入口：加载配置、构建全局单例（ConfigManager → Station → Nexus）、spawn 管理 API 与 station MCP 服务端、进入主循环 | 调 ConfigManager / Station / Nexus / HttpServer / StationHttpServer |
| config_manager | ConfigManager | 配置加载与读写（channels / providers+models / memory_structs / (agent, role) 配置 / session 配置 / station / admin）；建立 session 时按 memory_key 的 agent/role 合成一次 session 配置，读取按 session_id，pipeline 选择按 (agent, role) 合成 | 被几乎所有模块调用 |
| nexus | Nexus | **编排中心**：消息业务处理（管理命令分发 / 普通消息路由）、pipeline 定位与装配、通道连接调度、全局单例装配 | 持有全部运行时组件；被 channel_manager（Terminal 回调和变更消费者）、pipelines（回调 Nexus 单例）调用 |
| channel_manager | ChannelManager / Channel | **Terminal 实现**（通道适配层：回显过滤 + 转发业务）；每 channel 运行态（pending msg_id 回显判定、event、client、断线通知）；连接 / 重连 / 发送封装；channel 变更串行队列（含执行体） | 被 nexus 调用；被 channel-client 回调（Terminal）；队列消费者回经 Nexus 做 pipeline 重定位 |
| session_manager | SessionManager | 按 session_id（`ArcStr`）去重创建（返回 `(Session, 是否新建)`）、按 id 清理；session 数量与 id 由 pipeline 决定 | 被 nexus 与 pipeline 调用；持有 kissbot-llm 的 Session |
| pipeline | AgentPipeline / IncomingMessageProcessor / IncomingMessageBatchProcessor / TokenReducer | 定义 pipeline 的执行体抽象：消息接入（单条 / 整批）、token 压缩；`AgentPipeline` 提供 `reset_system_prompt`、`set_model`、`session_ids` 与 `channel_unbound`；工具调用直接用 pipelines 的 call_tool / call_tool_with_memory（并发 join） | 被 pipelines 实现/组合 |
| pipelines | PipelineManager / DefaultAgenticLoopPipeline / DefaultAgenticLoopPipelineWithMemory / AbstractMemoryPipeline / BatchProducer / CompressTokenReducer / MemoryRecoverTokenReducer | pipeline 体系实现与注册表：按 `Arc<MemoryKey>` 装配（pipeline 自行创建/持有 session）、工厂（`default` / `default_memory` / `abstract_memory`）、保留/清理；摘要记忆 pipeline 为每个 channel 维护独立批次生产者，触发时先 locate 并按 case 拆成任务入队，case 串行执行、每个 case 使用独立 session 并用该 case 的摘要与结尾原文重建上下文，其 agentic loop 复用本模块的发送/工具/提示词函数 | 被 nexus 调用；实现体回调 Nexus 单例（Provider、记忆、Station、回复发送） |
| command_router | CommandRouter | 管理命令解析与执行（/bind、/unbind、/admin、/unadmin、/agent、/role、/mode、/bind-outgoing、/unbind-outgoing、/model） | 被 nexus 调用；执行时读 config、调 nexus / channel_manager |
| message | MessageContent / pack_memory_messages / pack_batch | 消息内容组装：extract_content（record/event → 文本段）、记忆交替序列打包、batch → 单条 User 打包 | 被 memory_store_client / nexus（记忆回溯打包）/ pipelines（合批 flush、摘要记忆上下文）调用 |
| memory_store_client | MemoryStoreClient | memory-store 读写：推记录（channel / think / tool_call / tool_result，写入经 appender 批量投递并等待落盘）+ 读记忆（最近 N + 时间段两次查询并集打包） | 被 nexus 调用 |
| memory_ego_client | MemoryEgoClient | ego REST 客户端（agent / individual / role 查询）；system_prompt_for_agent / verify_agent_exists 经它发请求 | 被 nexus 调用 |
| abstract_client | AbstractClient | 摘要记忆（memory-struct-abstract）REST 客户端：case 定位、case 摘要、case 原文副本尾部查询 | Nexus 成员；AbstractMemoryPipeline 经 Nexus 单例调用 |
| provider | ProviderManager | provider 工厂与分派（agent 运行时组件）：按 provider 名缓存实现实例（同名去重），解析有效模型配置并按 provider_type 分派调用 / 模型列表查询 | 被 nexus 调用（`call_provider_model` / `verify_model`）；实现与重试来自 kissbot-llm |
| kissbot-llm（库依赖） | Provider / Session / SessionContext / Message / ToolConfig / ProviderModelConfig… | provider 实现构造（`create_*_provider`）与带重试调用（`call_with_retry`）、模型列表；单会话上下文（内存 + 本地缓存 + 历史归档一体）；LLM 配置与消息类型 | 被 provider（构造实现 / 带重试调用）、nexus 与 pipelines（会话上下文）使用 |
| station | Station / NativeTool / Group / ToolImplement / SubStation / McpClient | 全局 Station 单例：本级工具索引（工具元数据 + 原生实现，由内置注册表按 group 名给出）+ 直接子 Station（仅连接信息，持 McpClient）；实现 McpHandler（server/discover / tools/list / tools/call），list_tools(ancestors) 平铺查询 / list_tools_local(groups) 按会话白名单本地过滤 / call_tool_local | 被 nexus（工具分组白名单）与 pipelines（工具调用适配层）调用；被 station_http 挂载为 MCP 处理器 |
| station_http | StationHttpServer / McpServer | station 对外 MCP 服务端（kissbot-mcp 的 McpServer + 单端点 `POST /mcp`，方法 server/discover / tools/list / tools/call），认证走 security.api_key | 被 main 调用；供其他 station 作为 sub 调用 |
| ego_md | build_ego_identity_md / build_ego_individual_recognition_md / build_role_play_md | ego 结构 → 系统提示词 markdown | 被 nexus（system_prompt_for_agent）调用 |
| http_server | HttpServer | 管理 API（config CRUD），认证走 kissbot-security | 被 main 调用；仅操作 ConfigManager |
| types | MemoryKey / ChannelCommand / RoleEvent 用法 | agent 侧共享数据类型（错误、pipeline 标识、通道命令与 MCP→LLM 工具定义转换）；LLM 消息与配置类型见 kissbot-llm | 被几乎所有模块引用 |

## 二、组件调用关系总览

```mermaid
graph TB
    subgraph Ext["外部组件"]
        CC["kissbot-channel-client<br/>ChannelClient（WSS）"]
        MS["kissbot-memory-store"]
        MSA["kissbot-memory-struct-abstract"]
        EG["kissbot-memory-ego"]
        LLM["LLM API"]
        UI["管理界面 / HTTP"]
        SUB["其他 station（作为 sub）"]
    end

    subgraph Agent["kissbot-agent"]
        MAIN["main"]
        CFG["config_manager<br/>ConfigManager"]
        CO["nexus<br/>Nexus"]
        CHM["channel_manager<br/>ChannelManager / Channel<br/>+ Terminal 实现 + 变更队列"]
        SM["session_manager<br/>SessionManager<br/>（按 session_id 保存与清理；session 由 pipeline 决定）"]
        PL["pipeline<br/>AgentPipeline / 各执行体 trait"]
        PLS["pipelines<br/>PipelineManager / AgentPipeline 实现<br/>批次生产者 / reducer"]
        CRT["command_router<br/>CommandRouter"]
        MSG["message<br/>MessageContent<br/>pack_memory_messages / pack_batch"]
        MSC["memory_store_client<br/>MemoryStoreClient"]
        MEC["memory_ego_client<br/>MemoryEgoClient"]
        ABC["abstract_client<br/>AbstractClient"]
        PRV["provider<br/>ProviderManager（工厂 + 分派）"]
        KLLM["kissbot-llm（库）<br/>provider 实现 / Session<br/>LLM 配置与消息类型"]
        ST["station<br/>Station / SubStation<br/>NativeTool"]
        EGO["ego_md"]
        HS["http_server<br/>HttpServer"]
        SHS["station_http<br/>StationHttpServer（MCP 服务端）"]
        MCP["kissbot-mcp（库）<br/>protocol / server / http / client"]
    end

    MAIN --> CFG
    MAIN --> CO
    MAIN --> HS
    MAIN --> SHS
    HS --> CFG
    CO --> CFG
    CO --> CHM
    CO --> SM
    CO --> PLS
    CO --> MSG
    CO --> MSC
    CO --> MEC
    CO --> PRV
    PRV --> KLLM
    CO --> KLLM
    PLS --> KLLM
    CO --> ST
    CO --> EGO
    CO --> CRT
    CRT --> CFG
    CRT -->|命令执行回调| CO
    CRT -->|channel 侧入口| CHM
    PLS --> PL
    PLS --> CFG
    PLS --> MSG
    PLS -->|pipeline / session / 记忆 / Provider / Station / 回复| CO
    CHM -. 创建 client / 重连循环 .-> CC
    CC -. Terminal 回调（incoming_message 等） .-> CHM
    CHM -->|回显过滤后转发业务| CO
    CHM -->|变更队列执行体：pipeline 重定位| CO
    MSC --> MS
    CO -->|system_prompt_for_agent / verify_agent_exists| MEC
    MEC --> EG
    CO --> ABC
    ABC --> MSA
    PRV --> LLM
    ST --> MCP
    SHS --> MCP
    ST -->|MCP 客户端调用子 Station| SHS
    SUB --> SHS
    UI --> HS
```

## 三、启动流程

```mermaid
sequenceDiagram
    participant main
    participant CFG as ConfigManager
    participant ST as Station
    participant CO as Nexus
    participant CHM as ChannelManager
    participant SM as SessionManager
    participant HS as HttpServer

    main->>CFG: new()（读 KISSBOT_CONFIG 的 agent 段，引导 nexus.json / station.json，注册全局单例）
    main->>ST: Station::new()（读 station.json 构建全局单例）
    main->>CO: Nexus::new()（装配运行时组件 + 注册单例）
    Note over CHM: ChannelManager::new 内建 channel 变更队列并 spawn 消费者
    main->>HS: start()（后台管理 API，spawn）
    main->>main: StationHttpServer::start()（后台 station MCP 服务端，spawn）
    main->>CO: run()
    loop 每个 channel 绑定
        CO->>PLS: ensure_pipeline(Arc<MemoryKey>)（按 agent/role 的 pipeline_name 装配）
        PLS->>SM: pipeline 按自己的 session_id get_or_create（新建时从缓存恢复）
        PLS->>CFG: ensure_session_config(memory_key, session_id)（新建时按 agent/role 合成一次）
        Note over PLS: pipeline 初始化系统提示词（存为待定）
    end
    loop 每个 enabled channel
        CO->>CHM: connect_channel(channel_id)（创建 ChannelClient + spawn 重连循环）
    end
    Note over CO: 保持进程；channel 回调经 Terminal（ChannelManager）驱动
```

## 四、上行消息处理

```mermaid
sequenceDiagram
    participant CC as ChannelClient
    participant CHM as ChannelManager（Terminal）
    participant CO as Nexus
    participant MSC as MemoryStoreClient
    participant CRT as CommandRouter
    participant SM as SessionManager
    participant PLS as PipelineManager
    participant IP as pipeline（合批 / 摘要记忆）

    CC->>CHM: incoming_message(channel_id, event)（Terminal 回调）
    CHM->>CHM: consume_pending(msg_id)（回显判定，通道层过滤）
    alt 命中回显（自身发出）
        Note over CHM: 消费并丢弃，不转发业务
    else 非回显
        CHM->>CO: incoming_message(channel_id, event)（转发业务）
        CO->>CO: config.channel(channel_id) 不存在则丢弃
        alt 管理命令（Content::Text 以 "/" 开头 + admins 命中）
            CO->>CRT: CommandRouter::execute(text, channel_id)
            CRT-->>CO: reply
            CO->>CHM: send（发回来源 channel + 记录 pending；命令不入记忆）
        else 系统事件（群组变更 / 用户移除）
            Note over CO: 直接返回，不进 pipeline
        else 普通消息
            CO->>PLS: incoming_message(Arc<MemoryKey>, event)（不存在则先 ensure_pipeline）
            PLS->>IP: incoming_message(event)（该 pipeline 的实例）
            alt default_memory
                IP->>MSC: push_channel_record(is_self=0, meta=None)
                Note over IP: 合批（mpsc + deadline）→ pack_batch → agentic loop
            else abstract_memory
                Note over IP: 按 channel 独立攒批（BatchProducer DashMap）
                IP->>IP: locate → 写上行记录(meta=case) → 按 case 拆任务入队
                Note over IP: 每个 case 一个 session（懒创建）；case 队列串行 run（同一 case 排队中合并）：该 case 摘要+tail 重建自己的 session → pack_batch → agentic loop（回复记录 meta=case）
            else default
                Note over IP: 合批 → pack_batch → agentic loop（不写记忆）
            end
        end
    end
```

## 五、Agentic Loop（LLM 回复 / 工具调用）

```mermaid
sequenceDiagram
    participant PL as AgentPipeline（run / run_once）
    participant MSN as 发送与 reducer（send_messages_with_limit）
    participant CO as Nexus
    participant CTX as SessionContext（kissbot-llm：内存 + 缓存 + 历史一体）
    participant PRV as ProviderManager（工厂 + 分派）
    participant OUT as AgentOutputProcessor（channel）
    participant ST as Station
    participant MSC as MemoryStoreClient
    participant CHM as ChannelManager

    Note over PL: pipeline 触发（合批 / 摘要记忆）→ 各自的 run(message, tools)；tools 由 tools_for_session 预取，会话在装配时已注入
    loop 多轮（≤ MAX_TOOL_ROUNDS = 10）
        PL->>MSN: send_messages(messages, tools)
        alt send_messages_with_limit（compress / memory_recover）
            MSN->>MSN: max_tokens_usage > 0 且上次 usage.total_tokens > max_tokens_usage × compress_threshold（默认 0.8）
            MSN->>MSN: 超阈值 → reducer 处理，返回仍需追加的消息
            Note over MSN: 压缩：只读 build_context 出上下文（不应用待定系统消息、不触发重建）并只从中弹出最后一条 assistant（含）及之后的消息 → 剩余部分 + 压缩指令 → LLM 总结<br/>→ 一次重置为「压缩指令 + 总结 + 弹出的消息 + 新消息」（返回空；重置时应用待定系统消息）<br/>记忆回溯：只读取最后一条 assistant（含）及之后的会话消息 → 拼接 memory-store 打包结果 → 一次重置
        end
        MSN->>CTX: context_append(仍需追加的消息) → build_context_full()（应用待定系统消息）
        MSN->>CO: call_provider_model(llm_cfg, messages, tools)
        CO->>PRV: call（取有效模型配置 + provider_type 分派 + 实现实例工厂去重）
        PRV->>PRV: kissbot-llm call_with_retry（超时/重试来自有效模型配置）
        PRV-->>MSN: ModelResponse（content / reasoning_content / thinking / tool_calls / total_tokens）
        MSN->>MSN: 记录本次 usage.total_tokens（供下次阈值判定）
        PL->>MSC: send_memory_think（reasoning_content / thinking 任一有值，每轮成功响应都推；等待详情落盘）
        PL->>OUT: accept(round, response)
        alt 有 tool_calls
            OUT->>CTX: 追加 Assistant（raw_content / raw_reasoning_content / raw_tool_calls）
            loop 每个 tool call（并发 join；带记忆的 pipeline 在全部完成后收集详情引用）
                PL->>MSC: send_memory_tool_call（UUID key，等待详情落盘）
                PL->>ST: Station::call_tool（本级原生实现 / 路由到子 Station 的 MCP tools/call）
                PL->>MSC: send_memory_tool_result（同 key，等待详情落盘）
                OUT->>CTX: 追加 Tool（tool_call_id + content=result 或 error）
            end
            Note over OUT: 本轮没有文本 → 不发送，占位内容留到有文本的轮次一起发送<br/>有 tool_calls 且未达上限 → next=true，进入下一轮
        else 最终回复
            OUT->>CTX: 追加 Assistant
            PL->>CO: send_outgoing（本次 run 累积的 think/tool 占位引用 + 本轮文本；详情已落盘，摘要记忆带当前 case 的 meta）
        end
    end
    PL->>CTX: 无下一轮时把本轮新消息追加进上下文
```

## 六、pipeline 装配与上下文管理

```mermaid
sequenceDiagram
    participant CO as Nexus
    participant PLS as PipelineManager
    participant PL as AgentPipeline
    participant SM as SessionManager
    participant CFG as ConfigManager
    participant SC as SessionContext（kissbot-llm：内存 + 缓存 + 历史一体）
    participant SP as AgentPipeline::reset_system_prompt
    participant MEC as MemoryEgoClient

    CO->>PLS: ensure_pipeline(Arc<MemoryKey>)
    alt pipeline 未装配
        PLS->>PLS: 按 agent/role 的 pipeline_name 创建 pipeline
        alt default / default_memory
            PL->>SM: get_or_create(MemoryKey::encode()) → (session, 是否新建)
            PL->>CFG: 新建时 ensure_session_config(memory_key, session_id)
        else abstract_memory
            Note over PL: 构造时不建 session；基准（-none）session 在入口/set_model 时创建，case session 在首次 run 时创建
        end
        SM->>SC: 新建时 recover_from_cache()（正序全量回读；缓存 System 首行 → 当前系统消息）
        PLS->>SP: reset_system_prompt()
        alt memory-ego 可用
            SP->>MEC: system_prompt_for_agent（agent 元数据 + 个体识别 + 角色设定 → ego_md 转 markdown）
            Note over SP: 保留 agent（agent_id="0"）/ 查询失败 → 回退默认提示词
        end
        SP->>SC: set_system_message（只存待定）
    else 已装配
        Note over CO: 直接复用（不重复恢复/装配）
    end
    Note over CO: 系统消息存待定；下次 build_context_full 时 apply_pending_system 对比应用，重建/压缩时由 context_archive_and_clear_cache_and_reset_messages 一并应用<br/>记忆回溯在 send_messages_with_limit 内按阈值触发（memory_recover reducer 只读取最后一条 assistant 及之后的会话消息，拼接 build_context_from_memory_store 的打包结果后一次重置）
```

## 七、对外交互边界

| 方向 | 组件 | 协议 | 说明 |
|------|------|------|------|
| agent → channel | kissbot-channel-client | WSS | agent 作为客户端连接消息通道；ChannelClient 持 `Weak<dyn Terminal>` 回调 ChannelManager（Terminal 实现：incoming_message / join_group / leave_group / user_removed / download_chunk / closed）；ChannelManager 回显过滤后转 Nexus |
| agent → memory-store | kissbot-memory-store | HTTP | MemoryStoreClient 推记录（`/store/*`）+ 读记忆（`/store/query/channel*`） |
| agent → memory-ego | kissbot-memory-ego | HTTP | MemoryEgoClient 查询（`/agent/get`、`/individual/get-all`、`/role/get`）；system_prompt_for_agent / verify_agent_exists 经它发请求 |
| agent → LLM | 模型提供方 API | HTTP | ProviderManager 按 provider_type 取 kissbot-llm 的实现（openai: `/chat/completions`、anthropic: `/v1/messages`），经 `call_with_retry` 支持重试、工具调用、thinking/reasoning 回传 |
| agent ← 管理界面 | HttpServer | HTTP | 当前仅 config CRUD（操作 ConfigManager）；管理命令走 channel 消息（/ 开头） |
| agent ← station（sub） | StationHttpServer | MCP（Streamable HTTP） | 单端点 `POST /mcp`，供其他 station 作为 sub 调用（server/discover、tools/list、tools/call），认证 security.api_key |

## 八、未接线 / 预留

- **无手动触发的 trigger**：当前没有独立 trigger 集合；pipeline 自己维护批次生产者，channel 解绑与 pipeline 销毁时退出消费者任务。
- **memory-struct**：`NexusRepo.memory_structs` 与 `MemoryStructConfig` 仅建结构，无消费方；摘要记忆的上下文由 `AbstractMemoryPipeline` 经 memory-struct-abstract 提供，其余会话的上下文回溯走 memory-store 的两查询并集。
- **Terminal（ChannelManager）的 join_group / leave_group / user_removed / download_chunk**：回调为 no-op / 未使用，业务逻辑预留。
- **`ModelResponse.finish_reason`**：已解析但无消费方（截断等停止原因不触发额外处理）。
- **http_server ↔ nexus**：管理命令（/bind、/mode 等）目前由 channel 上行消息触发，HttpServer 未直接调用 nexus。
- **配置热更新监听**：ConfigManager 未提供变更监听，接入 Station/Nexus 为遗留事项。
