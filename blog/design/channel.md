# Channel 设计

## 通信流程

- 发送消息：Terminal 发送消息给 Messenger，消息的 msg_id 在 Messenger 侧生成，Terminal 从返回值获取
- 接收消息：Terminal 发送绑定申请（messenger + user）给 Messenger，Messenger 绑定成功后推送所有 group 的消息至 Terminal
- 上传附件：Terminal 发送的消息中包含附件消息，Messenger 收到后生成 key 和 transferId，Terminal 保存 ke 备用，并根据 transferId 发送实际数据块
- 下载附件：Terminal 发送下载请求至 Messenger，Messenger 生成 transferId 返回 Terminal，然后根据 key 查找对应的数据块发送给 Terminal
