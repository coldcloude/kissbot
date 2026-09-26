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
│   │       ├── channel-records-{date}.jsonl
│   │       ├── think-records-{date}.jsonl
│   │       ├── tool-call-records-{date}.jsonl
│   │       └── tool-result-records-{date}.jsonl
│   └── memory-struct-*/          # 各记忆结构实现的数据
```

`memory-struct-*` 下由各实现模块自行组织数据。摘要记忆实现模块（kissbot-memory-struct-abstract）的目录布局：

```
{根目录}/{agent-id}/memory-struct-abstract/
├── llm/                          # 模型会话（会话标识按记忆后缀，session_id 即后缀的净化形式）
├── {角色名}/                      # 角色目录：case 树与原文副本
│   ├── tree.json                 # 一棵 case 树（角色模式与事件模式共用）
│   └── cases/{case-id}/          # case 原文副本，每类一个文件
│       ├── channel-records.jsonl
│       ├── think-records.jsonl
│       ├── tool-call-records.jsonl
│       └── tool-result-records.jsonl
└── {记忆后缀}/                    # 水位与当前 case 指针（后缀 = 角色名时就是上面的角色目录）
    ├── current.json              # 各通道的当前 case 指针（字段 case_id）
    └── watermark.json            # 各通道的已处理水位
```

case 树按**角色**共用；原始记录、水位、当前 case 指针、模型会话按**记忆后缀**分开。case id 命名：角色模式为 `role-1`、`role-2`…（角色树内递增），事件模式固定为 `event-{事件标识}`（顶层叶子）。详见 [memory-struct-abstract.md](memory-struct-abstract.md)。

## role-event 后缀命名

`suffix` 由调用方（nexus、memory-struct）拼接，路径构造器不做解析。摘要记忆实现模块按**第一个** `-` 把它拆成「角色名 + 可选事件标识」（角色名不允许含 `-`，事件标识可以含 `-`，如 UUID）：

- 角色记忆：`{role-name}`（不含 `-`），在 memory-store 里形如 `{year}-{role-name}`（role_name 为空时形如 `2026-`）
- 事件记忆：`{role-name}-{event-id}`，在 memory-store 里形如 `{year}-{role-name}-{event-id}`（role_name 为空时形如 `2026--{event-id}`）

例：`dev-3f2a-c3d4` 的角色是 `dev`、事件标识是 `3f2a-c3d4`。

## 文件格式

记录使用 JSON Lines 格式，便于追加和流式读取。

| 文件 | 内容 |
|------|------|
| channel-records-{date}.jsonl | 消息通道的文本记录（同 agent、角色、日期下全部通道消息；记录内携带 msg_id 与 messenger/group/self 身份） |
| think-records-{date}.jsonl | 思考内容记录 |
| tool-call-records-{date}.jsonl | 工具调用记录 |
| tool-result-records-{date}.jsonl | 工具调用结果记录 |

## ChannelRecord 身份字段与语义

channel 记录包含完整身份字段：

| 字段 | 含义 |
|------|------|
| msg_id | 消息标识：上行取自 `incoming_message.msg_id`、下行取自 `outgoing_message_response.msg_id`；思考/工具占位记录为空 |
| user_id | 消息发送者身份 |
| self_user_id | agent 在 channel 绑定的用户（接收方身份 / agent 视角的 self） |
| messenger_id | 消息来源通道标识 |
| group_id | 群组标识（单聊可为空） |
| is_self | 是否 agent 实际发送（1 / 0） |

> **msg_id 与去重**：消息与已存记录是否同一条，在**单通道批次内只按 `msg_id`** 判定（归档与定位都按通道分开调用），不比较内容；`msg_id` 为空（思考/工具占位记录、加字段前的旧记录）的条目不参与去重、原样保留。
>
> **is_self 与 self_user_id 不同**：is_self 只在 agent 实际发送消息时为 1（agent 经 msg_id 回显匹配识别并过滤自身回声后确定），不由 `user_id == self_user_id` 推导。其他人使用 agent 绑定的用户发送消息时，`user_id == self_user_id`，但 `is_self == 0`。

## 自我认知文件

`{agent-id}/memory-ego/` 目录下存储 agent 的自我认知数据：

| 文件 | 内容 |
|------|------|
| metadata.json | agent 元数据 |
| individual-recognition-.json | 用户识别信息 |
| role-play-{role-name}.json | 角色设定（每个角色一个文件） |
