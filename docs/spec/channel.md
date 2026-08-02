# channel 通道框架

> 消息模型见 [channel-message.md](channel-message.md)，附件见 [channel-attachment.md](channel-attachment.md)

## 组件结构

- channel 是消息通道框架（库）：定义 Messenger 接口、ChannelManager、AttachmentRegistry 等
- 具体通讯应用由通道实现组件实现 Messenger 接口接入（如 channel-web）

## Messenger 接口与注册

- Messenger 接口能力：读取通道信息（用户 / 群组）、发送消息、上传附件分块、下载头请求、启动下载推送
- 注册时由 ChannelManager 注入四个回调：群组变化、消息到达、附件下载数据、用户删除；回调以 Weak 弱引用传入，避免循环引用
- 按 messenger_id 注册和查找，重复注册报错

## 连接管理

- 每个 agent 连接分配全局递增 connect_id
- 连接需通过 API key 认证，心跳保活（10 秒）
- 连接关闭时清理该连接在所有 messenger 上的绑定；遍历与删除分步执行，避免并发 map 死锁

## 绑定模型

- agent 按 (messenger_id, user_id) 绑定，绑定记录携带 connect_id、agent_id、role_name
- agent_id / role_name 用于消息写入记忆库时的归属
- 同一 user 只能绑定一个连接；绑定、解绑、发消息、下载附件都校验绑定归属本连接

## 消息路由

### 上行：双路分发

- messenger 上报 IncomingMessage 后，并行执行两路处理：
  - 按绑定找到 agent 连接，推送消息
  - 写入 memory-store

### 下行

- agent 发 OutgoingMessage → 校验绑定 → 按 messenger_id 路由到对应 Messenger 发送
- 发送响应中的附件注册 transfer_id 到上传路由表（见 [channel-attachment.md](channel-attachment.md)）

## 写入 memory-store

- channel 将 IncomingMessage 聚合后批量推送到 memory-store（见 [memory-store.md](memory-store.md)）
- 推送复用 kai-file 批量追加框架：append 入队即返回，按批量大小或超时触发一次 HTTP 批量提交
- 推送以 force 方式写入：信任消息源时间，乱序时由 memory-store 重排

## 群组变化处理

- 群组加入 / 退出事件统一转换为系统消息（Content::GroupJoin / GroupLeave），记忆记录 is_self=0，agent 按 Content 类型跳过 agentic loop
- 同时向 agent 发送 join / leave 控制通知
- 顺序：join 先通知后发系统消息，leave 先发系统消息再通知

## 用户删除处理

- messenger 上报用户删除 → channel 移除该用户绑定 → 通知 agent（user_removed）
