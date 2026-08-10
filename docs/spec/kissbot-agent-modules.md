# kissbot-agent 组件调用关系

> 说明 kissbot-agent（后台 Rust 实现）内部各模块之间的调用关系，以及与外部的交互边界。
> 代码位置：`kissbot-agent/src/`。整体技术栈与部署形态见 [technical-architecture.md](technical-architecture.md)。

## 一、组件清单

| 模块 | 关键类型 | 职责 | 主要调用方 / 依赖 |
|------|----------|------|-------------------|
| main | — | 启动入口：加载配置、构建 Coordinator、启动管理 API、进入主循环 | 调 ConfigManager / AgentCoordinator / HttpServer |
| config_manager | ConfigManager | 配置加载与读写（channel、provider/model、context、station、admin）；合成 effective 模型配置 | 被几乎所有模块调用 |
| coordinator | AgentCoordinator | **编排中心**：实现 Terminal trait、消息处理、会话管理、agentic loop、工具分派、命令执行、通道连接 | 持有全部运行时组件；被 command_router / session_manager（弱引用）回调 |
| channel_manager | ChannelManager / Channel | 每 channel 运行态：pending msg_id（回显判定）、agent_id、mode、client、合批 producer | 仅被 coordinator 调用；无外部依赖 |
| session_manager | SessionManager / Session / SessionContext / BatchProducer / BatchConsumer | 会话全功能：三元组去重创建、合批生产侧（producer）与触发任务（consumer，DelayQueue 定时 flush）、会话上下文一体化（SessionContext 内存 + 缓存 agent-data/context + 历史归档 agent-data/context-history；三者格式一致，均为 <session_key编码>.jsonl 每行一条 Message，首行 System（如有）；归档 = 直接把当前内存写成一个历史文件，不复制缓存；系统消息变更走待定懒切换：set 只存待定，下次发送前对比应用，不一致则旧上下文（含原系统消息）先归档再替换重建；持久化对 coordinator 透明） | 被 coordinator 调用；trigger 任务经 session.coordinator 弱引用回调 coordinator |
| command_router | CommandRouter | 管理命令解析与执行（/bind、/mode、/model、/reset 等） | 被 coordinator 调用；执行时回调 coordinator + 读 config |
| memory_reader | MemoryReader | 从 memory-store 读记忆构建上下文（组合查询 + 并集打包、事件列表、记忆索引） | 被 coordinator 调用；依赖 config + memory-store |
| memory_store_client | MemoryStoreClient | 向 memory-store 推记录（channel / tool_call / tool_result / think） | 被 coordinator 调用 |
| model_client | ModelClient | LLM 调用（多轮重试、工具调用）与模型列表校验 | 被 coordinator 调用；依赖 config（provider/model） |
| provider | Provider | 模型提供方（provider type → body 构造）解析 | 被 model_client 调用 |
| station | StationRuntime / Tool | 本地 tool 执行主机（内置 Read 工具等）；configured_tools / call_tool | 被 coordinator 调用 |
| station_client / station_router | StationClient / StationRouter | 远程 station 连接骨架与路由表 | **尚未接线**（预留；coordinator 当前只用本地 station） |
| ego_md | build_ego_* | ego 结构 → 系统提示词 markdown | 被 coordinator 调用 |
| http_server | HttpServer | 管理 API（config CRUD），认证走 kissbot-security | 被 main 调用；仅操作 ConfigManager |
| types | Mode / Message / SessionKey / ToolCall … | 共享数据类型 | 被几乎所有模块引用 |

## 二、组件调用关系总览

```mermaid
graph TB
    subgraph Ext["外部组件"]
        CC["kissbot-channel-client<br/>ChannelClient（WSS）"]
        MS["kissbot-memory-store"]
        EG["kissbot-memory-ego"]
        LLM["LLM API"]
        UI["管理界面 / HTTP"]
    end

    subgraph Agent["kissbot-agent"]
        MAIN["main"]
        CFG["config_manager<br/>ConfigManager"]
        CO["coordinator<br/>AgentCoordinator<br/>+ Terminal 实现"]
        CHM["channel_manager<br/>ChannelManager / Channel"]
        SM["session_manager<br/>SessionManager / Session<br/>BatchProducer / Consumer<br/>SessionContext（内存+缓存+历史）"]
        CRT["command_router<br/>CommandRouter"]
        MR["memory_reader<br/>MemoryReader"]
        MSC["memory_store_client<br/>MemoryStoreClient"]
        MLC["model_client<br/>ModelClient"]
        PRV["provider"]
        ST["station<br/>StationRuntime / Tool"]
        EGO["ego_md"]
        HS["http_server<br/>HttpServer"]
    end

    MAIN --> CFG
    MAIN --> CO
    MAIN --> HS
    HS --> CFG
    CO --> CFG
    CO --> CHM
    CO --> SM
    CO --> MR
    CO --> MSC
    CO --> MLC
    CO --> ST
    CO --> EGO
    CO --> CRT
    CRT --> CFG
    CRT -->|执行命令回调| CO
    MLC --> CFG
    MLC --> PRV
    MR --> CFG
    CO -. 创建 client / 重连循环 .-> CC
    CC -. Terminal 回调（incoming_message 等） .-> CO
    SM -. trigger flush 升级弱引用 .-> CO
    MSC --> MS
    MR --> MS
    CO -->|resolve_agent_id / load_ego_info| EG
    MLC --> LLM
    UI --> HS
```

## 三、启动流程

```mermaid
sequenceDiagram
    participant main
    participant CFG as ConfigManager
    participant CO as AgentCoordinator
    participant CHM as ChannelManager
    participant SM as SessionManager
    participant EG as memory-ego
    participant CC as ChannelClient

    main->>CFG: new()（加载/引导配置）
    main->>CO: new(config)
    CO->>CO: 校验 default_model（ModelClient.list_models）
    loop 每个 channel
        CO->>EG: resolve_agent_id_http(agent_name)（绑定运行态 agent_id）
        CO->>SM: ensure_session(key, channel_id)
        SM-->>CO: (Session, created)
        CO->>CHM: bind_producer(session.batch_producer)
    end
    CO->>CC: ChannelClient::new（id + 指向 coordinator 的 Terminal 弱引用）
    CO->>CHM: bind_client(channel_id, client)
    CO->>CC: connect(ws_url, api_key)（spawn 重连循环，绑定用户）
    main->>HS: start()（后台管理 API）
    main->>CO: run()（主循环保持进程）
```

## 四、上行消息处理

```mermaid
sequenceDiagram
    participant CC as ChannelClient
    participant CO as AgentCoordinator（Terminal）
    participant CHM as ChannelManager
    participant MSC as MemoryStoreClient
    participant CRT as CommandRouter
    participant SM as SessionManager
    participant BP as BatchProducer

    CC->>CO: incoming_message(channel_id, event)
    CO->>CO: config.channel(channel_id) 不存在则丢弃
    CO->>CHM: consume_pending(msg_id)（回显判定）
    alt 命中回显（自身发出）
        Note over CO: 跳过，不存记忆、不进 loop
    else 非回显
        CO->>MSC: push_channel_record(is_self=0)
        alt 管理命令（is_command + check_admin）
            CO->>CRT: parse + execute(config, coordinator)
            CRT-->>CO: (reply, effect)
            CO->>CHM: client(channel_id)
            CO->>CC: send_message(reply)（回来源 channel）
            CO->>CHM: add_pending(msg_id)
            CO->>MSC: push_channel_record(is_self=1)
            CO->>CO: 应用 effect（ResetSession → reset_session_for 等）
        else 普通消息
            CO->>SM: ensure_session(key, channel_id)
            CO->>BP: enqueue_batch（tx 入队 + set_deadline + Trigger::At）
            Note over BP: trigger 任务（DelayQueue）到期 → try_flush → 升级 coordinator 弱引用 → run_agentic_loop
        end
    end
```

## 五、Agentic Loop（LLM 回复 / 工具调用）

```mermaid
sequenceDiagram
    participant BP as BatchConsumer / trigger 任务
    participant CO as AgentCoordinator
    participant CTX as SessionContext（内存 + 缓存 + 历史一体）
    participant MLC as ModelClient
    participant ST as StationRuntime
    participant MSC as MemoryStoreClient
    participant CHM as ChannelManager
    participant CC as ChannelClient

    BP->>CO: resolve_out_channel_for_session + run_agentic_loop(session, out_channel)
    CO->>CTX: apply_pending_system()（系统消息对比应用：不一致→旧上下文归档→替换→重建缓存）
    CO->>CTX: append(User)（内存 + 缓存一体）
    loop 多轮（≤ MAX_TOOL_ROUNDS）
        CO->>MLC: call(pm, messages, tools)
        alt 返回 tool_calls
            CO->>CTX: append(Assistant + tool_calls)
            loop 每个 tool call
                CO->>MSC: push_channel_record(ToolCall 占位)
                CO->>ST: call_tool(name, args)
                CO->>CTX: append(Tool)
                CO->>MSC: push_tool_call / push_tool_result（同 key 关联）
                CO->>MSC: push_channel_record(ToolResult 占位)
            end
        else 最终回复
            CO->>CTX: append(Assistant)
            CO->>MSC: push_think（reasoning/thinking 任一有值）
            CO->>CHM: client(out_channel.channel_id)
            CO->>CC: send_message(reply)（发往 out_channel）
            CO->>CHM: add_pending(msg_id)
            CO->>MSC: push_channel_record(is_self=1)
            Note over CO: 溢出检查（effective.max_context_messages）<br/>event → compress_context；role → reset_context
        end
    end
```

## 六、会话构建与上下文管理

```mermaid
sequenceDiagram
    participant CO as AgentCoordinator
    participant EG as memory-ego
    participant SC as SessionContext（内存 + 缓存 + 历史一体）
    participant MR as MemoryReader
    participant SM as SessionManager
    participant BP as BatchProducer

    Note over CO: build_initial_context（会话创建 / 重置时）
    alt 保留 agent（agent_id="0"）
        CO->>CO: 用默认系统提示词（config.default_system_prompt）
    else 普通 agent
        CO->>EG: load_ego_info（agent 元数据 + 个体识别 + 角色设定 → markdown）
    end
    alt event 模式
        CO->>SC: recover_from_cache()（全量回读恢复；缓存 System 首行 → 当前系统消息）
    else role 模式
        CO->>SC: archive_and_clear_cache()（旧上下文归档，缓存清空）
        CO->>MR: read_recent_for_context（组合查询 + 并集打包为一条 user 消息）
    end
    CO->>MR: read_memory_struct_index（顶层记忆索引，未实现时跳过）
    Note over CO: 配置/ego 生成系统消息执行一次 set（待定）；下次发送前 apply_pending_system 对比应用<br/>reset_context：SessionContext.reset()（archive_and_clear_cache → clear 内存）→ build_initial_context → Trigger::Forced 强制 flush<br/>compress_context（event 超长）：apply_pending_system → LLM 总结 → archive_and_clear_cache → rebuild（user(压缩指令)+assistant(总结)，从内存写回缓存）
```

## 七、对外交互边界

| 方向 | 组件 | 协议 | 说明 |
|------|------|------|------|
| agent → channel | kissbot-channel-client | WSS | agent 作为客户端连接消息通道；ChannelClient 持 `Weak<dyn Terminal>` 回调 coordinator（incoming_message / join_group / leave_group / user_removed / download_chunk / closed） |
| agent → memory-store | kissbot-memory-store | HTTP | MemoryStoreClient 推记录、MemoryReader 读记忆 |
| agent → memory-ego | kissbot-memory-ego | HTTP | resolve_agent_id（search-name）、load_ego_info（agent/individual/role 查询） |
| agent → LLM | 模型提供方 API | HTTP | ModelClient 调用（支持重试、工具调用、reasoning 回传） |
| agent ← 管理界面 | HttpServer | HTTP | 当前仅 config CRUD（操作 ConfigManager）；管理命令走 channel 消息（/ 开头） |

## 八、未接线 / 预留

- **station_client / station_router**：远程 station 连接与路由表，当前 coordinator 只用本地 `station_runtimes`（base_url 为空的本地 station 注册内置工具），远程调用走 REST 骨架、本轮未实现。
- **Terminal 的 join_group / leave_group / user_removed / download_chunk**：回调已实现（no-op / 未使用），业务逻辑预留。
- **http_server ↔ coordinator**：管理命令（/bind、/mode 等）目前由 channel 上行消息触发，HttpServer 未直接调用 coordinator。
