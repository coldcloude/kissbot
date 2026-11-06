# 记忆摘要 case 路径标识与定位流程改造设计

> 日期：2026-11-04
> 范围：`kissbot-memory-struct-summary` 的 case 标识、locate 的 LLM 流程、归档落位与存储布局；`kissbot-agent` 的 case 会话 id；`kissbot-api` 的注释；`docs/` 下的相关文档。
> 不改 `blog/`（人工文档，禁止修改）。

## 背景

当前 case 以全局唯一的扁平 `case_name` 作为键：角色 case 名 `role-{模型名称}`，事件根 `event-{event_id}`。这样无法表达：

- 不同子树里可以出现相同的 case 名（不同父下的同名 case 是两个 case）；
- 同一父下同名应当视为同一个 case，而不是追加 `-2`、`-3`；
- 模型返回的名称应直接使用，不再由组件拼前缀；
- 定位阶段只返回 case 名，不再返回摘要。

本设计把 case 的唯一标识改为**相对森林的完整路径**，LLM 始终只看到/返回路径的最后一段；存储目录与树结构一一对应。

## 1. Case 标识与树结构

### 1.1 路径即标识

case 的唯一标识是相对森林的完整路径，段之间用 `/` 连接：

| 对象 | `case_name`（唯一标识） | 存储目录（相对 `cases/`） |
|---|---|---|
| 隐藏角色根 | `[role]` | `role` |
| 角色顶层 case | `<模型名>` | `role/<模型名>` |
| 角色子 case | `<父路径>/<模型名>` | `role/<父路径>/<模型名>` |
| 事件根 | `[event]<event_id>` | `event-<event_id>` |
| 事件子 case | `[event]<event_id>/<模型名>` | `event-<event_id>/<模型名>` |

示例：

- 角色顶层天气 case：`weather_chat`，目录 `cases/role/weather_chat`；
- 其下的子 case：`weather_chat/rain`，目录 `cases/role/weather_chat/rain`；
- 事件 `e1` 的根：`[event]e1`，目录 `cases/event-e1`；
- 其下的子 case：`[event]e1/topic`，目录 `cases/event-e1/topic`。

`CaseTree` 内部以完整路径为 `BTreeMap` 的键；`CaseNode.case_name`、`parent_id`、`children`、当前 case 指针、记录 meta 的 `case_name` 全部使用完整路径。隐藏角色根 `[role]` 只用于组织角色树，对外查询隐藏。

### 1.2 重名语义

- **同一父节点下最后一段相同 = 同一个 case**：创建时直接复用已有节点，不追加 `-2`、`-3`。
- **不同父节点下最后一段相同 = 不同 case**：`weather_chat/rain` 与 `work/rain` 是两个 case，路径不同。
- 路径是精确查找键，不再用裸名在多个子树间猜测。
- 事件根以 `[event]` 开头，角色 case 名只含字母/数字/下划线，二者不会冲突；事件子 case 直接使用模型名。

### 1.3 名称与路径校验

- 模型返回的 case 名：只能含 `[A-Za-z0-9_]`，长度 1–64。只校验字符组成与长度，不额外判断保留字（`[role]`、`[event]` 含括号，天然不会由模型产生）。
- `event_id`：维持现有规则，`[A-Za-z0-9-]`，长度 1–64；事件根路径段为 `[event]{event_id}`。
- 完整路径：总长 ≤ 200；不允许空段、`.`、`..`、首尾 `/`；首段可以是 `[event]{event_id}` 或模型名，其余段必须是模型名。`[role]` 只作为隐藏根，不接受外部 meta 指向它。
- meta 的 case_name 按同样规则校验；非法则跳过该记录并告警（沿用“不可用 meta 跳过”的行为）。

## 2. locate 的 LLM 流程

### 2.1 第 1 轮：分组轮（保持现有结构）

输入本批消息 + 该通道当前 case。当前 case 在提示词里只展示**最后一段**（和摘要；摘要为空则只给名称）。输出：

```json
{"groups":[{"msg_ids":["m1","m2"],"current":true}]}
```

标记 `current` 的组直接命中该通道当前 case 指针；没有当前 case 时退回第 2 轮。分组输出的覆盖校验保持不变。

### 2.2 第 2 轮：逐层判定

对每个非当前组，从当前层开始逐层调用模型。当层候选是该父节点下的直接子 case，提示词里只展示每个候选的**最后一段**（和摘要；摘要为空则只给名称）。模型输出只有一个字段：

```json
{"case_name":"rain"}
```

处理规则：

- 与当层某个候选的最后一段相同：
  - 该候选是叶子 → 归入该 case，返回其完整路径；
  - 该候选是复合 case → 下潜到它的子层，继续判定。
- 与当层所有候选都不同 → 视为新名称，在**当前层**新建叶子 case，路径 = 父路径 + `/` + 名称；新建后不再下钻。
- 当层没有候选时，提示词要求模型必须返回一个新名称。
- 模型输出不再包含 `new`/`target` 标志，也不再包含摘要。

### 2.3 事件模式

- 事件根固定为 `[event]{event_id}`，不向 LLM 询问名称或摘要。
- 事件根不存在：直接建出叶子（摘要为空），整个后缀归入它。
- 事件根是叶子：直接命中。
- 事件根是复合 case：以它为父层，走 2.2 的逐层判定；新子 case 路径为 `[event]{event_id}/<模型名>`。

### 2.4 新建 case 与摘要

- locate 阶段不再生成任何摘要：角色新 case、事件根、事件子 case 的摘要都为空。
- 新建的 case 一律是叶子；摘要等消息写入记忆、归档时按记录生成。
- 定位结果 `LocatedGroup.case_name` 返回完整路径；同一路径的多个组在返回前合并。

### 2.5 模型接口

`DecisionModel::decide_level` 由返回 `LevelDecision` 改为返回 `Result<ArcStr>`（模型给出的最后一段名称）；`LevelDecision` 枚举删除。提示词明确名称字符集与“已有则给已有名、否则给新名”。

## 3. 归档、meta 与存储

### 3.1 meta 落位

- 记录 meta 的 `memory_summary.case_name` 是完整路径。
- 归档 pending：
  - 路径存在且是叶子：直接写入；
  - 路径存在但非叶子：跳过该 case 的记录并告警（不写记录、不更新当前指针，沿用现有行为）；
  - 路径不存在：逐段建出——缺的祖先建成空摘要复合节点，最后一段建成叶子；然后写记录、生成摘要。
- 祖先节点只作为容器，不持有原文副本；原文副本只写叶子。

### 3.2 原文副本目录

原文副本按树结构落盘，子 case 就是子目录：

```text
{role_dir}/cases/role/weather_chat/channel-records.jsonl
{role_dir}/cases/role/weather_chat/rain/channel-records.jsonl
{role_dir}/cases/event-e1/topic/channel-records.jsonl
```

`case_name` → 目录的映射：首段 `[role]` → `role`，`[event]{event_id}` → `event-{event_id}`，普通模型名 → 先加 `role` 再按段拼接；其余段原样拼接。

### 3.3 重建

- 角色模式：清空该 AgentRole 的全部角色 case（含嵌套）与 `cases/role` 目录，保留 `[role]` 节点。
- 事件模式：清空 `[event]{event_id}` 整棵子树与 `cases/event-{event_id}` 目录，其他事件根与角色 case 不受影响。
- 清理后按 meta 重新归档；没有 meta 的记录跳过。
- 摘要级联、祖先计算均用完整路径；给模型渲染子 case 摘要时展示最后一段。

## 4. 查询与对外接口

- `LocatedGroup.case_name`、`CaseNode.case_name/parent_id/children`、`tree/get` 的当前 case 指针、`case/list` 的 `parent_id`、`case/summary` 与 `case/records*` 的 `case_name` 全部是完整路径。
- `case/list`：
  - `parent_id` 缺省或 `null`：森林顶层（角色顶层 + 事件根）；
  - `parent_id = [role]`：角色顶层（内部特殊值）；
  - 其他值：该路径的直接子层。
- 查询按完整路径精确匹配；裸名不再解析。隐藏角色根 `[role]` 不作为业务 case，查询它按不存在处理。

## 5. agent 会话 id

`memory_summary_pipeline` 的 `case_session_id` 用 case_name 推导。case 路径只可能包含字母、数字、下划线、`[`、`]`、`-`、`/`，其中 `.` 不会出现在任何段里（event_id 允许 `-`，但不允许 `.`），因此把 `/` 替换为 `.` 即可得到文件名安全且不碰撞的 session id：

```text
[event]e1/topic  ->  [event]e1.topic
```

其余逻辑（会话按 case_name 缓存、meta 带完整路径）不变。

## 6. 兼容性

- 不做旧数据迁移：旧的 `role-` / `event-` 扁平名称不再识别；升级后建议按新规则重建。
- 测试数据与测试断言统一改为新命名。
- `docs/` 下的相关文档同步更新；`blog/` 不修改。
- 代码内已过时的注释同步更新或删除。

## 7. 验证

单元测试（`kissbot-memory-struct-summary`）：

- 路径索引：角色顶层/子层、事件根/子层、隐藏角色根不可见、`from_nodes` 往返；
- 兄弟同名复用同一节点（无 `-2` 后缀）；不同父下同名是两个节点；
- 名称校验：字母/数字/下划线通过，中文、空格、连字符、`.`、空名、超长拒绝；
- 逐层判定：返回已有叶子→归入；返回已有复合→下潜；返回新名→当前层建叶子且不再下钻；空候选必须给新名；
- locate 只返回 case_name、无摘要；新 case 摘要为空；
- 事件模式：根固定 `[event]{id}`、缺失时建叶子、非叶子下潜、不调分组轮、不向 LLM 要根名称；
- 归档 meta：全路径、缺祖先逐段建出、非叶子跳过、原文副本按目录树落盘；
- 重建：角色模式清 `cases/role`，事件模式清 `cases/event-{id}`；
- 当前 case 指针与 `tree/get` 使用完整路径。

集成测试（TypeScript）：更新现有用例到新命名；覆盖“跨子树同名不同 case”和“同父同名同 case”。

验证命令：

```bash
cd kissbot-memory-struct-summary && cargo test
cd kissbot-agent && cargo test
cd test && npx playwright test tests/memory-struct-summary-api.spec.ts
```

## 8. 明确不做

- 不实现旧数据迁移或旧名称兼容。
- 不修改 `blog/`。
- 不改变消息分组覆盖校验、水位推进、原文副本内容格式。
- 不在 locate 阶段生成摘要。
