# kissbot-memory-struct-abstract 技术规格

> 说明摘要记忆实现模块的对外接口、通知协议、懒处理与分组、配置结构与存储格式。
> 组件设计与职责边界见 [kissbot-memory-struct-abstract 组件设计](../design/components-design/kissbot-memory-struct-abstract.md)，框架接口见 [kissbot-memory-struct 组件设计](../design/components-design/kissbot-memory-struct.md)，原始记录格式见 [memory-directory.md](memory-directory.md)。

## 位置与依赖

代码位置：`kissbot-memory-struct-abstract/`。

| 依赖 | 用途 |
|------|------|
| kissbot-api | `ApiResponse`、网络地址配置（memory-store 地址）、本组件的请求/响应类型、`ArcSwapHashMap` |
| kissbot-config | 公共配置读取（本组件配置段） |
| kissbot-security | API 认证（`X-Api-Key`） |
| kissbot-memory-struct | 记忆结构框架接口 |
| kissbot-memory | 记录类型、路径构造、目录管理与索引查询 |
| kissbot-llm | provider 调用与会话上下文 |
| kai-ws | 连接记忆存储的通知服务（WS 客户端） |
| kai-file | 原始记录的反向读取 |
| axum / tokio / serde / chrono / thiserror | HTTP 服务、异步运行时、序列化、时间、错误 |

不依赖 kissbot-agent / kissbot-channel / kissbot-mcp。

## 对外 HTTP 接口

所有参数放在 JSON 请求体中，路径不含动态参数；响应统一使用 `ApiResponse`；认证走 `X-Api-Key`。

| 接口 | 用途 | 触发归档 |
|------|------|----------|
| `POST /abstract/locate` | 传入 agent、角色与新消息，返回命中事件的标识 | 是（pending + 本批消息一次处理） |
| `POST /abstract/event/list` | 按作用域与父事件查询事件节点 | 否（只读） |
| `POST /abstract/event/summary` | 取事件摘要（节点：摘要文本、层级、记录数、起止时间） | 否（只读） |
| `POST /abstract/event/records` | 取事件全部原文副本 | 否（只读） |
| `POST /abstract/event/records-after` | 取事件中指定位置之后的原文副本（分页续读） | 否（只读） |
| `POST /abstract/event/records-tail` | 取事件结尾若干条原文副本 | 否（只读） |
| `POST /abstract/tree/get` | 取某作用域的事件树与各通道的当前事件指针 | 否（只读） |
| `POST /abstract/rebuild` | `rebuild=false` 手动触发归档；`rebuild=true` 清空后重建 | 是 |

请求与响应类型作为模块专属类型定义在 kissbot-api（见 [system-design-api](../design/system-design-api.md)）。

### locate

定位一批消息涉及的事件：可带多条消息、可跨通道。执行只有一条路径，分三步：

1. **提取**：读一次共享记忆，取出各通道水位之后的待归档记录（pending），与本批消息一起**按通道分组**（pending 与本批消息都可以为空）；
2. **定位（只读）**：对每个通道调用归档入口 `archive(记录, 消息)` —— 两者按 `msg_id` 去重合并成一个批次做第 1 轮分组，然后**逐组判定落点**：归入某个已有事件，或「需要新建在某个父事件之下」。判定不改事件树、不落盘，**pending 是否为空走的是同一套判定**；
3. **存储 + 返回**：只有组里确实有待归档记录的，才真正落地——需要新建的此刻建出事件、记录追加为该事件的原文副本、重算摘要、推进水位、更新当前事件指针；其余组只把判定结果作为消息的事件返回。最后把各通道的分组映射回请求下标合并成响应。

没有「先归档待处理记录、再定位消息」的两段式，也没有第二轮补齐循环：只要调用 locate，输入就是「pending + 消息」，两者都可为空。`messages` 为空时（`/abstract/rebuild` 的 rebuild=false 手动触发 / rebuild=true 重建）即原来的归档，不产出分组。

- 没有待处理标记且 `messages` 为空时不读盘、不调模型；带消息时总会读一次共享记忆以提取 pending。
- 只定位的组不落地：判定为「需要新建」时不建事件，用它所属的父事件代表该话题（顶层则视为新话题、不带 `event_id`）。
- 事件模式整体一个事件、不做分组：有记录落盘时确保事件存在；只有消息且事件不存在时不落地。

**去重与合并**：同一通道批次内，新消息与待归档记录按 `msg_id` 判为同一条（`msg_id` 为空的不参与去重、原样保留；通道已由批次切分，内容、时间、发送者、`is_self` 都不参与判定）。设重复的部分新来为 `A`、已存为 `A'`，不重复的部分新来为 `B`、已存为 `C`，则把 `A'`、`B`、`C` 一起分析：`A'` 与 `C` 照常落盘归档，`A` 与 `B` 只返回所属事件（新消息自身不落盘）。

```jsonc
// 请求
{
  "agent_id": "0",
  "role": "dev",                       // 记忆后缀；事件模式下形如 "dev-e1"
  "messages": [                        // 待定位消息：可多条、可跨通道，每条自带通道三元组
    { "msg_id": "m-1001",              // 消息标识：与同通道的已存记录去重只按 msg_id
      "time": "2026-09-22 13:48:37", "user_name": "alice", "is_self": 0,
      "messenger_id": "web", "self_user_id": "self1", "group_id": "g1",
      "content": "…" },               // 统一的公共消息格式
    { "msg_id": "m-1002",
      "time": "2026-09-22 13:49:02", "user_name": "alice", "is_self": 0,
      "messenger_id": "web", "self_user_id": "self2", "group_id": "g2",
      "content": "…" }
  ]
}

// 响应 data
{
  "events": [                                       // 消息涉及的全部事件（按首次出现顺序去重）
    { "event_id": "e12", "is_current": true },
    { "event_id": "e13", "is_current": false }
  ],
  "groups": [                                       // 分组：哪几条消息属于哪个事件
    { "messages": [0, 1], "event_id": "e12", "is_current": true },
    { "messages": [2], "event_id": "e13", "is_current": false },
    { "messages": [3], "event_id": "e14", "is_current": true }  // 新话题：本次建出该事件（记忆条数 0）
  ]
}
```

约定：

- 只返回**事件标识**与分组，不返回摘要与原文副本；摘要与原文由调用方按事件标识另取（`event/summary`、`event/records*`）。判定阶段只读；落地时每个组都会命中或建出事件，只有组里有待归档记录才会追加原文副本、重算摘要、推进水位。新话题建出的事件记忆条数为 0（摘要为空），其内容等这些消息写入记忆并被归档时才补上。
- 消息按**通道**分别处理：每条消息的通道三元组 `(messenger_id, self_user_id, group_id)` 决定用哪个通道的当前事件指针（发送者 `user_id` 不参与）；归档与定位都按通道分开调用，去重在单通道批次内只比 `msg_id`。
- 分组轮的输出必须完整覆盖该通道的合并批次（待归档记录 + 去重后的新消息；下标升序、不重不漏），否则按失败处理。
- 组标记为当前事件但该通道没有当前事件时：含待归档记录的组按归档失败处理（内容无处可落）；只含新消息的组容忍该不一致，退回逐层判定。
- 其余组自顶层逐层判定：命中叶子事件即该组的事件；命中非叶子后其子层都未命中则该组落在该父事件下。判定为「需要新建」时一律建出叶子事件——只有消息的组也建，只是那个事件的记忆条数为 0（便于这些消息之后写入记忆时并入同一事件）。
- 事件模式的作用域整体一个事件，不需要判定（不调模型）：全部消息都归入该事件；事件还不存在时建出它（记忆条数为 0）。
- 作用域还没有事件（空树）时没有事件可比对，不做逐层判定；第 1 轮分组照常，随后每个组建出事件（记忆条数为 0）。
- `is_current` 表示该事件是否是**该组消息所在通道**的当前事件：处理完本批后，该通道的当前事件指针指向本批**最后一个组**落入的事件（含新建的空事件），`is_current` 据此判定；`events` 里同一事件被多个通道命中时，只要有一个通道认为它是当前事件就为 `true`。
- `groups` 按各组首条消息在请求 `messages` 中的下标升序；消息在请求中的下标即分组里的 `messages`。重复的新消息映射到已存记录所在的事件。

### event/summary、event/records、event/records-after、event/records-tail

四个接口都以 `{ agent_id, role, event_id }` 定位同一个事件，区别是取回的内容：

```jsonc
// /abstract/event/summary 的请求与响应 data
{ "agent_id": "0", "role": "dev", "event_id": "e12" }
{ "event": { "event_id": "e12", "parent_id": null, "children": ["e13"], "mode": "role",
             "summary": "…", "record_count": 42,
             "start_time": "…", "end_time": "…", "created_at": "…", "updated_at": "…" } }

// /abstract/event/records 的请求与响应 data（全部原文副本，按 (time, sn) 升序）
{ "agent_id": "0", "role": "dev", "event_id": "e12" }
{ "records": [ { "record_type": "channel", "msg_id": "m-1001", "user_id": "u1",
                 "content": { "msg_type": "Text", "data": "…" },
                 "time": "2026-09-22 13:40:00", "sn": 127, "…": "…" } ] }

// /abstract/event/records-after 的请求与响应 data（指定位置之后的原文副本）
{ "agent_id": "0", "role": "dev", "event_id": "e12",
  "after": { "time": "2026-09-22 13:40:00", "sn": 127, "record_type": "channel" } }
{ "records": [ /* 该位置之后的原文副本 */ ] }

// /abstract/event/records-tail 的请求与响应 data（结尾若干条原文副本）
{ "agent_id": "0", "role": "dev", "event_id": "e12", "count": 10 }
{ "records": [ /* 结尾 count 条原文副本，升序 */ ] }
```

- 摘要即事件节点本身（摘要文本 + 层级 + 记录数 + 起止时间），不含原文副本；子事件摘要用 `event/list`（`parent_id`）或逐个 `event/summary` 取。
- 原文副本不截断：`event/records` 返回命中事件的全部原文副本，类型由 `record_type` 标签给出（`channel` / `think` / `tool-call` / `tool-result`）。
- **游标**（`records-after.after`）：与某一条原文副本一一对应，调用方把上一条记录的 `time` / `sn` / `record_type` 原样回传即可连续分页。比较规则与归档批次末尾一致——同类型比较 `(time, sn)`，同一时刻的其他类型记录视为在其后（各类型文件的 `sn` 相互独立、跨类型不可比）。
- `records-tail` 的 `count` 为 0 时返回空数组；超过总条数时返回全部，均保持升序。
- 事件不存在时四个接口（以及 `records-after` / `records-tail`）都以失败响应返回。

## 通知协议

记忆存储模块在写入新记录后，向已连接的本组件广播通知。通知不区分写入方式。

帧格式复用 kai-ws 信封（与消息通道同一套约定），记录类型由 `payload_type` 承载：

| payload_type | 记录类型 |
|--------------|----------|
| `0x3001_0001` | channel |
| `0x3001_0002` | think |
| `0x3001_0003` | tool-call |
| `0x3001_0004` | tool-result |

```jsonc
// WsMessage 信封
{ "sn": 0, "payload_type": 805371905, "status_code": 200,
  // payload：位置信息；通道消息另附发送者身份与通道三元组（平铺）
  "payload": { "agent_id": "0", "role": "dev", "time": "2026-09-22 13:48:37",
               "is_self": 0,
               "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" } }
```

- `role` 为记忆后缀：角色模式为角色名，事件模式为以第一个 `-` 分隔出的 `角色名-事件id`。
- 通道消息的 payload 携带 `time`、`agent_id`、`role`、`is_self` 与通道三元组 `messenger_id` / `self_user_id` / `group_id`；其余类型只携带 `time`、`agent_id`、`role`，用于登记（通道未知时按整个作用域登记）。
- 收到通知后**只登记**「哪个作用域哪个通道有新数据」，不做任何计算（懒处理）：通道消息通知按通道登记，通知缺省通道字段时按整个作用域登记。
- 共享记忆文件按 agent+role+日期共用，一次刷盘可能混合多个通道的记录：memory-store 按通道分组逐条通知，每个通道一条；订阅方不依赖「一条通知覆盖一批」。
- 思考/工具记录只有 key、写入侧无法确定通道，因此其通知不带通道字段——按整个作用域登记，通道归属由记录读取时按 key 关联的占位通道记录确定。
- 通知服务由 memory-store 提供（监听地址见 [memory-store 技术规格](memory-store.md)），本组件作为客户端连接并在配置的 `notify_url` 上重连。
- 本组件按 `(agent_id, role, 通道)` 维护已处理水位；通知只是「有新数据」的信号，真正的读取与归档只发生在定位、手动触发与重建时（见懒处理）。

## 懒处理与分组

**懒处理**：收到通知只登记待处理标记；**只有三种入口**会统一计算——`locate`（pending + 本批消息）、`/abstract/rebuild` 的 rebuild=false（手动触发，messages 为空）、rebuild=true（重建）。事件树/摘要/原文查询**只读**，不触发归档、也不清除待处理标记。计算时把该作用域**每个通道**水位之后的记录各自成批处理一次，通道之间互不影响。

- 待处理标记是内存态（不落盘）：通道级（通道消息通知）或作用域级（思考/工具通知、首次加载、通知重连）。作用域级标记按该作用域全部通道处理。
- 没有待处理标记时，不带消息的查询不做任何读盘与模型调用；**定位请求**即使没有标记也会提取 pending 并定位本批消息（pending 可空）。
- 首次加载（进程启动恢复作用域、新作用域第一次被访问）一律视为「全部通道待处理」；通知服务每次（重）连接后也把已加载作用域标记为作用域级，以覆盖断线期间漏掉的通知。
- 每个通道的**待归档记录**按 `max_batch_records` 分块处理，逐块落盘：一块失败时已完成的块保留进度，失败块回到处理前状态、保留待处理标记，留待下次查询重试。
- 定位与懒处理是同一个入口：读一次共享记忆提取各通道 pending，与本批消息一起按通道分组，逐通道执行「归档 + 定位」（见 [locate](#locate)）。同一通道批次内按 `msg_id` 去重，重复的新消息用已存版本分析一次，其余新消息随批次参与分组但不落盘；水位只由待归档记录推进。pending 与消息都可以为空（只归档 / 只定位）。
- 没有超时收尾，也没有「本人消息之后出现非本人消息」这类触发规则：批次边界完全由第 1 轮的分组轮决定。

**通道归属**（决定一条记录属于哪个通道的批次）：

| 记录 | 归属 |
|------|------|
| 通道记录 | 记录自身的 `(messenger_id, self_user_id, group_id)` |
| 思考 / 工具调用 / 工具结果记录 | 按记录自身的 `key` 找到同 key 的思考/工具**占位通道记录**，取其通道身份 |

找不到占位通道记录的思考/工具记录不属于任何通道，不进入任何通道的批次。

**第 1 轮（分组轮）**：把该通道的当前事件与水位之后的全部待处理消息一起交给模型，模型按事件把消息分组，并标记哪些组属于当前事件。定位请求触发时，批次是「待归档记录 + 去重后的新消息」的合并结果。

- 标记为当前事件的组**直接并入**当前事件（即使当前事件是非叶子，也不再下潜）。
- 其余组**以组为单位**进入第 2 轮起的逐层判定（并入当层某事件、非叶子继续下潜，或判定为需要新建叶子事件）。
- 只含新消息的组走同一套判定：命中当前事件、归入某已有事件，或建出新的空事件（记忆条数 0）。
- 分组必须完整覆盖合并批次的全部条目（下标升序、不重不漏）；不满足按失败处理，水位不动，下次查询重试——不能让任何一条记录被水位跳过。
- 处理完一批后，该通道的当前事件指针指向**最后一批有内容的记录落入的事件**；水位推进到该批待归档记录中各类记录的最晚位置。

## 配置结构

```jsonc
{
  "memory_struct_abstract": {
    "listen_addr": "127.0.0.1",              // 本组件 HTTP 服务监听地址
    "listen_port": 8083,
    "notify_url": "ws://127.0.0.1:8182",     // memory-store 通知服务地址；缺省则不订阅通知
    "ws_reconnect_interval_secs": 5,         // 通知断线重连间隔
    "max_batch_records": 200,                // 单通道单次处理的条目上限（归档分块）
    "session_max_messages": 20,              // 作用域会话上下文保留的消息条数上限
    "max_event_depth": 16,                   // 逐层判定最大层数
    "provider": {                            // provider 配置，结构见 kissbot-llm spec
      "provider_type": "openai",             // "openai" | "anthropic" | "stub"
      "base_url": "https://api.deepseek.com",
      "api_key": "sk-…"
    },
    "default_model_config": {                // max_tokens_usage 必填
      "max_tokens_usage": 131072, "timeout_secs": 60, "retry_count": 3
    },
    "model_configs": { "deepseek-flash": { "max_tokens_usage": 131072 } },
    "llm": {                                  // 生成参数须与 has_* 标志成对出现
      "provider": "deepseek", "model": "deepseek-flash",
      "max_tokens": 4096, "has_max_tokens": true
    }
  }
}
```

- 记忆根目录取自 kissbot-memory 的 `memory.root_dir`（`memory-directory.md` 的根目录），本组件不单独配置。
- memory-store 的 HTTP 地址取自 kissbot-api 的网络地址配置；通知地址为其 WS 端点（`memory.store.notify_listen_addr/notify_listen_port`，见 [memory-store 技术规格](memory-store.md)），在本组件以 `notify_url` 显式给出。
- provider 与模型配置复用 kissbot-llm 的配置结构（`provider_config` 键名同样接受 `provider`）；本组件只读取自己的配置段。
- `provider_type = "stub"` 时使用确定性桩模型（不调用外部模型），仅供集成测试与手动验证。

## 存储格式

事件数据位于记忆根目录下各 agent 目录的 `memory-struct-abstract/`，按记忆后缀分作用域：

```
{根目录}/{agent-id}/memory-struct-abstract/
├── llm/                             # 模型会话上下文（kissbot-llm 的会话缓存与历史归档）
│   ├── context/{role}.jsonl
│   └── context-history/{role}-{时间戳}.jsonl
└── {role}/
    ├── tree.json                    # 事件节点与层级（一棵，作用域级）
    ├── current.json                 # 各通道的当前事件指针
    ├── watermark.json               # 各通道的已处理水位
    └── events/{event-id}/
        ├── channel-records.jsonl     # 原文副本，每类一个文件（行内容与记忆存储一致）
        ├── think-records.jsonl
        ├── tool-call-records.jsonl
        └── tool-result-records.jsonl
```

- `{role}` 为记忆后缀；事件模式作用域只有一个顶层事件，`event-id` 取后缀中第一个 `-` 之后的部分。
- 角色模式新建事件的 `event-id` 由本组件分配（`e{N}`，作用域内递增）。
- 事件树按作用域组织（跨通道共享一棵）；当前事件指针与水位按通道分组。
- 原文副本按类型分文件、按时间顺序追加，行内容与记忆存储的记录格式一致；不按日期分文件（日期切分只用于 memory-store 的原始记录文件，事件副本按时间顺序单文件累积）。
- 模型会话上下文按作用域（记忆后缀）隔离，会话标识即记忆后缀。

### tree.json

```jsonc
{
  "nodes": {
    "e12": {
      "event_id": "e12",
      "parent_id": null,
      "children": ["e13"],
      "mode": "role",                  // "role" | "event"
      "summary": "…",
      "record_count": 42,
      "start_time": "2026-09-22 10:00:00",
      "end_time": "2026-09-22 13:48:37",
      "created_at": "2026-09-22 10:05:00",
      "updated_at": "2026-09-22 13:49:00"
    }
  }
}
```

### current.json / watermark.json

```jsonc
// current.json：各通道的当前事件指针（无通道时文件不存在）
{
  "channels": [
    { "channel": { "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" }, "event_id": "e13" }
  ]
}

// watermark.json：各通道、各记录类型的已处理位置（时间 + 序号）
{
  "channels": [
    {
      "channel": { "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" },
      "watermark": {
        "channel":     { "time": "2026-09-22 13:48:37", "sn": 128 },
        "think":       { "time": "2026-09-22 13:48:30", "sn": 64 },
        "tool-call":   { "time": "2026-09-22 13:48:31", "sn": 30 },
        "tool-result": { "time": "2026-09-22 13:48:32", "sn": 30 }
      }
    }
  ]
}
```

- 两个文件都是「通道 → 状态」的列表；文件形状不匹配（例如旧版按类型平铺的水位文件）按状态文件损坏处理并报错，不做静默降级。
- 水位过滤按通道进行：某通道的待归档记录 = 共享记忆文件中归属该通道、且 `(time, sn)` 在该通道对应类型水位之后的记录。

## 模型交互

| 环节 | 输入 | 输出 |
|------|------|------|
| 第 1 轮（分组轮） | 该通道当前事件摘要 + 该通道水位之后的待归档记录（定位触发时再加上去重后的新消息，一个批次一次调用） | 消息按事件分组的组列表（各组条目下标 + 是否属于当前事件） |
| 第 2 轮起（逐层判定） | 一个组的内容 + 当层全部事件摘要 | 并入当层某事件（非叶子继续下潜）/ 在本层新增叶子事件 |
| 摘要生成 | 事件直接记录 + 子事件摘要 | 事件摘要 |

结构化输出约定（系统提示词要求只输出一个 JSON 对象；解析容忍 markdown 代码块与前后解释文字）：

```jsonc
// 第 1 轮（分组轮）：各组消息下标必须合起来覆盖全部下标（升序、不重不漏）
{ "groups": [ { "messages": [0, 1], "current": true },   // 属于当前事件 → 直接并入当前事件
              { "messages": [2, 3], "current": false } ] } // 其他事件 → 以组为单位进入逐层判定

// 第 2 轮起（逐层判定；只含新消息的组同样走这一步，只是不落盘）
{ "target": "e3" }        // 并入当层事件；该事件非叶子时继续下探其子层
{ "target": "new" }       // 在本层新增叶子事件（只有消息的组也建，记忆条数为 0）

// 摘要生成
{ "summary": "…" }
```

- 新增事件都是叶子事件：逐层判定下潜到非叶子事件后，若当层（该事件的子事件）未命中任何事件，则在**该事件下**新建叶子事件；顶层未命中则在顶层新建。
- 第 1 轮把组标记为当前事件、但该通道没有当前事件时：含待归档记录的组按模型失败处理（重新分组重试）；只含新消息的组容忍该不一致并退回逐层判定（定位只读、不落盘）。
- 父子层级由后续「建立事件关联关系 / 合并为大事件」的定时程序产生（预留）。
- 各环节解析失败按失败处理并保留原状态（模型调用全部完成前不落盘、不推进水位），保留待处理标记，等待下次查询重试。
- 会话标识按作用域编码，同一作用域的连续判定共享上下文（超过 `session_max_messages` 时归档并只保留最近的消息）。

## 错误类型

```rust
enum Error {
    NotifyError(String),        // 通知连接与帧解析
    StoreReadError(String),     // 原始记录读取
    ModelApiError(String),      // 模型调用（含映射自 kissbot-llm 的错误）
    StateError(String),         // 事件树、水位、指针的读写与一致性
    IoError(std::io::Error),
    JsonError(serde_json::Error),
    FileError(kai_file::Error),
    InternalError(String),
}
pub type Result<T> = std::result::Result<T, Error>;
```

## 验证

- 单元测试：`cd kissbot-memory-struct-abstract && cargo test`（懒处理与待处理标记、分组落位与分层下探、分组覆盖校验、通道归属与通道隔离、摘要级联、水位推进、事件树读写、定位匹配、定位即「pending + 消息」的一次处理（无待处理标记时也定位、每通道只归档一次、跨通道请求的消息下标、重复消息不重复送模型、判定与落地分开：每个组都命中或建出事件、只有有记录的组写原文副本与推进水位、新话题建出记忆条数为 0 的事件、去重按通道进行）、通知帧解析、写入可见性、模型失败后的状态保留）。
- 集成测试：`test/tests/memory-struct-abstract-api.spec.ts` 用固定记忆数据目录验证「通知只登记 → 查询只读（不触发归档）→ 手动触发归档 → 多条/跨通道定位（pending + 消息一次处理）→ 定位新话题建出空事件 → 摘要/原文分页查询 → 重建 → 事件模式 → 通道隔离 → 思考记录按 key 归属」全流程，模型调用以桩实现替换（`test/workspace-template/config.json` 配置 `provider_type = "stub"`）。
