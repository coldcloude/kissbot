# channel-web 消息流转

> 消息模型见 [channel-message.md](channel-message.md)

## ID 与时间分配

- 发送方不决定消息的 ID 和时间，由 messenger 在 out 转 in 时填入当前时间并分配 ID
- 消息 ID 格式：时间（`yyyyMMddHHmmss`）+ 自增序号（固定 6 位，超过回 0）

## Outgoing→Incoming 转换

- 统一由 messenger 完成转换，admin 通过 Web 发消息和 agent 通过 WSS 发消息走同一套处理
- 转换时完成：权限校验、附件注册、msg_id 和 time 分配

## 群消息分发

- 按群组成员为每个成员生成一条 IncomingMessage
- 分发消息携带发送者的 messenger_name/user_name/group_name；自身回显由 agent 按 msg_id 识别（不按接收者计算 is_self）
- admin 不参与成员分发，通过 SSE 接收所有群组的消息

## 先存后推

- 消息先写入本地存储（见 [channel-web-message-storage.md](channel-web-message-storage.md)），发送方在消息进入写入队列后即获得响应
- 消息实际落盘后才执行推送：SSE 推 admin、按成员分发 IncomingMessage
