# Channel

## 设计实现

- **代码组织** 核心的交互逻辑提取为 Channel Server、Channel Client
  - Server 对接消息工具，作为消息工具的代理，和 Client 通信
  - Client 对接消息工具的用户，作为 agent 或人的代理，和 Server 通信
  - 消息工具抽象为 Messenger trait，客户端抽象为 Terminal trait

- **通信流程**
  - 发送消息：Terminal 发送消息给 Messenger，消息的 msg_id 在 Messenger 侧生成，Terminal 从返回值获取
  - 接收消息：Terminal 发送绑定申请（messenger + user）给 Messenger，Messenger 绑定成功后推送所有 group 的消息至 Terminal
  - 上传附件：Terminal 发送的消息中包含附件消息，Messenger 收到后生成 key 和 transferId，Terminal 保存 key 备用，并根据 transferId 发送实际数据块
  - 下载附件：Terminal 发送下载请求至 Messenger，Messenger 生成 transferId 返回 Terminal，然后根据 key 查找对应的数据块，带 TransferId 发送给 Terminal
  - key 是附件的永久标识，transferId 是附件在传输过程中的临时标识
  - 附件要求顺序传输，发现既存块时跳过，发现跳跃块时返回错误，并报告当前预期位置
  - 入群/退群统一转成系统消息，入群先通知再发系统消息，退群先发系统消息再通知
  - 用户删除，先解除绑定再通知
  - WS 应支持断线重连

- **调用方向** 按照消息流动顺序调用
  - 发消息、上传文件、发送下载请求时，Terminal -> Client -> Server -> Messenger
  - 接收消息、下载文件时，Messenger -> Server -> Client -> Terminal

- **消息存储**
  - 仅作为对接层时，不存储历史消息，所有历史消息由对接的消息工具保存
  - 包含消息工具的完整实现时，应有消息和附件存储、读取功能

- **绑定**
  - WS 连接时，Server 给 Client 分配连接 ID
  - 一个连接 ID 可以绑定多个 ChannelUser（messenger + user）
  - 一个 ChannelUser 只能被一个连接 ID 绑定
