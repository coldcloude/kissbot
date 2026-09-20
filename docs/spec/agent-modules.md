# kissbot-agent 组件调用关系

> 说明 kissbot-agent（后台 Rust 实现）内部各模块之间的调用关系，以及与外部的交互边界。
> 代码位置：`kissbot-agent/src/`。整体技术栈与部署形态见 [technical-architecture.md](technical-architecture.md)。

## 一、组件清单

| 模块 | 关键类型 | 职责 | 主要调用方 / 依赖 |
|------|----------|------|-------------------|
| main | — | 启动入口：加载配置、构建全局单例（ConfigManager → Station → Nexus）、spawn 管理 API 与 station MCP 服务端、进入主循环 | 调 ConfigManager / Station / Nexus / HttpServer / StationHttpServer |
| config_manager | ConfigManager | 配置加载与读写（channels / providers+models / memory_structs / (agent, role) 配置 / session 配置 / station / admin）；合成 provider+model 与 (agent, role)+session 的有效配置 | 被几乎所有模块调用 |
| nexus | Nexus | **编排中心**：消息业务处理（管理命令分发 / 普通消息路由）、会话定位与构建、通道连接调度、回复发送、全局单例装配 | 持有全部运行时组件；被 channel_manager（Terminal 回调和变更消费者）、pipelines（回调 Nexus 单例）调用 |
| channel_manager | ChannelManager / Channel | **Terminal 实现**（通道适配层：回显过滤 + 转发业务）；每 channel 运行态（pending msg_id 回显判定、mode、client、断线通知）；连接 / 重连 / 发送封装；channel 变更串行队列（含执行体） | 被 nexus 调用；被 channel-client 回调（Terminal）；队列消费者回经 Nexus 做会话重定位 |
| session_manager | SessionManager / Session / SessionContext | 会话三元组去重创建（返回 `(Session, 是否新建)`）；会话上下文一体管理（SessionContext：内存消息 + 本地缓存 + 历史归档，三者格式一致） | 被 nexus 调用 |
| pipeline | AgentPipeline / AgentTrigger / Agent{Input,Message,Tool,Output}… 等 trait | 定义 agentic loop 的执行体抽象：输入处理器、消息发送器、工具调用器、输出处理器、系统提示词器，以及无手动触发的 trigger 标记 trait；AgentPipeline 装配时注入会话并直接读写其上下文 | 被 pipelines 实现/组合；AgentPipeline::run 是 agentic loop 本体 |
| pipelines | PipelineManager / SessionPipeline / BatchAgentInputProcessor / TokenLimitMessageSender / ChannelOutputProcessor / DefaultSystemPrompter … | pipeline 体系实现与注册表：按会话 key 装配（装配时注入会话；输入处理器 / 系统提示词器 / trigger 为按名索引的多实例集合，运行中的 pipeline 另有独立锁）、工厂与 preset 映射、保留/清理；合批输入（mpsc + DelayQueue）在 flush 时回调 `Nexus::run_pipeline` | 被 nexus 调用；实现体回调 Nexus 单例（Provider、记忆、Station、回复发送） |
| command_router | CommandRouter | 管理命令解析与执行（/bind、/unbind、/admin、/unadmin、/agent、/role、/mode、/bind-outgoing、/unbind-outgoing、/model） | 被 nexus 调用；执行时读 config、调 nexus / channel_manager |
| message | MessageContent / pack_memory_messages / pack_batch | 消息内容组装：extract_content（record/event → 文本段）、记忆交替序列打包、batch → 单条 User 打包 | 被 memory_store_client / nexus（记忆回溯打包）/ pipelines（合批 flush）调用 |
| memory_store_client | MemoryStoreClient | memory-store 读写：推记录（channel / think / tool_call / tool_result，写入经 appender 批量投递）+ 读记忆（最近 N + 时间段两次查询并集打包） | 被 nexus 调用 |
| memory_ego_client | MemoryEgoClient | ego REST 客户端（agent / individual / role 查询）；system_prompt_for_agent / verify_agent_exists 经它发请求 | 被 nexus 调用 |
| provider | ProviderManager / OpenAiProvider / AnthropicProvider | 按 provider 配置分派实现：构造请求、调用 API、归一化响应（含 thinking / reasoning_content / tool_calls / usage）；模型列表查询 | 被 nexus 调用（`call_provider_model` / `list_models`） |
| station | Station / Group / GroupTool / SubStation / NativeTool / McpClient | 全局 Station 单例：本级 group 集合（工具元数据 + 原生实现，由内置注册表按 group 名填充）+ 直接子 Station（仅连接信息，持 McpClient）；实现 McpHandler（server/discover / tools/list / tools/call），list_tools(filter, ancestors) 平铺查询 / call_tool | 被 nexus（工具分组白名单）与 pipelines（工具调用适配层）调用；被 station_http 挂载为 MCP 处理器 |
| station_http | StationHttpServer / McpServer | station 对外 MCP 服务端（kissbot-mcp 的 McpServer + 单端点 `POST /mcp`，方法 server/discover / tools/list / tools/call），认证走 security.api_key | 被 main 调用；供其他 station 作为 sub 调用 |
| ego_md | build_ego_identity_md / build_ego_individual_recognition_md / build_role_play_md | ego 结构 → 系统提示词 markdown | 被 nexus（system_prompt_for_agent）调用 |
| http_server | HttpServer | 管理 API（config CRUD），认证走 kissbot-security | 被 main 调用；仅操作 ConfigManager |
| types | Mode / Message / SessionKey / ToolCall / ChannelCommand / ModelResponse … | 共享数据类型 | 被几乎所有模块引用 |

## 二、组件调用关系总览

```mermaid
graph TB
    subgraph Ext["外部组件"]
        CC["kissbot-channel-client<br/>ChannelClient（WSS）"]
        MS["kissbot-memory-store"]
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
        SM["session_manager<br/>SessionManager / Session<br/>SessionContext（内存+缓存+历史）"]
        PL["pipeline<br/>AgentPipeline / 各执行体 trait"]
        PLS["pipelines<br/>PipelineManager / SessionPipeline<br/>合批输入 / 发送器 / 工具调用 / 输出处理 / 提示词"]
        CRT["command_router<br/>CommandRouter"]
        MSG["message<br/>MessageContent<br/>pack_memory_messages / pack_batch"]
        MSC["memory_store_client<br/>MemoryStoreClient"]
        MEC["memory_ego_client<br/>MemoryEgoClient"]
        PRV["provider<br/>ProviderManager"]
        ST["station<br/>Station / Group / SubStation<br/>NativeTool"]
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
    CO --> ST
    CO --> EGO
    CO --> CRT
    CRT --> CFG
    CRT -->|命令执行回调| CO
    CRT -->|channel 侧入口| CHM
    PLS --> PL
    PLS --> CFG
    PLS --> MSG
    PLS -->|会话 / 记忆 / Provider / Station / 回复| CO
    CHM -. 创建 client / 重连循环 .-> CC
    CC -. Terminal 回调（incoming_message 等） .-> CHM
    CHM -->|回显过滤后转发业务| CO
    CHM -->|变更队列执行体：会话重定位| CO
    MSC --> MS
    CO -->|system_prompt_for_agent / verify_agent_exists| MEC
    MEC --> EG
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
        CO->>SM: ensure_session(key)（get_or_create：仅新建时从缓存恢复上下文）
        Note over CO: 新建会话时顺带 sync_pipeline（装配 pipeline/输入处理器/系统提示词器）+ reset_system_prompt（存为待定）
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
    participant IP as BatchAgentInputProcessor

    CC->>CHM: incoming_message(channel_id, event)（Terminal 回调）
    CHM->>CHM: consume_pending(msg_id)（回显判定，通道层过滤）
    alt 命中回显（自身发出）
        Note over CHM: 消费并丢弃，不转发业务
    else 非回显
        CHM->>CO: incoming_message(channel_id, event)（转发业务）
        CO->>CO: config.channel(channel_id) 不存在则丢弃
        CO->>MSC: push_channel_record(is_self=0)
        alt 管理命令（Content::Text 以 "/" 开头 + admins 命中）
            CO->>CRT: CommandRouter::execute(text, channel_id)
            CRT-->>CO: reply
            CO->>CHM: send（发回来源 channel + 记录 pending；命令不入记忆）
        else 系统事件（群组变更 / 用户移除）
            Note over CO: 直接返回，不进 pipeline
        else 普通消息
            CO->>SM: ensure_session(key)（不存在则新建 + pipeline 同步 + 系统提示词）
            CO->>PLS: incoming_message(key, event)
            PLS->>IP: accept(event)（mpsc 入队 + 设置 deadline）
            Note over IP: 消费任务（DelayQueue）到期 drain → pack_batch → Nexus::run_pipeline
        end
    end
```

## 五、Agentic Loop（LLM 回复 / 工具调用）

```mermaid
sequenceDiagram
    participant PL as AgentPipeline（run / run_once）
    participant MSN as AgentMessageSender（raw / compress / memory_recover）
    participant CO as Nexus
    participant CTX as SessionContext（内存 + 缓存 + 历史一体）
    participant PRV as ProviderManager
    participant OUT as AgentOutputProcessor（channel）
    participant ST as Station
    participant MSC as MemoryStoreClient
    participant CHM as ChannelManager

    Note over PL: 合批 flush → Nexus::run_pipeline（tools 由 Nexus::tools_for_session 预取）<br/>→ AgentPipeline::run(message, tools)；会话在装配时已注入，运行期不经 Nexus 反查
    loop 多轮（≤ MAX_TOOL_ROUNDS = 10）
        PL->>MSN: send_messages(messages, tools)
        alt TokenLimitMessageSender（compress / memory_recover）
            MSN->>MSN: max_tokens_usage > 0 且上次 usage.total_tokens > max_tokens_usage × compress_threshold（默认 0.8）
            Note over MSN: 超阈值 → reducer：压缩（LLM 总结）或记忆回溯（memory-store 打包）
        end
        MSN->>CTX: context_append(messages) → build_context()（应用待定系统消息）
        MSN->>CO: call_provider_model(llm_cfg, messages, tools)
        CO->>PRV: ProviderManager::call（按 provider_type 分派 openai / anthropic）
        PRV-->>MSN: ModelResponse（content / reasoning_content / thinking / tool_calls / total_tokens）
        MSN->>MSN: 记录本次 usage.total_tokens（供下次阈值判定）
        PL->>MSC: send_memory_think（reasoning_content / thinking 任一有值，每轮成功响应都推）
        PL->>OUT: accept(round, response)
        alt 有 tool_calls
            OUT->>CTX: 追加 Assistant（raw_content / raw_reasoning_content / raw_tool_calls）
            loop 每个 tool call（并发 join）
                PL->>MSC: send_memory_tool_call（UUID key）
                PL->>ST: Station::call_tool（本级原生实现 / 路由到子 Station 的 MCP tools/call）
                PL->>MSC: send_memory_tool_result（同 key）
                OUT->>CTX: 追加 Tool（tool_call_id + content=result 或 error）
            end
            Note over OUT: 有 tool_calls 且未达上限 → next=true，进入下一轮
        else 最终回复
            OUT->>CTX: 追加 Assistant
            OUT->>CO: send_outgoing（校验 out_channel 绑定 → CHM.send）
        end
    end
    PL->>CTX: 无下一轮时把本轮新消息追加进上下文
```

## 六、会话构建与上下文管理

```mermaid
sequenceDiagram
    participant CO as Nexus
    participant SM as SessionManager
    participant SC as SessionContext（内存 + 缓存 + 历史一体）
    participant PLS as PipelineManager
    participant SP as AgentSystemPrompter（default / memory_ego）
    participant MEC as MemoryEgoClient

    CO->>SM: ensure_session → get_or_create(key) → (session, 是否新建)
    alt 新建会话
        SM->>SC: recover_from_cache()（正序全量回读；缓存 System 首行 → 当前系统消息）
        CO->>PLS: sync_pipeline(key, session)（按 preset / 显式配置装配，注入会话；输入处理器/系统提示词器/trigger 按名索引）
        CO->>SP: reset_system_prompt()
        alt memory_ego prompter
            SP->>MEC: system_prompt_for_agent（agent 元数据 + 个体识别 + 角色设定 → ego_md 转 markdown）
            Note over SP: 保留 agent（agent_id="0"）/ 查询失败 → 回退默认提示词
        end
        SP->>SC: set_system_message（只存待定）
    else 已存在会话
        Note over CO: 快速路径命中，直接复用（不重复恢复/装配）
    end
    Note over CO: 系统消息存待定；下次 build_context 时 apply_pending_system 对比应用<br/>记忆回溯在 TokenLimitMessageSender 内按阈值触发（memory_recover reducer → build_context_from_memory_store 读 memory-store）
```

## 七、对外交互边界

| 方向 | 组件 | 协议 | 说明 |
|------|------|------|------|
| agent → channel | kissbot-channel-client | WSS | agent 作为客户端连接消息通道；ChannelClient 持 `Weak<dyn Terminal>` 回调 ChannelManager（Terminal 实现：incoming_message / join_group / leave_group / user_removed / download_chunk / closed）；ChannelManager 回显过滤后转 Nexus |
| agent → memory-store | kissbot-memory-store | HTTP | MemoryStoreClient 推记录（`/store/*`）+ 读记忆（`/store/query/channel*`） |
| agent → memory-ego | kissbot-memory-ego | HTTP | MemoryEgoClient 查询（`/agent/get`、`/individual/get-all`、`/role/get`）；system_prompt_for_agent / verify_agent_exists 经它发请求 |
| agent → LLM | 模型提供方 API | HTTP | ProviderManager 按 provider_type 分派（openai: `/chat/completions`、anthropic: `/v1/messages`），支持重试、工具调用、thinking/reasoning 回传 |
| agent ← 管理界面 | HttpServer | HTTP | 当前仅 config CRUD（操作 ConfigManager）；管理命令走 channel 消息（/ 开头） |
| agent ← station（sub） | StationHttpServer | MCP（Streamable HTTP） | 单端点 `POST /mcp`，供其他 station 作为 sub 调用（server/discover、tools/list、tools/call），认证 security.api_key |

## 八、未接线 / 预留

- **无手动触发的 trigger（`SessionPipeline.triggers`）**：类型与保留位已建（`AgentTrigger` 标记 trait），当前装配时为空列表，尚无生产实现。
- **memory-struct**：`NexusRepo.memory_structs` 与 `MemoryStructConfig` 仅建结构，无消费方（记忆索引 / 记忆搜索工具未实现；上下文回溯走 memory-store 的两查询并集）。
- **Terminal（ChannelManager）的 join_group / leave_group / user_removed / download_chunk**：回调为 no-op / 未使用，业务逻辑预留。
- **`ModelResponse.finish_reason`**：已解析但无消费方（截断等停止原因不触发额外处理）。
- **http_server ↔ nexus**：管理命令（/bind、/mode 等）目前由 channel 上行消息触发，HttpServer 未直接调用 nexus。
- **配置热更新监听**：ConfigManager 未提供变更监听，接入 Station/Nexus 为遗留事项。
