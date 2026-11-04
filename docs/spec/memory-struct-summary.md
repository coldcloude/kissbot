# kissbot-memory-struct-summary 技术规格

> 说明摘要记忆实现模块的对外接口、通知协议、懒处理与分组、配置结构与存储格式。
> 摘要记忆的树节点称为 **Case（专案）**，与 agent 侧 role-event 的 **Event（事件）** 区分：case 是本组件对记忆切分/归并的产物，event 是 agent 侧的记忆模式标识（role-event 后缀）。文中的「事件模式 / 事件标识 / role-event」指 agent 侧 event，「case / 专案」指本组件的树节点。
> 组件设计与职责边界见 [kissbot-memory-struct-summary 组件设计](../design/components-design/kissbot-memory-struct-summary.md)，框架接口见 [kissbot-memory-struct 组件设计](../design/components-design/kissbot-memory-struct.md)，原始记录格式见 [memory-directory.md](memory-directory.md)。

## 位置与依赖

代码位置：`kissbot-memory-struct-summary/`。

| 依赖 | 用途 |
|------|------|
| kissbot-api | `ApiResponse`、网络地址配置（memory-store 地址）、本组件的请求/响应类型 |
| kissbot-config | 公共配置读取（本组件配置段） |
| kissbot-security | API 认证（`X-Api-Key`） |
| kissbot-memory-struct | 记忆结构框架接口 |
| kissbot-memory | 记录类型、路径构造、目录管理与索引查询 |
| kissbot-llm | provider 调用与会话上下文 |
| kai-ws | 连接记忆存储的通知服务（WS 客户端） |
| kai-file | 原始记录的反向读取 |
| axum / tokio / serde / chrono / thiserror | HTTP 服务、异步运行时、序列化、时间、错误 |

不依赖 kissbot-agent / kissbot-channel / kissbot-mcp。

## 术语与两级归属

摘要记忆的归属分两级：

| 概念 | 结构 | 决定什么 |
|------|------|----------|
| 角色 | `AgentRole { agent_id, role }` | **case 森林**：一个 AgentRole 一片森林（角色树 + 每个事件一棵树），同一 AgentRole 的角色模式与事件模式**共用这片森林**、同一个 `AgentRoleHandle`（同一个目录、同一份 `AgentRoleState`）；`AgentRoleManager` 按 AgentRole 缓存句柄 |
| 记忆后缀 | `RoleEvent { role_name, event_id }`（项目级基础类型，memory 对外通信统一使用） | **输入与状态**：原始记录读取、水位、当前 case 指针都按后缀区分 |
| 事件标识 | `event_id: Option<ArcStr>`（组件内部的独立参数） | **模式与 case 查找**：缺省为角色模式、给出事件标识为事件模式；事件模式直接定位森林里的事件根 `event-{事件标识}`（叶子直接归入、非叶子下潜），模型会话按 `(AgentRole, event_id)` 隔离 |

**记忆后缀的表示**：项目级基础类型 `RoleEvent { role_name, event_id }` 直接给出角色名与事件标识（`event_id` 缺省即角色模式），外部调用方不需要手工拼接。拼成文件/目录后缀（角色名，或 `角色名-事件标识`）是 memory 的内部约定，由 kissbot-memory 统一负责（`role_event_suffix`）。

磁盘上历史目录名仍按**第一个** `-` 识别：无 `-` 是角色名，含 `-` 时前半为角色名、后半为事件标识；角色名不允许含 `-`，事件标识可以含 `-`（如 UUID）。

**case 名**：

- 角色模式：`role-{模型给出的名称}`——名称由模型在判定「新建」的**同一次调用**里给出，只允许英文字母、数字与 `-`（如 `role-weather-chat`）；与该 AgentRole 树里已有 case 重名时追加 `-2`、`-3`…。
- 事件模式：`event-{事件标识}`，是该 AgentRole 森林里**事件树的根**，由组件内部的 `event_id` 参数直接定位（不存在则建出）；通常是叶子，非叶子时继续下潜，新子 case 为 `event-{事件标识}-{模型给出的名称}`。case 名是命名约定与查找键，节点不记录模式/事件字段。事件标识同样只能含英文字母、数字与 `-`（不超过 64 字符），否则归档按失败处理（记忆后缀的 `角色名-事件标识` 里，事件标识可以含 `-`，如 UUID）。
- 隐藏角色根：`__role_root__`，只用于把角色 case 组织成一棵树、便于遍历，不是业务 case；不出现在查询结果里，`count`/重建清空都不算它。

两种模式走**不同的子树、互不并入**：一个 AgentRole 的 case 组织成森林——角色模式一棵树（隐藏的固定根 `__role_root__`，其子节点是 `role-` 前缀的角色 case），每个事件一棵树（根是可见的 `event-{事件标识}`），靠名称前缀区分。角色模式的逐层判定当层候选只取 `role-` 前缀的 case，事件模式的 case 不参与判定、也不会被并入角色 case；两种前缀天然不会重名，因此不需要跨模式判重。角色根只用于遍历与隐藏的层级组织，不是业务 case，不出现在查询结果里。

## 记录 meta 与直接定位

通道记录可以带可选的 `meta`（kissbot-api 的 `MemoryMeta`）。摘要记忆的使用方（agent 侧摘要记忆 pipeline）在写入记录时填入 `meta.memory_summary.case_name`：

```jsonc
{ "msg_id": "m-1001", "…": "…",
  "meta": { "memory_summary": { "case_name": "role-weather-chat" } } }
```

- 归档读到带该信息的记录时**直接按 case 名落位**，不再走分组轮与逐层判定；该 case 不存在（例如重建后树里还没有该节点）时**按 meta 名建出顶层 case**，记录不丢。
- 没有 meta（或 meta 的 case 名不可用）的待归档记录**直接跳过**：不定位、不落 case，但水位照常推进，避免下次重复读取。
- **原始记录只能写入叶子 case**：meta 指向的 case 已存在但不是叶子（复合 case 或角色根）时按失败处理，不写记录、不推进水位；定位结果同样必须是叶子 case，落到非叶子时按失败处理。复合 case 的摘要只由子 case 摘要组成，不再包含自身记录。
- 因此归档只对新消息做定位；现有记忆（pending）只有带 meta 才会进入 case 树，没有 meta 的记录不会被重新分组。
- 直接落位只影响定位方式：记录照常追加为 case 原文副本、重算摘要（必要时级联祖先）、推进水位；该通道的当前 case 指针按本批时间最晚的条目更新。
- 该字段由 locate 的调用方在拿到各条消息的 case 之后填入，调用方在 locate 之后才写入记忆，因此 locate 不需要与已存记录去重。
- **归档时序**：locate 只返回 case 名，不把新消息写成 case 原文副本；调用方在 locate 之后写入记忆并带 meta，摘要记忆归档时才按 meta 落盘（带 meta 的直接落位、无 meta 跳过）。
- 思考/工具内容内嵌在通道记录的 `content` 里，随所属通道记录一起归属到 case，不单独读取。

## 对外 HTTP 接口

所有参数放在 JSON 请求体中，路径不含动态参数；响应统一使用 `ApiResponse`（`{ "success": bool, "data": …, "error": … }`）；认证走 `X-Api-Key`。

| 接口 | 用途 | 触发归档 |
|------|------|----------|
| `POST /summary/locate` | 传入 agent、记忆后缀与新消息，返回命中 case 的标识 | 是（pending + 本批消息一次处理） |
| `POST /summary/case/list` | 按记忆后缀与父 case 查询 case 节点 | 否（只读） |
| `POST /summary/case/summary` | 取 case 摘要（节点：摘要文本、层级、记录数、起止时间） | 否（只读） |
| `POST /summary/case/records` | 取 case 全部原文副本 | 否（只读） |
| `POST /summary/case/records-after` | 取 case 中指定位置之后的原文副本（分页续读） | 否（只读） |
| `POST /summary/case/records-tail` | 取 case 结尾若干条原文副本 | 否（只读） |
| `POST /summary/tree/get` | 取某 AgentRole 的 case 树与该记忆后缀各通道的当前 case 指针 | 否（只读） |
| `POST /summary/rebuild` | `rebuild=false` 手动触发归档；`rebuild=true` 清空后重建 | 是 |

请求与响应类型作为模块专属类型定义在 kissbot-api（见 [system-design-api](../design/system-design-api.md)）。请求里的 `role_event` 字段为项目级基础类型 **`RoleEvent`**（`{ role_name, event_id }`，`event_id` 缺省即角色模式）；所有 memory 组件的对外通信统一使用它，文件/目录后缀由 memory 内部拼接。

### locate

定位一批消息涉及的 case：一次只针对**一个通道**（通道由请求给出），可带多条消息。执行只有一条路径，分三步：

1. **提取**：读一次该记忆后缀的共享记忆，取出各通道水位之后的待归档记录（pending）；请求通道的 pending 与本批消息一起处理（pending 与本批消息都可以为空）；
2. **定位（只读）**：
   - 带 `meta.memory_summary.case_name` 的 pending 记录直接落到该 case（不调模型；case 不存在时留到存储阶段按 meta 名建出）；
   - 没有 meta（或 meta 的 case 名不可用）的 pending 记录直接跳过（不调模型、不落 case，水位照常推进）；
   - 只有本批新消息按时间升序合并成一个批次（不按 `msg_id` 去重），角色模式下做第 1 轮分组并**逐组判定落点**：归入某个已有 case，或「需要新建在某个父 case 之下」；事件模式跳过分组轮，事件根是叶子时直接归入、非叶子时以它为根逐层下潜。判定不改 case 森林、不落盘；
3. **存储 + 返回**：带 meta 的 pending 记录按 meta 落位（缺失 case 建出、追加原文副本、重算摘要、推进水位、更新当前 case 指针）；新消息按组落地——需要新建的此刻建出 case、消息只返回所属 case、不落盘。最后把同一 case 的多个新消息组按 `msg_id` 取并合并成一组返回。

只要调用 locate，输入就是「pending + 消息」：pending 只有带 meta 才落 case，没有 meta 跳过；只有新消息参与分组与逐层判定。没有「先归档待处理记录、再定位消息」的两段式，也没有第二轮补齐循环。`messages` 为空时（`/summary/rebuild` 的 rebuild=false 手动触发 / rebuild=true 重建）即按 meta 归档，无 meta 跳过，不产出分组。

- 没有待处理标记且 `messages` 为空时不读盘、不调模型；带消息时总会读一次共享记忆以提取 pending。
- 判定阶段一律只读：不建 case、不改 case 树，也不落盘；建 case、追加原文副本、重算摘要、推进水位都只发生在第 3 步（存储）。每个新消息组都会命中或建出 case（只定位的组记忆条数为 0）；带 meta 的 pending 记录按 meta 直接落位，meta 指向的 case 不存在时也建出。
- **事件模式**整体归入事件根、**不调分组轮**：`event-{事件标识}` 是叶子时全部新消息直接归入它；非叶子时以它为根调用逐层判定，命中叶子即归入、未命中则新建 `event-{事件标识}-{名称}`。带 meta 的 pending 记录只能落到叶子，事件根非叶子时指向它会按失败处理。事件根不存在时建出叶子（记忆条数为 0），摘要按消息内容生成；由带记录的归档建出/补全时按记录生成。

**记录与消息的合并**：同一通道批次内，只有新消息进入模型分析；pending 记录带 meta 时直接按 case 落位、没有 meta 时跳过，都不参与分组。批次的单位是**通道消息**：一条消息一条原文，内容里的思考/工具记录按其在 `Multi` 中的顺序用标签内嵌在原文里（见 [模型交互](#模型交互)），不单独参与分组。新消息在调用 locate 时还没有写入记忆，因此不需要与已存记录去重（同一 `msg_id` 会各占一个条目，分组标识加 `#序号` 后缀区分，定位结果仍按原 `msg_id` 返回）；pending 记录照常按 meta 落盘归档，新消息只返回所属 case、不落盘。

```jsonc
// 请求
{
  "agent_id": "0",
  "role_event": { "role_name": "dev" },   // 记忆后缀：角色模式省略 event_id；事件模式为 { "role_name": "dev", "event_id": "e1" }
  "channel": {                            // 本批消息所属通道：一次定位只针对一个通道
    "messenger_id": "web", "self_user_id": "self1", "group_id": "g1"
  },
  "messages": [                           // 待定位消息：可多条，同属上面的通道
    { "msg_id": "m-1001",                 // 消息标识：分组按它标识消息，本批内必须非空且唯一
      "time": "2026-09-22 13:48:37", "user_id": "u-1", "user_name": "alice", "is_self": 0,
      "content": "…" },                   // 统一的公共消息格式
    { "msg_id": "m-1002",
      "time": "2026-09-22 13:49:02", "user_id": "u-1", "user_name": "alice", "is_self": 0,
      "content": "…" }
  ]
}

// 响应 data
{
  "groups": [                                       // 分组：哪几条消息属于哪个 case（同一 case 只有一组）
    { "messages": ["m-1001", "m-1002"], "case_name": "role-weather-chat" },
    { "messages": ["m-1003"], "case_name": "role-work-plan" },
    { "messages": ["m-1004"], "case_name": "role-new-topic" }  // 新话题：本次建出该 case（记忆条数 0）
  ]
}
```

约定：

- 只返回 **case 名**（唯一标识）与分组（分组只含本批新消息），不返回摘要与原文副本；摘要与原文由调用方按 case 名另取（`case/summary`、`case/records*`，读取摘要不会重算摘要）。判定阶段只读；落地时每个新消息组都会命中或建出 case，带 meta 的 pending 记录另行按 meta 落位并追加原文副本、重算摘要、推进水位。新话题建出的 case 记忆条数为 0，但**名称与摘要当场就有**（模型判定新建的同一次调用一并给出，不留到后续再调模型）；原文副本等这些消息写入记忆并被归档时才补上。
- 消息按**通道**分别处理：请求里的通道三元组 `(messenger_id, self_user_id, group_id)` 决定用哪个通道的当前 case 指针（发送者 `user_id` 不参与）；一次请求只处理**请求通道**（该通道的 pending 与本批消息），跨通道请分多次调用。该后缀其他通道的待归档记录保留，等该通道的定位或手动触发 / 重建再处理，不会产出本请求的分组。
- 分组轮的输出必须完整覆盖该通道的**本批新消息**（不含 pending 记录；按 `msg_id` 不重不漏），否则按失败处理；组间与组内都不要求连续或有序（同一话题的消息可以交错）。
- 组标记为当前 case 但该通道没有当前 case 时：只含新消息的组容忍该不一致，退回逐层判定。
- 其余组自顶层逐层判定，**没有层数上限**：每判定一次要么落在当层（并入叶子 case，或新建），要么下潜一层；case 森林有限且无环，下潜必然终止。命中叶子 case 即该组的 case；下潜后当层都未命中则该组在该父 case 下新建。判定为「需要新建」时一律建出叶子 case——只有消息的组也建，只是那个 case 的记忆条数为 0（便于这些消息之后写入记忆时并入同一 case）。
- **事件模式**不调分组轮：事件根是叶子时整个后缀直接归入它；事件根是复合 case 时以它为根逐层下潜（新子 case 为 `event-{事件标识}-{名称}`），未命中叶子即新建；事件根还不存在时建出叶子（记忆条数为 0）。
- 角色树里还没有角色 case（空树）时候选为空，逐层判定照常**调用模型**（提示词写明当层没有 case）：模型必须给出新 case 的名称与摘要，随后每个组建出 case（记忆条数为 0，但已有名称与摘要）。
- 请求里的每条消息都必须带**非空且唯一**的 `msg_id`（分组结果用它标识消息），否则按失败处理；定位不按 `msg_id` 与已存记录去重。
- 分组里的 `messages` 是消息的 `msg_id`，**同一 case 的多个组在返回前合并成一组**（`messages` 取并集），因此同一 case 在响应里只出现一次；`groups` 之间、`messages` 内部都不构成顺序约定，调用方按 `msg_id` 匹配。
- `messages` 在内存里是集合（`HashSet<ArcStr>`，合并同 case 组时取并、调用方按 `contains` 匹配），按数组序列化。
- 事件模式的根是 `event-{事件标识}`（森林顶层，可见）；角色模式的判定看不到它，两种模式互不并入。

### case/list

按角色森林的一层查询 case：`parent_id` 缺省或 `null` 表示查询**森林顶层**（`role-` 角色 case 与事件根；隐藏的角色根 `__role_root__` 不出现），给出某 case 名则查询其直接子 case。返回的是**整个 AgentRole 森林**在该层的节点——角色模式与事件模式的 case 都在，与请求的记忆后缀属于哪种模式无关。

```jsonc
// 请求
{ "agent_id": "0", "role_event": { "role_name": "dev" } }                       // 顶层
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "parent_id": "role-weather-chat" } // 某 case 的子层

// 响应 data
{ "cases": [ { "case_name": "role-weather-chat", "parent_id": null, "children": ["role-work-plan"],
               "summary": "…", "record_count": 42,
               "start_time": "…", "end_time": "…", "created_at": "…", "updated_at": "…" } ] }
```

### case/summary、case/records、case/records-after、case/records-tail

四个接口都以 `{ agent_id, role, case_name }` 定位同一个 case，区别是取回的内容：

```jsonc
// /summary/case/summary 的请求与响应 data
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "case_name": "role-weather-chat" }
{ "case": { "case_name": "role-weather-chat", "parent_id": null, "children": ["role-work-plan"],
            "summary": "…", "record_count": 42,
            "start_time": "…", "end_time": "…", "created_at": "…", "updated_at": "…" } }

// /summary/case/records 的请求与响应 data（全部原文副本，按 (time, sn) 升序）
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "case_name": "role-weather-chat" }
{ "records": [ { "msg_id": "m-1001", "user_id": "u1",
                 "content": { "msg_type": "Text", "data": "…" },
                 "time": "2026-09-22 13:40:00", "sn": 127, "…": "…" } ] }

// /summary/case/records-after 的请求与响应 data（指定位置之后的原文副本）
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "case_name": "role-weather-chat",
  "after": { "time": "2026-09-22 13:40:00", "sn": 127 } }
{ "records": [ /* 该位置之后的原文副本 */ ] }

// /summary/case/records-tail 的请求与响应 data（结尾若干条原文副本）
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "case_name": "role-weather-chat", "count": 10 }
{ "records": [ /* 结尾 count 条原文副本，升序 */ ] }
```

- 摘要即 case 节点本身（摘要文本 + 层级 + 记录数 + 起止时间），不含原文副本；子 case 摘要用 `case/list`（`parent_id`）或逐个 `case/summary` 取。
- 原文副本不截断：`case/records` 返回命中 case 的全部通道记录；思考/工具内容内嵌在记录的 `content` 里。
- 原文副本只属于叶子 case：复合 case（含隐藏角色根）没有原文副本文件，`case/records*` 返回空，不递归子 case；记录只能写入叶子，指向非叶子按失败处理。
- **游标**（`records-after.after`）：与某一条通道记录一一对应，调用方把上一条记录的 `time` / `sn` 原样回传即可连续分页；按 `(time, sn)` 比较。
- `records-tail` 的 `count` 为 0 时返回空数组；超过总条数时返回全部，均保持升序。
- case 不存在时四个接口（以及 `records-after` / `records-tail`）都以失败响应返回；隐藏的角色根不作为业务 case，查询它也按不存在处理。

### tree/get

取某 **AgentRole** 的 case 树与**本次请求那个记忆后缀**各通道的当前 case 指针：

```jsonc
// 请求
{ "agent_id": "0", "role_event": { "role_name": "dev", "event_id": "e1" } }

// 响应 data
{
  "current_cases": [                    // 只含本次请求那个记忆后缀的当前 case 指针
    { "channel": { "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" },
      "case_name": "event-e1" }
  ],
  "nodes": [ /* 整棵 AgentRole 树的节点：角色模式与事件模式的 case 都在 */ ]
}
```

- `nodes` 是整棵 AgentRole **森林**（角色 case 与事件 case 都在；隐藏的角色根不出现，角色顶层 case 的 `parent_id` 序列化为 null）；`current_cases` 只含本次请求那个记忆后缀的指针——用角色后缀请求看不到事件后缀的指针，反之亦然。
- `current_cases` 为空时不出现该字段（序列化时省略）。

### rebuild

```jsonc
// 请求（rebuild 缺省为 false）
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "rebuild": false }   // 手动触发一次归档
{ "agent_id": "0", "role_event": { "role_name": "dev" }, "rebuild": true }    // 清空本后缀自己的 case 后按共享记忆重建

// 响应 data
{ "archived": 3, "case_count": 1 }   // 本次归档的记录条数 + 该 AgentRole 的业务 case 节点总数（不含角色根）
```

- `archived` 是本次处理（手动触发或重建）归档的记录条数；`case_count` 是该 **AgentRole** 的业务 case 节点总数（角色模式与事件模式的 case 都算，不含隐藏的角色根）。
- `rebuild=false` 忽略待处理标记，把该后缀各通道水位之后的记录全部处理一次（带 meta 的按 case 落位，无 meta 的跳过并推进水位）；`rebuild=true` 先清空该后缀自己的 case（角色模式清空该 AgentRole 里全部 `role-` 前缀的角色 case，保留隐藏的角色根；事件模式清本后缀 `event-{事件标识}` 及其整棵事件子树）与该后缀的指针/水位，再按记录里的 meta 重建（无 meta 跳过）。

## 通知协议

记忆存储模块在写入新记录后，向已连接的本组件广播通知。通知不区分写入方式。

帧格式复用 kai-ws 信封（与消息通道同一套约定），`payload_type` 只有通道记录一种：

| payload_type | 记录类型 |
|--------------|----------|
| `0x3001_0001` | channel |

```jsonc
// WsMessage 信封
{ "sn": 0, "payload_type": 805371905, "status_code": 200,
  // payload：位置信息；通道消息另附发送者身份与通道三元组（平铺）
  "payload": { "agent_id": "0", "role_event": { "role_name": "dev" }, "time": "2026-09-22 13:48:37",
               "is_self": 0,
               "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" } }
```

- `role_event` 为 `RoleEvent`：角色模式省略 `event_id`，事件模式给出 `角色-事件标识`。
- 每条通知都携带 `time`、`agent_id`、`role_event`、`is_self` 与通道三元组 `messenger_id` / `self_user_id` / `group_id`。
- 收到通知后**只登记**「哪个记忆后缀哪个通道有新数据」，不做任何计算（懒处理）：按通道登记。
- 共享记忆文件按 agent+后缀+日期共用，一次刷盘可能混合多个通道的记录：memory-store 按通道分组逐条通知，每个通道一条；订阅方不依赖「一条通知覆盖一批」。
- 通知服务由 memory-store 提供（监听地址见 [memory-store 技术规格](memory-store.md)），本组件作为客户端连接并在配置的 `notify_url` 上重连。
- 本组件按 `(记忆后缀, 通道)` 维护已处理水位；通知只是「有新数据」的信号，真正的读取与归档只发生在定位、手动触发与重建时（见懒处理）。

## 懒处理与分组

**懒处理**：收到通知只登记待处理标记；**只有两种入口**会统一计算——`locate`（pending + 本批消息）、`/summary/rebuild`（rebuild=false 手动触发，messages 为空；rebuild=true 重建）。case 森林/摘要/原文查询**只读**，不触发归档、也不清除待处理标记。计算时把该记忆后缀**每个通道**水位之后的记录各自成批处理一次，通道之间互不影响。

- 待处理标记是内存态（不落盘），按「**记忆后缀 + 通道**」登记：通道级（通道消息通知）或后缀级全部通道（首次加载、通知重连）。后缀级标记按该后缀全部通道处理。
- 没有待处理标记时，不带消息的查询不做任何读盘与模型调用；**定位请求**即使没有标记也会提取 pending 并定位本批消息（pending 可空）。
- 首次加载（进程启动恢复角色、新角色第一次被访问）一律按「记忆存储里出现过的后缀、全部通道待处理」登记；通知服务每次（重）连接后也把已加载角色的全部已出现后缀登记为后缀级，以覆盖断线期间漏掉的通知。
- 每个通道的**待归档消息**按 `max_batch_records` 分块处理（一条消息为一格，思考/工具内容随所属消息），逐块落盘：一块失败时已完成的块保留进度，失败块回到处理前状态、保留待处理标记，留待下次查询重试。
- 定位与懒处理是同一个入口：定位读一次共享记忆，但**只处理请求通道**（该通道水位之后的 pending + 本批消息，见 [locate](#locate)）；手动触发与重建才逐通道处理全部通道。带 meta 的 pending 记录直接落到指定 case（缺失则建出）；没有 meta 的 pending 直接跳过但推进水位；只有新消息参与分组与逐层判定，新消息只定位、不落盘，水位只由待归档记录推进。pending 与消息都可以为空（只归档 / 只定位）。
- 没有超时收尾，也没有「本人消息之后出现非本人消息」这类触发规则：批次边界完全由第 1 轮的分组轮决定。

**通道归属**（决定一条记录属于哪个通道的批次）：

| 记录 | 归属 |
|------|------|
| 通道记录 | 记录自身的 `(messenger_id, self_user_id, group_id)`；思考/工具内容内嵌在它的 `content` 里 |

记忆存储只有通道记录一类，读取时直接按通道记录自身的身份归属；不需要再读取其他记录文件。

**第 1 轮（分组轮，仅角色模式）**：把该通道的当前 case 与本批新消息一起交给模型，模型按 case 把消息分组，并标记哪些组属于当前 case。待归档记录不参与分组：带 meta 的直接落 case，没有 meta 的跳过。批次的单位是通道消息：一条消息（`Multi` 内容也一样）合成一条原文，内容里的思考/工具记录按其在 `Multi` 中的顺序用标签内嵌在原文里，不单独参与分组。

- 标记为当前 case 的组**直接并入**当前 case；当前 case 必须是叶子，指向非叶子时按失败处理（记录只能写叶子）。
- 其余组**以组为单位**进入第 2 轮起的逐层判定（并入当层某 case、非叶子继续下潜，或判定为需要新建叶子 case）；组按组内**首条记忆的时间顺序**依次处理，而不是按模型输出顺序。
- 只含新消息的组走同一套判定：命中当前 case、归入某已有 case，或建出新的空 case（记忆条数 0）。
- 逐层判定的当层候选**只有 `role-` 前缀的角色 case**（两种 case 共用一片森林、靠名称前缀区分）；事件模式的 case 不参与判定。
- 分组必须按 `msg_id` 完整覆盖本批新消息（每条恰好一次、不重不漏）；不要求连续或有序——同一话题的消息可以交错，组内顺序也不构成约定。不满足按失败处理，水位不动，下次查询重试——不能让任何一条记录被水位跳过。
- 处理完一批后，该通道的当前 case 指针指向本批**时间最晚的条目落入的 case**（含带 meta 的记录、本次新建记忆条数为 0 的空 case；与组在模型输出里的顺序无关）；水位推进到该批待归档通道记录的最晚位置（只有定位的批次不动水位）。
- **事件模式**不调分组轮：由 `event_id` 直接定位事件根；根是叶子时直接归入，根是复合 case 时以它为根逐层下潜，新子 case 为 `event-{事件标识}-{名称}`。

**重建（`/summary/rebuild` 的 rebuild=true）只清空本次记忆后缀自己的那部分 case**：角色模式清空该 AgentRole 树里全部 `role-` 前缀的角色 case，事件模式只清本后缀固定的 `event-{事件标识}` 叶子；同时复位该后缀的当前 case 指针与水位。重建按记录里的 meta 重建 case（同名 case 直接建出、按记录生成摘要），没有 meta 的记录跳过；另一种模式不受影响。

## 配置结构

```jsonc
{
  "memory_struct_summary": {
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

摘要记忆数据位于记忆根目录下各 agent 目录的 `memory-struct-summary/`：

```
{根目录}/{agent-id}/memory-struct-summary/
├── llm/                                  # 模型会话（会话标识按记忆后缀，session_id 即后缀的净化形式）
│   ├── context/{session-id}.jsonl
│   └── context-history/{session-id}-{时间戳}.jsonl
├── {角色名}/                              # 角色目录：case 森林与原文副本
│   ├── tree.jsonl                        # 一个 AgentRole 的 case 森林，每行一个 case 节点
│   └── cases/{case 名}/                  # 叶子 case 的原文副本（单文件）
│       └── channel-records.jsonl
└── {记忆后缀}/                            # 水位与当前 case 指针（后缀 = 角色名时就是上面的角色目录）
    ├── current.json                      # 各通道的当前 case 指针（字段 case_name）
    └── watermark.json                    # 各通道的已处理水位
```

- `{角色名}` 为 RoleEvent 里的角色名；`{记忆后缀}` 为完整 role-event 后缀：角色模式等于角色名（此时后缀目录即角色目录），事件模式为 `{角色名}-{事件标识}`。
- **case 森林按 AgentRole 共用**：`tree.jsonl` 在角色目录下，同一 AgentRole 的角色模式与事件模式看到的是同一片森林（查询 `tree/get` 时 `nodes` 相同）；case 节点不记录模式/事件字段，节点自带 `case_name`，不另存外层键。
- **森林结构**：角色模式一棵树，固定根为隐藏的 `__role_root__`（复合 case、摘要恒为空），其子节点是 `role-` 前缀的角色 case；每个事件一棵树，根为可见的 `event-{事件标识}`，森林顶层只有角色根与各事件根。角色根只用于遍历，`case/list`/`tree/get`/`case_count` 都不返回/不计它；角色顶层 case 的 `parent_id` 在对外响应里序列化为 null。
- **原始记录、水位、当前指针、模型会话按记忆后缀分开**：原文副本目录在角色目录下按 `case 名` 命名，水位/当前指针在各自的后缀目录下，模型会话按后缀的净化形式（session_id，非字母数字与 `-`/`_`/`.` 的字符替换为 `_`，空后缀用 `_default`）分开。
- 事件模式的根 case 是 `event-{事件标识}`（命名约定与查找键）；它通常是叶子，非叶子时以它为根继续下潜，新子 case 名为 `event-{事件标识}-{模型给出的名称}`（只含英文字母、数字与 `-`，重名追加 `-2`、`-3`…）。角色模式新建 case 的 case 名为 `role-{模型给出的名称}`（名称只含英文字母、数字与 `-`，重名时追加 `-2`、`-3`…）。
- **原始记录只能写入叶子 case**：复合 case（含角色根）不持有原始记录；meta 或定位结果指向非叶子时按失败处理，不做迁移或下潜。
- **新建 case 的名称与摘要都在新建当次给出**：名称非法（含中文、空格或其他字符，或超过 64 字符）或缺摘要都按模型失败处理（保留状态、下次重试）。
- 原文副本单文件、按时间顺序追加，行内容与记忆存储的通道记录格式一致（思考/工具内容内嵌在 `content` 里）；不按日期分文件（日期切分只用于 memory-store 的原始记录文件，case 副本按时间顺序单文件累积）。
- 记忆存储侧的原始记录仍是 `{根目录}/{agent-id}/memory-store/{year}-{记忆后缀}/…`。

### tree.jsonl

```jsonc
{"case_name":"__role_root__","children":["role-weather-chat"],"summary":"","record_count":0,"created_at":"…","updated_at":"…"}
{"case_name":"role-weather-chat","parent_id":"__role_root__","children":["role-work-plan"],"summary":"…","record_count":0,"start_time":"…","end_time":"…","created_at":"2026-09-22 10:05:00","updated_at":"2026-09-22 13:49:00"}
{"case_name":"role-work-plan","parent_id":"role-weather-chat","children":[],"summary":"…","record_count":8,"start_time":"…","end_time":"…","created_at":"…","updated_at":"…"}
{"case_name":"event-e1","children":[],"summary":"…","record_count":3,"start_time":"…","end_time":"…","created_at":"…","updated_at":"…"}
{"case_name":"event-e1-topic","parent_id":"event-e1","children":[],"summary":"…","record_count":0,"start_time":"…","end_time":"…","created_at":"…","updated_at":"…"}
```

- 每行一个 case 节点，节点自带 `case_name`；`parent_id` 为森林顶层（事件根）时省略该字段（反序列化缺省即 null），角色 case 的父节点是隐藏的 `__role_root__`。整片森林写盘时按创建时间、`case_name` 顺序逐行覆盖；`__role_root__` 一行始终存在（旧数据加载时补齐）。

### current.json / watermark.json

```jsonc
// current.json：某记忆后缀各通道的当前 case 指针（无通道时文件不存在）
{
  "channels": [
    { "channel": { "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" }, "case_name": "role-work-plan" }
  ]
}

// watermark.json：某记忆后缀各通道的已处理位置（时间 + 序号）
{
  "channels": [
    {
      "channel": { "messenger_id": "web", "self_user_id": "self1", "group_id": "g1" },
      "watermark": { "time": "2026-09-22 13:48:37", "sn": 128 }
    }
  ]
}
```

- 两个文件都放在**记忆后缀目录**下，形状是「通道 → 状态」的列表；文件形状不匹配（例如旧版按类型平铺的水位文件）按状态文件损坏处理并报错，不做静默降级。
- 水位过滤按通道进行：某通道的待归档记录 = 共享记忆文件中归属该通道、且 `(time, sn)` 在该通道水位之后的通道记录。

## 模型交互

| 环节 | 输入 | 输出 |
|------|------|------|
| 第 1 轮（分组轮，仅角色模式） | 该通道当前 case 摘要 + 本批新消息（一个批次一次调用；待归档记录不参与）；消息按固定字段 `msg_id` / `time` / `user_id` / `content` 组成 JSON 数组，思考/工具内容按标签内嵌在 `content` 里 | 消息按 case 分组的组列表（各组是原文 `msg_id` 列表 + 是否属于当前 case） |
| 第 2 轮起（逐层判定，仅角色模式） | 一个组的内容 + **当层候选 case 摘要**（只有 `role-` 前缀的角色 case；当层为空时提示词写明没有候选） | 并入当层某 case（非叶子继续下潜）/ 在本层新增叶子 case——新增时**同一次输出** case 名称（英文字母、数字、`-`）与摘要 |
| 摘要生成 | 叶子 case：**当前摘要 + 本批全部待归档记录**；复合 case：**当前摘要 + 子 case 摘要**。不再读取全部历史记录 | case 摘要（用于祖先级联与事件模式的新 case；本批新建的角色 case 已在逐层判定里拿到摘要，不再重复生成） |

- 事件模式**不调分组轮**：事件根是叶子时直接归入它；事件根非叶子时以它为根调用逐层判定（新子 case 用 `event-{事件标识}-{名称}`），命中叶子即归入。该 case 由只带消息的定位建出时（还没有记录），当场按消息内容生成摘要，不留给后续调用；由带 meta 的记录建出/补全时按记录生成。
- 第 1 轮与逐层判定的提示词文案同样使用 case 术语（「当前 case」「当层 case」）。

### 分组输入（固定格式）

分组轮的批次只含**本批新消息**，按通道消息给出，一个消息一条，JSON 数组每项四个字段：

```jsonc
[
  { "msg_id": "m-1001", "time": "2026-09-22 13:48:37", "user_id": "u-1",
    "content": "你好" },
  { "msg_id": "m-1002", "time": "2026-09-22 13:48:52", "user_id": "self-1",
    "content": "<reasoning_content>用户在打招呼</reasoning_content>\n<tool_call>search {\"q\":\"天气\"}</tool_call>\n<tool_result>…</tool_result>\n今天晴，适合出门" }
]
```

- `msg_id`：分组标识，批次内唯一；同一条消息在 pending 与新消息里各占一条时加 `#序号` 后缀区分（定位结果仍按原 `msg_id` 返回）。
- `content`：整条消息合成的一个字符串。文本原样保留；思考/工具记录按其在 `Multi` 中的顺序内嵌，并分别用 `<think>…</think>`（`thinking`）、`<reasoning_content>…</reasoning_content>`（`reasoning_content`）、`<tool_call>…</tool_call>`（工具调用）、`<tool_result>…</tool_result>`（工具结果）包裹。思考/工具**不单独成条**：一条 `Multi` 消息只对应一个 `msg_id`。
- 提示词写明输入格式、分组操作与输出格式：按 case（独立话题）分组，同一话题的消息即使交错也放同一组，每条消息恰好属于一组。

结构化输出约定（系统提示词要求只输出一个 JSON 对象；解析容忍 markdown 代码块与前后解释文字）：

```jsonc
// 第 1 轮（分组轮）：每组的 msg_ids 是该组原文消息的 msg_id 列表；
// 全部 msg_id 必须恰好各出现一次（不重不漏），组间/组内都不要求连续或有序
{ "groups": [ { "msg_ids": ["m-1001", "m-1003"], "current": true },   // 属于当前 case → 直接并入当前 case
              { "msg_ids": ["m-1002"], "current": false } ] }         // 其他 case → 以组为单位进入逐层判定

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
- 第 1 轮把组标记为当前 case、但该通道没有当前 case 时：只含新消息的组容忍该不一致并退回逐层判定（定位只读、不落盘）。
- 父子层级由后续「建立 case 关联关系 / 合并为大 case」的定时程序产生（预留）。
- 各环节解析失败按失败处理并保留原状态（模型调用全部完成前不落盘、不推进水位），保留待处理标记，等待下次查询重试。
- 会话标识按**记忆后缀**编码；模型会话按 `(AgentRole, event_id)` 隔离，同一后缀的连续判定共享上下文（超过 `session_max_messages` 时归档并只保留最近的消息）；同一 AgentRole 的角色模式与事件模式各自一个会话。

## 错误类型

```rust
enum Error {
    NotifyError(ArcStr),        // 通知连接与帧解析
    StoreReadError(ArcStr),     // 原始记录读取
    ModelApiError(ArcStr),      // 模型调用（含映射自 kissbot-llm 的错误）
    StateError(ArcStr),         // case 树、水位、指针的读写与一致性
    IoError(std::io::Error),
    JsonError(serde_json::Error),
    FileError(kai_file::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
```

## 验证

- 单元测试：`cd kissbot-memory-struct-summary && cargo test`（角色/记忆后缀解析与同一 AgentRole 的角色模式与事件模式共用 case 森林、靠名称前缀区分（节点不带模式字段）、角色根隐藏与森林顶层、懒处理与待处理标记按后缀隔离、分组落位与分层下探只取 `role-` 前缀的角色 case、分组按 `msg_id` 覆盖校验（允许话题交错、不要求有序）、组按首条记忆时间排序、分组输入固定字段与思考/工具标签内嵌（一条 multi 消息只占一个条目）、通道归属与通道隔离、摘要级联、增量摘要、水位推进、case 森林读写与 tree.jsonl 形状、case 定位匹配、定位即「pending + 消息」的一次处理（无待处理标记时也定位、每通道只归档一次、msg_id 必填且唯一、同 case 的多组合并成一组、只有新消息参与分组、带 meta 的 pending 直接落 case 不调分组/逐层判定、meta 指向缺失 case 时按 meta 建出、meta/定位指向非叶子按失败处理、无 meta 的 pending 跳过但推进水位、新话题建出记忆条数为 0 的 case）、事件模式由 `event_id` 定位事件根（叶子直接归入、非叶子下潜、新子 case 为 `event-{事件标识}-*`）、新建 case 当场拿到名称与摘要（名称字符集校验、重名追加 `-2`）、空树定位仍调一次逐层判定、重建只清本后缀自己的 case 并按 meta 重建、通知帧解析、写入可见性、模型失败后的状态保留）。
- 集成测试：`test/tests/memory-struct-summary-api.spec.ts` 用固定记忆数据目录验证「通知只登记 → 查询只读（不触发归档）→ 手动触发归档 → 多条消息定位（pending + 消息一次处理，按 msg_id 标识消息，同一 case 的多组合并）→ 定位新话题建出空 case → 摘要/原文分页查询 → 重建 → 事件模式归入 `event-{事件标识}` 事件根且与角色模式共用森林、定位新话题建出的 case 当场带名称与摘要 → 通道隔离 → 思考内容随所在通道记录保存」全流程，模型调用以桩实现替换（`test/workspace-template/config.json` 配置 `provider_type = "stub"`）。
