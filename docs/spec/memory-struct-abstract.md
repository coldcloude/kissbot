# kissbot-memory-struct-abstract 技术规格

> 说明摘要记忆实现模块的对外接口、通知协议、懒处理与分组、配置结构与存储格式。
> 摘要记忆的树节点称为 **Case（专案）**，与 agent 侧 role-event 的 **Event（事件）** 区分：case 是本组件对记忆切分/归并的产物，event 是 agent 侧的记忆模式标识（role-event 后缀）。文中的「事件模式 / 事件标识 / role-event」指 agent 侧 event，「case / 专案」指本组件的树节点。
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

## 术语与两级作用域

摘要记忆的作用域分两级：

| 概念 | 结构 | 决定什么 |
|------|------|----------|
| 角色 | `AgentRole { agent_id, role }` | **case 树**：同一角色的角色模式与事件模式**共用一棵树**、同一个 `ScopeManager` 句柄与同一个目录 |
| 记忆后缀 | `RoleEvent { role_name, event_id }`（项目级基础类型，memory 对外通信统一使用） | **输入与状态**：原始记录读取、水位、当前 case 指针、模型会话都按后缀区分 |
| 模型会话作用域 | `ModelScope { agent_id, role_event }` | 模型会话上下文（同一后缀的连续判定共享上下文） |

**记忆后缀的表示**：项目级基础类型 `RoleEvent { role_name, event_id }` 直接给出角色名与事件标识（`event_id` 缺省即角色模式），外部调用方不需要手工拼接。拼成文件/目录后缀（角色名，或 `角色名-事件标识`）是 memory 的内部约定，由 kissbot-memory 统一负责（`role_event_suffix`）。

磁盘上历史目录名仍按**第一个** `-` 识别：无 `-` 是角色名，含 `-` 时前半为角色名、后半为事件标识；角色名不允许含 `-`，事件标识可以含 `-`（如 UUID）。

**case 名**：

- 角色模式：`role-{模型给出的名称}`——名称由模型在判定「新建」的**同一次调用**里给出，只允许英文字母、数字与 `-`（如 `role-weather-chat`）；与该角色树里已有 case 重名时追加 `-2`、`-3`…。
- 事件模式：`event-{事件标识}`，是该角色的**固定顶层叶子 case**；事件标识部分同样只能含英文字母、数字与 `-`（不超过 64 字符），否则归档按失败处理（记忆后缀的 `角色名-事件标识` 里，事件标识可以含 `-`，如 UUID）。

两种模式走**不同的子树、互不并入**：角色模式的逐层判定候选只包含角色模式的 case，事件模式的 case 不参与判定，也不会被并入角色 case；名称前缀不同（`role-` / `event-`），两种模式天然不会重名，因此不需要跨模式判重。

## 对外 HTTP 接口

所有参数放在 JSON 请求体中，路径不含动态参数；响应统一使用 `ApiResponse`（`{ "success": bool, "data": …, "error": … }`）；认证走 `X-Api-Key`。

| 接口 | 用途 | 触发归档 |
|------|------|----------|
| `POST /abstract/locate` | 传入 agent、记忆后缀与新消息，返回命中 case 的标识 | 是（pending + 本批消息一次处理） |
| `POST /abstract/case/list` | 按记忆后缀与父 case 查询 case 节点 | 否（只读） |
| `POST /abstract/case/summary` | 取 case 摘要（节点：摘要文本、层级、记录数、起止时间） | 否（只读） |
| `POST /abstract/case/records` | 取 case 全部原文副本 | 否（只读） |
| `POST /abstract/case/records-after` | 取 case 中指定位置之后的原文副本（分页续读） | 否（只读） |
| `POST /abstract/case/records-tail` | 取 case 结尾若干条原文副本 | 否（只读） |
| `POST /abstract/tree/get` | 取某角色的 case 树与该记忆后缀各通道的当前 case 指针 | 否（只读） |
| `POST /abstract/rebuild` | `rebuild=false` 手动触发归档；`rebuild=true` 清空后重建 | 是 |

请求与响应类型作为模块专属类型定义在 kissbot-api（见 [system-design-api](../design/system-design-api.md)）。请求里的 `role_event` 字段为项目级基础类型 **`RoleEvent`**（`{ role_name, event_id }`，`event_id` 缺省即角色模式）；所有 memory 组件的对外通信统一使用它，文件/目录后缀由 memory 内部拼接。

### locate

定位一批消息涉及的 case：可带多条消息、可跨通道。执行只有一条路径，分三步：

1. **提取**：读一次该记忆后缀的共享记忆，取出各通道水位之后的待归档记录（pending），与本批消息一起**按通道分组**（pending 与本批消息都可以为空）；
2. **定位（只读）**：对每个通道调用归档入口 `archive(记录, 消息)` —— 角色模式下两者按 `msg_id` 去重合并成一个批次做第 1 轮分组，然后**逐组判定落点**：归入某个已有 case，或「需要新建在某个父 case 之下」；事件模式跳过分组与判定、固定归入 `event-{事件标识}`。判定不改 case 树、不落盘，**pending 是否为空走的是同一套判定**；
3. **存储 + 返回**：只有组里确实有待归档记录的，才真正落地——需要新建的此刻建出 case、记录追加为该 case 的原文副本、重算摘要、推进水位、更新当前 case 指针；其余组只把判定结果作为消息的 case 返回。最后把各通道的分组映射回请求下标合并成响应。

没有「先归档待处理记录、再定位消息」的两段式，也没有第二轮补齐循环：只要调用 locate，输入就是「pending + 消息」，两者都可为空。`messages` 为空时（`/abstract/rebuild` 的 rebuild=false 手动触发 / rebuild=true 重建）即原来的归档，不产出分组。

- 没有待处理标记且 `messages` 为空时不读盘、不调模型；带消息时总会读一次共享记忆以提取 pending。
- 判定阶段一律只读：不建 case、不改 case 树，也不落盘；新建 case、追加原文副本、重算摘要、推进水位都只发生在第 3 步（存储），且只针对组里有待归档记录的组。判定为「需要新建」的组无论有没有记录都对应一个 case：有记录时新建并落地，只定位时同样新建、只是记忆条数为 0。
- **事件模式**整体一个 case、**不分组、不调模型判定**（分组轮与逐层判定都不执行）：整个后缀固定归入 `event-{事件标识}` 顶层叶子 case；case 已存在则全部消息归入它，不存在时建出它（记忆条数为 0）。该 case 还需要摘要：由带记录的归档建出时按记录生成，由只带消息的定位建出时当场按消息内容生成（都在本次处理内完成，不留到后续）。

**去重与合并**：同一通道批次内，新消息与待归档记录按 `msg_id` 判为同一条（`msg_id` 为空的不参与去重、原样保留；通道已由批次切分，内容、时间、发送者、`is_self` 都不参与判定）。设重复的部分新来为 `A`、已存为 `A'`，不重复的部分新来为 `B`、已存为 `C`，则把 `A'`、`B`、`C` 一起分析：`A'` 与 `C` 照常落盘归档，`A` 与 `B` 只返回所属 case（新消息自身不落盘）。

```jsonc
// 请求
{
  "agent_id": "0",
  "role_event": { "role_name": "dev" },   // 记忆后缀：角色模式省略 event_id；事件模式为 { "role_name": "dev", "event_id": "e1" }
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
  "cases": [                                        // 消息涉及的全部 case（按首次出现顺序去重）
    { "case_name": "role-weather-chat", "is_current": true },
    { "case_name": "role-work-plan", "is_current": false }
  ],
  "groups": [                                       // 分组：哪几条消息属于哪个 case
    { "messages": [0, 1], "case_name": "role-weather-chat", "is_current": true },
    { "messages": [2], "case_name": "role-work-plan", "is_current": false },
    { "messages": [3], "case_name": "role-new-topic", "is_current": true }  // 新话题：本次建出该 case（记忆条数 0）
  ]
}
```

约定：

- 只返回 **case 名**（唯一标识）与分组，不返回摘要与原文副本；摘要与原文由调用方按 case 名另取（`case/summary`、`case/records*`，读取摘要不会重算摘要）。判定阶段只读；落地时每个组都会命中或建出 case，只有组里有待归档记录才会追加原文副本、重算摘要、推进水位。新话题建出的 case 记忆条数为 0，但**名称与摘要当场就有**（模型判定新建的同一次调用一并给出，不留到后续再调模型）；原文副本等这些消息写入记忆并被归档时才补上。
- 消息按**通道**分别处理：每条消息的通道三元组 `(messenger_id, self_user_id, group_id)` 决定用哪个通道的当前 case 指针（发送者 `user_id` 不参与）；归档与定位都按通道分开调用，去重在单通道批次内只比 `msg_id`。
- 分组轮的输出必须完整覆盖该通道的合并批次（待归档记录 + 去重后的新消息；下标升序、不重不漏），否则按失败处理。
- 组标记为当前 case 但该通道没有当前 case 时：含待归档记录的组按归档失败处理（内容无处可落）；只含新消息的组容忍该不一致，退回逐层判定。
- 其余组自顶层逐层判定，**没有层数上限**：每判定一次要么落在当层（并入叶子 case，或新建），要么下潜一层；case 树有限且无环，下潜必然终止。命中叶子 case 即该组的 case；下潜后当层都未命中则该组在该父 case 下新建。判定为「需要新建」时一律建出叶子 case——只有消息的组也建，只是那个 case 的记忆条数为 0（便于这些消息之后写入记忆时并入同一 case）。
- **事件模式**不分组、不做定位判定：整个后缀固定归入 `event-{事件标识}` 顶层叶子 case；case 还不存在时建出它（记忆条数为 0）。
- 角色树里还没有角色 case（空树）时候选为空，逐层判定照常**调用模型**（提示词写明当层没有 case）：模型必须给出新 case 的名称与摘要，随后每个组建出 case（记忆条数为 0，但已有名称与摘要）。
- `is_current` 表示该 case 是否是**该组消息所在通道**的当前 case：处理完本批后，该通道的当前 case 指针指向本批**最后一个组**落入的 case（含新建的空 case），`is_current` 据此判定；`cases` 里同一 case 被多个通道命中时，只要有一个通道认为它是当前 case 就为 `true`。
- `groups` 按各组首条消息在请求 `messages` 中的下标升序；消息在请求中的下标即分组里的 `messages`。重复的新消息映射到已存记录所在的 case。
- 事件模式的 case 是固定的顶层叶子；角色模式的判定看不到它，两种模式互不并入。

### case/list

按角色树的一层查询 case：`parent_id` 缺省或 `null` 表示查询顶层 case，给出某 case 名则查询其直接子 case。返回的是**整个角色树**在该层的节点——角色模式与事件模式的 case 都在（`mode` 字段区分），与请求的记忆后缀属于哪种模式无关。

```jsonc
// 请求
{ "agent_id": "0", "role_event": { "role_name": "dev" } }                       // 顶层
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "parent_id": "role-weather-chat" } // 某 case 的子层

// 响应 data
{ "cases": [ { "case_name": "role-weather-chat", "parent_id": null, "children": ["role-work-plan"], "mode": "role",
               "summary": "…", "record_count": 42,
               "start_time": "…", "end_time": "…", "created_at": "…", "updated_at": "…" } ] }
```

### case/summary、case/records、case/records-after、case/records-tail

四个接口都以 `{ agent_id, role, case_name }` 定位同一个 case，区别是取回的内容：

```jsonc
// /abstract/case/summary 的请求与响应 data
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "case_name": "role-weather-chat" }
{ "case": { "case_name": "role-weather-chat", "parent_id": null, "children": ["role-work-plan"], "mode": "role",
            "summary": "…", "record_count": 42,
            "start_time": "…", "end_time": "…", "created_at": "…", "updated_at": "…" } }

// /abstract/case/records 的请求与响应 data（全部原文副本，按 (time, sn) 升序）
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "case_name": "role-weather-chat" }
{ "records": [ { "record_type": "channel", "msg_id": "m-1001", "user_id": "u1",
                 "content": { "msg_type": "Text", "data": "…" },
                 "time": "2026-09-22 13:40:00", "sn": 127, "…": "…" } ] }

// /abstract/case/records-after 的请求与响应 data（指定位置之后的原文副本）
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "case_name": "role-weather-chat",
  "after": { "time": "2026-09-22 13:40:00", "sn": 127, "record_type": "channel" } }
{ "records": [ /* 该位置之后的原文副本 */ ] }

// /abstract/case/records-tail 的请求与响应 data（结尾若干条原文副本）
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "case_name": "role-weather-chat", "count": 10 }
{ "records": [ /* 结尾 count 条原文副本，升序 */ ] }
```

- 摘要即 case 节点本身（摘要文本 + 层级 + 记录数 + 起止时间），不含原文副本；子 case 摘要用 `case/list`（`parent_id`）或逐个 `case/summary` 取。
- 原文副本不截断：`case/records` 返回命中 case 的全部原文副本，类型由 `record_type` 标签给出（`channel` / `think` / `tool-call` / `tool-result`）。
- **游标**（`records-after.after`）：与某一条原文副本一一对应，调用方把上一条记录的 `time` / `sn` / `record_type` 原样回传即可连续分页。比较规则与归档批次末尾一致——同类型比较 `(time, sn)`，同一时刻的其他类型记录视为在其后（各类型文件的 `sn` 相互独立、跨类型不可比）。
- `records-tail` 的 `count` 为 0 时返回空数组；超过总条数时返回全部，均保持升序。
- case 不存在时四个接口（以及 `records-after` / `records-tail`）都以失败响应返回。

### tree/get

取某**角色**的 case 树与**本次请求那个记忆后缀**各通道的当前 case 指针：

```jsonc
// 请求
{ "agent_id": "0", "role_event": { "role_name": "dev", "event_id": "e1" } }

// 响应 data
{
  "current_cases": [                    // 只含本次请求那个记忆后缀的当前 case 指针
    { "channel": { "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" },
      "case_name": "event-e1" }
  ],
  "nodes": [ /* 整棵角色树的节点：角色模式与事件模式的 case 都在 */ ]
}
```

- `nodes` 是整棵角色树（两种模式的 case 都在）；`current_cases` 只含本次请求那个记忆后缀的指针——用角色后缀请求看不到事件后缀的指针，反之亦然。
- `current_cases` 为空时不出现该字段（序列化时省略）。

### rebuild

```jsonc
// 请求（rebuild 缺省为 false）
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "rebuild": false }   // 手动触发一次归档
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "rebuild": true }    // 清空本后缀自己的 case 后按共享记忆重建

// 响应 data
{ "archived": 3, "case_count": 1 }   // 本次归档的记录条数 + 该角色的 case 树节点总数
```

- `archived` 是本次处理（手动触发或重建）归档的记录条数；`case_count` 是该**角色** case 树的节点总数（两种模式的 case 都算）。
- `rebuild=false` 忽略待处理标记，把该后缀各通道水位之后的记录全部处理一次；`rebuild=true` 先清空该后缀自己的 case（角色模式为该角色的角色子树，事件模式只清本后缀的 `event-{事件标识}`）与该后缀的指针/水位，再重建。

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
  "payload": { "agent_id": "0", "role_event": { "role_name": "dev" }, "time": "2026-09-22 13:48:37",
               "is_self": 0,
               "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" } }
```

- `role_event` 为 `RoleEvent`：角色模式省略 `event_id`，事件模式给出 `角色-事件标识`。
- 通道消息的 payload 携带 `time`、`agent_id`、`role_event`、`is_self` 与通道三元组 `messenger_id` / `self_user_id` / `group_id`；其余类型只携带 `time`、`agent_id`、`role_event`，用于登记（通道未知时按该后缀全部通道登记）。
- 收到通知后**只登记**「哪个记忆后缀哪个通道有新数据」，不做任何计算（懒处理）：通道消息通知按通道登记，通知缺省通道字段时按该后缀全部通道登记。
- 共享记忆文件按 agent+后缀+日期共用，一次刷盘可能混合多个通道的记录：memory-store 按通道分组逐条通知，每个通道一条；订阅方不依赖「一条通知覆盖一批」。
- 思考/工具记录只有 key、写入侧无法确定通道，因此其通知不带通道字段——按该后缀全部通道登记，通道归属由记录读取时按 key 关联的占位通道记录确定。
- 通知服务由 memory-store 提供（监听地址见 [memory-store 技术规格](memory-store.md)），本组件作为客户端连接并在配置的 `notify_url` 上重连。
- 本组件按 `(记忆后缀, 通道)` 维护已处理水位；通知只是「有新数据」的信号，真正的读取与归档只发生在定位、手动触发与重建时（见懒处理）。

## 懒处理与分组

**懒处理**：收到通知只登记待处理标记；**只有两种入口**会统一计算——`locate`（pending + 本批消息）、`/abstract/rebuild`（rebuild=false 手动触发，messages 为空；rebuild=true 重建）。case 树/摘要/原文查询**只读**，不触发归档、也不清除待处理标记。计算时把该记忆后缀**每个通道**水位之后的记录各自成批处理一次，通道之间互不影响。

- 待处理标记是内存态（不落盘），按「**记忆后缀 + 通道**」登记：通道级（通道消息通知）或后缀级全部通道（思考/工具通知、后缀未知通道、首次加载、通知重连）。后缀级标记按该后缀全部通道处理。
- 没有待处理标记时，不带消息的查询不做任何读盘与模型调用；**定位请求**即使没有标记也会提取 pending 并定位本批消息（pending 可空）。
- 首次加载（进程启动恢复角色、新角色第一次被访问）一律按「记忆存储里出现过的后缀、全部通道待处理」登记；通知服务每次（重）连接后也把已加载角色的全部已出现后缀登记为后缀级，以覆盖断线期间漏掉的通知。
- 每个通道的**待归档记录**按 `max_batch_records` 分块处理，逐块落盘：一块失败时已完成的块保留进度，失败块回到处理前状态、保留待处理标记，留待下次查询重试。
- 定位与懒处理是同一个入口：读一次共享记忆提取各通道 pending，与本批消息一起按通道分组，逐通道执行「归档 + 定位」（见 [locate](#locate)）。同一通道批次内按 `msg_id` 去重，重复的新消息用已存版本分析一次，其余新消息随批次参与分组但不落盘；水位只由待归档记录推进。pending 与消息都可以为空（只归档 / 只定位）。
- 没有超时收尾，也没有「本人消息之后出现非本人消息」这类触发规则：批次边界完全由第 1 轮的分组轮决定。

**通道归属**（决定一条记录属于哪个通道的批次）：

| 记录 | 归属 |
|------|------|
| 通道记录 | 记录自身的 `(messenger_id, self_user_id, group_id)` |
| 思考 / 工具调用 / 工具结果记录 | 按记录自身的 `key` 找到同 key 的思考/工具**占位通道记录**，取其通道身份 |

找不到占位通道记录的思考/工具记录不属于任何通道，不进入任何通道的批次。

**第 1 轮（分组轮，仅角色模式）**：把该通道的当前 case 与水位之后的全部待处理消息一起交给模型，模型按 case 把消息分组，并标记哪些组属于当前 case。定位请求触发时，批次是「待归档记录 + 去重后的新消息」的合并结果。

- 标记为当前 case 的组**直接并入**当前 case（即使当前 case 是非叶子，也不再下潜）。
- 其余组**以组为单位**进入第 2 轮起的逐层判定（并入当层某 case、非叶子继续下潜，或判定为需要新建叶子 case）。
- 只含新消息的组走同一套判定：命中当前 case、归入某已有 case，或建出新的空 case（记忆条数 0）。
- 逐层判定的当层候选**只有角色模式的 case**（角色子树）；事件模式的固定 case 不参与判定。
- 分组必须完整覆盖合并批次的全部条目（下标升序、不重不漏）；不满足按失败处理，水位不动，下次查询重试——不能让任何一条记录被水位跳过。
- 处理完一批后，该通道的当前 case 指针指向本批**最后一个组落入的 case**（含本次新建、记忆条数为 0 的空 case）；水位推进到该批待归档记录中各类记录的最晚位置（只有定位的批次不动水位）。
- **事件模式**不分组、不做逐层判定：整个记忆后缀固定归入 `event-{事件标识}` 顶层叶子 case。

**重建（`/abstract/rebuild` 的 rebuild=true）只清空本次记忆后缀所属模式的那部分子树**：角色模式清空该角色树里全部 `mode == role` 的 case（角色子树），事件模式清空该后缀固定的那个事件 case（名称即事件标识）；同时复位该后缀的当前 case 指针与水位。另一种模式不受影响。

## 配置结构

```jsonc
{
  "memory_struct_abstract": {
    "listen_addr": "127.0.0.1",              // 本组件 HTTP 服务监听地址
    "listen_port": 8083,
    "notify_url": "ws://127.0.0.1:8182",     // memory-store 通知服务地址；缺省则不订阅通知
    "ws_reconnect_interval_secs": 5,         // 通知断线重连间隔
    "max_batch_records": 200,                // 单通道单次处理的条目上限（归档分块）
    "session_max_messages": 20,              // 每个记忆后缀的模型会话上下文保留消息条数上限
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

摘要记忆数据位于记忆根目录下各 agent 目录的 `memory-struct-abstract/`：

```
{根目录}/{agent-id}/memory-struct-abstract/
├── llm/                                  # 模型会话（会话标识按记忆后缀，session_id 即后缀的净化形式）
│   ├── context/{session-id}.jsonl
│   └── context-history/{session-id}-{时间戳}.jsonl
├── {角色名}/                              # 角色目录：case 树与原文副本
│   ├── tree.json                         # 一棵 case 树（角色模式与事件模式共用）
│   └── cases/{case 名}/                  # 原文副本，每类一个文件
│       ├── channel-records.jsonl
│       ├── think-records.jsonl
│       ├── tool-call-records.jsonl
│       └── tool-result-records.jsonl
└── {记忆后缀}/                            # 水位与当前 case 指针（后缀 = 角色名时就是上面的角色目录）
    ├── current.json                      # 各通道的当前 case 指针（字段 case_name）
    └── watermark.json                    # 各通道的已处理水位
```

- `{角色名}` 为 RoleEvent 里的角色名；`{记忆后缀}` 为完整 role-event 后缀：角色模式等于角色名（此时后缀目录即角色目录），事件模式为 `{角色名}-{事件标识}`。
- **case 树按角色共用**：`tree.json` 在角色目录下，同一角色的角色模式与事件模式看到的是同一棵树（查询 `tree/get` 时 `nodes` 相同）。
- **原始记录、水位、当前指针、模型会话按记忆后缀分开**：原文副本目录在角色目录下按 `case 名` 命名，水位/当前指针在各自的后缀目录下，模型会话按后缀的净化形式（session_id，非字母数字与 `-`/`_`/`.` 的字符替换为 `_`，空后缀用 `_default`）分开。
- 事件模式的 case 是角色树里固定的顶层叶子，case 名为 `event-{事件标识}`；角色模式新建 case 的 case 名为 `role-{模型给出的名称}`（名称只含英文字母、数字与 `-`，重名时追加 `-2`、`-3`…）。
- **新建 case 的名称与摘要都在新建当次给出**：名称非法（含中文、空格或其他字符，或超过 64 字符）或缺摘要都按模型失败处理（保留状态、下次重试）。
- 原文副本按类型分文件、按时间顺序追加，行内容与记忆存储的记录格式一致；不按日期分文件（日期切分只用于 memory-store 的原始记录文件，case 副本按时间顺序单文件累积）。
- 记忆存储侧的原始记录仍是 `{根目录}/{agent-id}/memory-store/{year}-{记忆后缀}/…`。

### tree.json

```jsonc
{
  "nodes": {
    "role-weather-chat": {
      "case_name": "role-weather-chat",
      "parent_id": null,               // 顶层为 null（序列化时该字段省略，反序列化缺省即 null）
      "children": ["role-work-plan"],
      "mode": "role",                  // "role" | "event"
      "summary": "…",
      "record_count": 42,
      "start_time": "2026-09-22 10:00:00",
      "end_time": "2026-09-22 13:48:37",
      "created_at": "2026-09-22 10:05:00",
      "updated_at": "2026-09-22 13:49:00"
    },
    "event-e1": {
      "case_name": "event-e1",
      "children": [],
      "mode": "event",
      "summary": "…",
      "record_count": 3,
      "start_time": "…", "end_time": "…", "created_at": "…", "updated_at": "…"
    }
  }
}
```

### current.json / watermark.json

```jsonc
// current.json：某记忆后缀各通道的当前 case 指针（无通道时文件不存在）
{
  "channels": [
    { "channel": { "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" }, "case_name": "role-work-plan" }
  ]
}

// watermark.json：某记忆后缀各通道、各记录类型的已处理位置（时间 + 序号）
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

- 两个文件都放在**记忆后缀目录**下，形状是「通道 → 状态」的列表；文件形状不匹配（例如旧版按类型平铺的水位文件）按状态文件损坏处理并报错，不做静默降级。
- 水位过滤按通道进行：某通道的待归档记录 = 共享记忆文件中归属该通道、且 `(time, sn)` 在该通道对应类型水位之后的记录。

## 模型交互

| 环节 | 输入 | 输出 |
|------|------|------|
| 第 1 轮（分组轮，仅角色模式） | 该通道当前 case 摘要 + 该通道水位之后的待归档记录（定位触发时再加上去重后的新消息，一个批次一次调用） | 消息按 case 分组的组列表（各组条目下标 + 是否属于当前 case） |
| 第 2 轮起（逐层判定，仅角色模式） | 一个组的内容 + **当层候选 case 摘要**（只有角色子树的 case；当层为空时提示词写明没有候选） | 并入当层某 case（非叶子继续下潜）/ 在本层新增叶子 case——新增时**同一次输出** case 名称（英文字母、数字、`-`）与摘要 |
| 摘要生成 | case 直接记录 + 子 case 摘要 | case 摘要（用于祖先级联与事件模式的新 case；本批新建的角色 case 已在逐层判定里拿到摘要，不再重复生成） |

- 事件模式**不调分组轮与逐层判定**，只对整个后缀固定的顶层叶子 case 生成摘要；该 case 由只带消息的定位建出时（还没有记录），当场按消息内容生成摘要，不留给后续调用。
- 第 1 轮与逐层判定的提示词文案同样使用 case 术语（「当前 case」「当层 case」）。

结构化输出约定（系统提示词要求只输出一个 JSON 对象；解析容忍 markdown 代码块与前后解释文字）：

```jsonc
// 第 1 轮（分组轮）：各组消息下标必须合起来覆盖全部下标（升序、不重不漏）
{ "groups": [ { "messages": [0, 1], "current": true },   // 属于当前 case → 直接并入当前 case
              { "messages": [2, 3], "current": false } ] } // 其他 case → 以组为单位进入逐层判定

// 第 2 轮起（逐层判定；只含新消息的组同样走这一步，只是不落盘）
{ "target": "role-new-topic" }    // 并入当层 case（target 为 case_name）；该 case 非叶子时继续下探其子层
{ "target": "new", "name": "weather-chat", "summary": "…" }  // 在本层新增叶子 case：同一次给出名称部分与摘要
                                    // 组件落位时拼成 case 名 "role-weather-chat"
                                    // name 只能含英文字母、数字与 "-"；校验不通过按模型失败处理
                                    // （只有消息的组也建，记忆条数为 0，但已有名称与摘要）

// 摘要生成
{ "summary": "…" }
```

- 新增 case 都是叶子 case：逐层判定下潜到非叶子 case 后，若当层（该 case 的子 case）未命中任何 case，则在**该 case 下**新建叶子 case；顶层未命中则在顶层新建。
- 第 1 轮把组标记为当前 case、但该通道没有当前 case 时：含待归档记录的组按模型失败处理（重新分组重试）；只含新消息的组容忍该不一致并退回逐层判定（定位只读、不落盘）。
- 父子层级由后续「建立 case 关联关系 / 合并为大 case」的定时程序产生（预留）。
- 各环节解析失败按失败处理并保留原状态（模型调用全部完成前不落盘、不推进水位），保留待处理标记，等待下次查询重试。
- 会话标识按**记忆后缀**编码（`ModelScope { agent_id, role_event }`），同一后缀的连续判定共享上下文（超过 `session_max_messages` 时归档并只保留最近的消息）；角色模式与事件模式各自一个会话。

## 错误类型

```rust
enum Error {
    NotifyError(String),        // 通知连接与帧解析
    StoreReadError(String),     // 原始记录读取
    ModelApiError(String),      // 模型调用（含映射自 kissbot-llm 的错误）
    StateError(String),         // case 树、水位、指针的读写与一致性
    IoError(std::io::Error),
    JsonError(serde_json::Error),
    FileError(kai_file::Error),
    InternalError(String),
}
pub type Result<T> = std::result::Result<T, Error>;
```

## 验证

- 单元测试：`cd kissbot-memory-struct-abstract && cargo test`（角色/记忆后缀解析与两种模式共用 case 树、懒处理与待处理标记按后缀隔离、分组落位与分层下探只取角色子树、分组覆盖校验、通道归属与通道隔离、摘要级联、水位推进、case 树读写与 tree.json 形状、case 定位匹配、定位即「pending + 消息」的一次处理（无待处理标记时也定位、每通道只归档一次、跨通道请求的消息下标、重复消息不重复送模型、判定与落地分开：每个组都命中或建出 case、只有有记录的组写原文副本与推进水位、新话题建出记忆条数为 0 的 case、去重按通道进行）、事件模式固定归入 `event-{事件标识}` 顶层叶子且不调分组/逐层判定、新建 case 当场拿到名称与摘要（名称字符集校验、重名追加 `-2`）、空树定位仍调一次逐层判定、重建只清本后缀自己的 case、通知帧解析、写入可见性、模型失败后的状态保留）。
- 集成测试：`test/tests/memory-struct-abstract-api.spec.ts` 用固定记忆数据目录验证「通知只登记 → 查询只读（不触发归档）→ 手动触发归档 → 多条/跨通道定位（pending + 消息一次处理）→ 定位新话题建出空 case → 摘要/原文分页查询 → 重建 → 事件模式固定归入 `event-{事件标识}` 顶层叶子且与角色模式共用一棵树、定位新话题建出的 case 当场带名称与摘要 → 通道隔离 → 思考记录按 key 归属」全流程，模型调用以桩实现替换（`test/workspace-template/config.json` 配置 `provider_type = "stub"`）。
