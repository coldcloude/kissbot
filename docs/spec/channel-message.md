# 消息模型

## msg_type 与 content

- 消息类型由 `msg_type` 标识，内容统一使用类型化的 `Content` 结构，二者对应：
  - `text`：文本
  - `attachment`：附件（见 [channel-attachment.md](channel-attachment.md)）
  - `system_group_join` / `system_group_leave`：群组变更通知
  - `user_remove`：用户移除通知
  - `multi`：复合消息
- `multi` 消息的 content 为消息项（msg_type + content）列表，可嵌套，一条消息可携带多种内容

## 消息方向

- `OutgoingMessage`：发送方发出的消息，不含 msg_id、time
- `IncomingMessage`：接收方收到的消息，含 msg_id、time（自身回显由 nexus 按 msg_id 识别，无 is_self 字段）
