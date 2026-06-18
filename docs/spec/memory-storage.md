# 记忆存储文件结构

## 目录组织

记忆系统根目录下，按 agent ID 划分各子目录。

角色记忆模式和事件记忆模式区别仅在于目录名的后缀：

### 角色记忆模式
`{agent-id}/memory-store/{年}-{角色名}/`

### 事件记忆模式
`{agent-id}/memory-store/{年}-{角色名}-{事件ID}/`

## 文件分类

每种后缀目录下，按日期分文件存储四种记录类型：

| 文件名模式 | 存储内容 |
|------------|----------|
| channel-{messenger_id}={user_id}={group_id}-records-{日期}.jsonl | 消息通道的文本记录 |
| think-records-{日期}.jsonl | 思考内容记录 |
| tool-call-records-{日期}.jsonl | 工具调用记录 |
| tool-result-records-{日期}.jsonl | 工具调用结果记录 |

记录使用 JSON Lines 格式，便于追加和流式读取。

## 自我认知文件

`{agent-id}/memory-ego/` 目录下存储 agent 的自我认知数据：
- 元数据文件
- 用户识别信息文件
- 角色设定文件（每个角色一个文件）
