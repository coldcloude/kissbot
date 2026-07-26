# channel 附件注册与传输

> channel-web 的附件实现见 [channel-web-attachment.md](channel-web-attachment.md)

## 两阶段表示

- 发送方在消息中携带 `AttachmentInfo`（file_name、mime_type、size_bytes），不包含任何存储标识
- messenger 在消息分发前完成附件注册，将 `AttachmentInfo` 转换为 `AttachmentInfoResponse`，嵌入：
  - `key`：附件的全局唯一标识
  - `transfer_id`：本次传输的路由标识
- 注册由 channel 组件统一定义（`AttachmentRegistry` 抽象），各 messenger 实现；注册过程递归处理 multi 嵌套消息中的所有附件

## 传输路由

- `transfer_id` 是上传/下载分块传输的路由标识，与 key 解耦
- channel 侧为上传和下载各维护独立的 transfer_id → 上下文映射：
  - 上传方向：OutgoingMessage 发送响应后注册（transfer_id → messenger + 附件信息）
  - 下载方向：下载头请求后注册（transfer_id → agent 连接 + 附件信息）
- 传输完成（最后一块确认）或出错后清理对应的映射

## 上传

- 消息发送方在收到 AttachmentInfoResponse 后，按 transfer_id 分块上传附件数据
- 分块为二进制帧：transfer_id + size + pos + 数据
- 按 pos 顺序写入：
  - 重复块（pos 落后于当前写入位置）幂等返回成功
  - 跳号（pos 超前）返回乱序错误码，由发送方纠正

## 下载

- 下载分两步：
  1. 请求方携带 key 请求下载头，messenger 返回 AttachmentInfoResponse（含为本次下载新分配的 transfer_id）
  2. messenger 主动推送分块，每块需请求方确认后推下一块
- 推送分块时由 channel 预组帧：buffer 预留数据偏移，messenger 直接填入数据，避免二次拷贝
- channel 侧对下载请求做绑定校验，只有绑定该 user 的连接可下载其附件
