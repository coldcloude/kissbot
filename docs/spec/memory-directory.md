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
│   │       ├── channel-{messenger_id}={user_id}={group_id}-records-{date}.jsonl
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
| channel-{messenger_id}={user_id}={group_id}-records-{date}.jsonl | 消息通道的文本记录（按 messenger、user、group 和时间组织） |
| think-records-{date}.jsonl | 思考内容记录 |
| tool-call-records-{date}.jsonl | 工具调用记录 |
| tool-result-records-{date}.jsonl | 工具调用结果记录 |

## 自我认知文件

`{agent-id}/memory-ego/` 目录下存储 agent 的自我认知数据：

| 文件 | 内容 |
|------|------|
| metadata.json | agent 元数据 |
| individual-recognition-.json | 用户识别信息 |
| role-play-{role-name}.json | 角色设定（每个角色一个文件） |
