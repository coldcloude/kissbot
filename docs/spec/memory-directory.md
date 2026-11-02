# 记忆存储文件结构

## 目录组织

记忆系统根目录下，按 agent ID 划分各子目录：

```
{根目录}/
├── {agent-id}/
│   ├── agent-{agent-id}          # agent 存在标识文件
│   ├── memory-ego/               # 自我认知数据
│   ├── memory-store/
│   │   └── {year}-{suffix}/
│   │       └── channel-records-{date}.jsonl
│   └── memory-struct-*/          # 各记忆结构实现的数据
```

`memory-struct-*` 下由各实现模块自行组织数据。摘要记忆实现模块（kissbot-memory-struct-summary）的目录布局：

```
{根目录}/{agent-id}/memory-struct-summary/
├── llm/                          # 模型会话（会话标识按记忆后缀，session_id 即后缀的净化形式）
├── {角色名}/                      # 角色目录：case 树与原文副本
│   ├── tree.jsonl                # 一棵 case 树（角色模式与事件模式共用），每行一个 case 节点
│   └── cases/{case 名}/          # case 原文副本（单文件）
│       └── channel-records.jsonl
└── {记忆后缀}/                    # 水位与当前 case 指针（后缀 = 角色名时就是上面的角色目录）
    ├── current.json              # 各通道的当前 case 指针（字段 case_name）
    └── watermark.json            # 各通道的已处理水位
```

case 树按**角色**共用；原始记录、水位、当前 case 指针、模型会话按**记忆后缀**分开。**case 名就是 case 的唯一标识**：角色模式为 `role-{模型给出的名称}`（名称只含英文字母、数字与 `-`，重名时追加 `-2`、`-3`…），事件模式为 `event-{事件标识}`（顶层叶子）；前缀不同，两种模式天然不会重名。详见 [memory-struct-summary.md](memory-struct-summary.md)。

## 记忆后缀（RoleEvent）

记忆后缀是 memory 内部的文件/目录名段，由项目级基础类型 `RoleEvent { role_name, event_id }` 拼成。**拼接由 memory（kissbot-memory 的 `role_event_suffix`，只在构造文件名时拼接）负责**，对外通信一律传 `RoleEvent`，调用方不再自己拼接字符串（`suffix` 只出现在 memory 内部的路径构造）：

- 角色记忆：`{role_name}`（`event_id` 缺省），在 memory-store 里形如 `{year}-{role_name}`（角色名为空时形如 `2026-`）
- 事件记忆：`{role_name}-{event_id}`，在 memory-store 里形如 `{year}-{role_name}-{event_id}`（角色名为空时形如 `2026--{event_id}`）

磁盘上的历史目录名按**第一个** `-` 识别角色名与可选事件标识：角色名不允许含 `-`，事件标识可以含 `-`（如 UUID）。例：`dev-3f2a-c3d4` 的角色是 `dev`、事件标识是 `3f2a-c3d4`。

## 文件格式

记录使用 JSON Lines 格式，便于追加和流式读取。

| 文件 | 内容 |
|------|------|
| channel-records-{date}.jsonl | 通道记录（同 agent、角色、日期下全部通道消息；记录内携带 msg_id、messenger/group/self 身份与可选的 meta）。思考/工具内容内嵌在 `content` 里，不单独成记录 |

## ChannelRecord 身份字段与语义

channel 记录包含完整身份字段：

| 字段 | 含义 |
|------|------|
| msg_id | 消息标识：上行取自 `incoming_message.msg_id`、下行取自 `outgoing_message_response.msg_id` |
| user_id | 消息发送者身份 |
| self_user_id | agent 在 channel 绑定的用户（接收方身份 / agent 视角的 self） |
| messenger_id | 消息来源通道标识 |
| group_id | 群组标识（单聊可为空） |
| is_self | 是否 agent 实际发送（1 / 0） |
| meta | 可选的记录元信息；摘要记忆使用 `meta.summary_memory.case_name` 记录所属 case，缺省不序列化 |

> **msg_id 与定位**：摘要记忆的调用方在 locate 之后才写入本批消息，因此 locate 不按 `msg_id` 与已存记录去重；`msg_id` 保留用于记录关联与排查。带 `meta.summary_memory.case_name` 的记录由摘要记忆组件直接按 case 名归档，见 [memory-struct-summary.md](memory-struct-summary.md)。
>
> **内嵌的思考/工具内容**：`Content` 的 `Think` / `ToolCall` / `ToolResult` 分支直接保存对应的 `ThinkRecord` / `ToolCallRecord` / `ToolResultRecord`（不含 `key` / `time` / `sn`）。agent 在一次 run 内把思考与工具内容累积到回复通道记录的 `Multi` 中，随该通道记录一起写入并等待落盘；没有文本的轮次不写通道记录，累积的思考/工具内容随之丢弃。工具调用与结果按它们在 `Multi` 中的先后顺序配对。
>
> **is_self 与 self_user_id 不同**：is_self 只在 agent 实际发送消息时为 1（agent 经 msg_id 回显匹配识别并过滤自身回声后确定），不由 `user_id == self_user_id` 推导。其他人使用 agent 绑定的用户发送消息时，`user_id == self_user_id`，但 `is_self == 0`。

## 自我认知文件

`{agent-id}/memory-ego/` 目录下存储 agent 的自我认知数据：

| 文件 | 内容 |
|------|------|
| metadata.json | agent 元数据 |
| individual-recognition-.json | 用户识别信息 |
| role-play-{role-name}.json | 角色设定（每个角色一个文件） |
