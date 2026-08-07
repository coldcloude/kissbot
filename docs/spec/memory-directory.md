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

`suffix` 由调用方（nexus、memory-struct）拼接，路径构造器不做解析：
- 角色记忆：`{year}-{role-name}`（role_name 为空时形如 `2026-`）
- 事件记忆：`{year}-{role-name}-{event-id}`（role_name 为空时形如 `2026--{event-id}`）

## 文件格式

记录使用 JSON Lines 格式，便于追加和流式读取。

| 文件 | 内容 |
|------|------|
| channel-records-{date}.jsonl | 消息通道的文本记录（同 agent、角色、日期下全部通道消息；记录内携带 messenger/group/self 身份） |
| think-records-{date}.jsonl | 思考内容记录 |
| tool-call-records-{date}.jsonl | 工具调用记录 |
| tool-result-records-{date}.jsonl | 工具调用结果记录 |

## ChannelRecord 身份字段与语义

channel 记录包含完整身份字段：

| 字段 | 含义 |
|------|------|
| user_id | 消息发送者身份 |
| self_user_id | agent 在 channel 绑定的用户（接收方身份 / agent 视角的 self） |
| messenger_id | 消息来源通道标识 |
| group_id | 群组标识（单聊可为空） |
| is_self | 是否 agent 实际发送（1 / 0） |

> **is_self 与 self_user_id 不同**：is_self 只在 agent 实际发送消息时为 1（agent 经 msg_id 回显匹配识别并过滤自身回声后确定），不由 `user_id == self_user_id` 推导。其他人使用 agent 绑定的用户发送消息时，`user_id == self_user_id`，但 `is_self == 0`。

## 自我认知文件

`{agent-id}/memory-ego/` 目录下存储 agent 的自我认知数据：

| 文件 | 内容 |
|------|------|
| metadata.json | agent 元数据 |
| individual-recognition-.json | 用户识别信息 |
| role-play-{role-name}.json | 角色设定（每个角色一个文件） |
