# kissbot-channel 组件设计

## 概述
消息通道框架组件，定义 Messenger 接口和 Channel 接口，提供通道管理器作为核心协调层。负责连接外部系统和 nexus，协调消息的路由与推送。具体的通讯应用接入由通道实现模块完成。

## 核心功能

1. **消息路由**：接收外部系统的上行消息转发到 nexus，接收 nexus 的下行消息转发到对应外部系统
2. **Nexus 绑定**：处理 nexus 的绑定请求，为每个 (user, group) 组合创建消息收发通道
3. **群组变化通知**：当通道实现模块检测到群组变化时，通过 WSS 通知 nexus

## 内部模块

### 1. ChannelManager - 通道管理器
模块的核心协调层，内部持有：
- messengers：按 messenger_id 索引的已注册 Messenger 实例集合
- channels：按 messenger_id + group_id + user_id 索引的已创建 Channel 实例集合
- wss_server：WSS Server 实例（管理 nexus 连接）
- memory_store_client：与记忆存储模块通信的客户端
- message_queue：消息队列（暂存待推送消息，不持久化）

职责：
- **Messenger 管理**：注册多个 Messenger 实例，按 ID 查找
- **Channel 管理**：通过 Messenger 创建 Channel 实例，维护全局索引
- **Nexus 绑定**：处理 nexus 的 bind 请求，指定多个 user_id 后由 Messenger 决定 group 列表，为每个 (user,group) 组合创建 Channel 实例
- **消息队列**：维护消息队列，依次推送至 nexus（WSS）和记忆存储模块
- **WSS Server 管理**：管理 nexus 连接的生命周期
- **消息路由**：外部消息和 nexus 消息均通过 ChannelManager 路由
- **群组变化事件**：通过 WSS 向 nexus 推送 group 变化通知
- **附件下载**：根据 key 从对应 Messenger 获取附件数据

### 2. Messenger 接口（由通道实现模块实现）
代表一个通讯应用接入。可以读取 MessengerInfo（包括 messenger 标识、用户列表、用户的群组列表），可以创建 Channel 实例，支持注册群组变化回调。

Messenger 负责管理用户的群组信息、管理附件存储（接收外部消息时保存附件，发送消息时保存附件后发送），接收外部消息后通过回调通知 ChannelManager。

### 3. Channel 接口（由通道实现模块实现）
表示一个 (messenger, group, user) 组合的消息收发通道。可以读取 ChannelInfo（messenger 标识、群组标识、用户标识），可以向外部系统发送消息（附件携带原始数据），支持注册消息到达回调和附件数据下载回调。Channel 不维护消息列表，不存储消息历史。

### 4. WSSServer - WSS 服务器
- 作为 WSS 服务器等待 nexus 连接
- 每个 nexus 对应唯一的 WSS 连接
- 管理多个 nexus 连接，支持心跳检测和重连

### 5. MemoryStoreClient - 记忆存储通信组件
- 与记忆存储模块通信，由 ChannelManager 驱动
- 从消息队列读取消息并推送至记忆存储模块

## 关键设计

### 消息结构
channel 消息的 msg_type 为 text 时 content 为实际文本，否则 content 为全局唯一 key。channel 通过 key 关联附件（如图片、文件等），memory 按 key 存储对应的二进制内容。

## 内部流程

### 消息上行（外部系统 → nexus）
1. Messenger 收到外部消息 → 保存附件 → 构建消息记录
2. 调用 on_message_received 回调 → 消息入队
3. ChannelManager 处理队列：推送记忆存储模块 + 通过 WSS 发送给 nexus

### 消息下行（nexus → 外部系统）
1. nexus 通过 WSS 发送消息 → WSS Server 接收
2. ChannelManager 解析 → 按 messenger_id + group_id + user_id 查找 Channel 实例
3. 消息入队 → 处理队列：推送记忆存储模块 + 调用 channel.send_message()

### Nexus 绑定流程
1. nexus 发送 bind 请求（messenger_id + user_ids）
2. ChannelManager 验证用户 → 获取 group 列表
3. 为每个 (user,group) 创建 Channel 实例
4. 注册回调 → 加入索引 → 返回绑定确认

### 附件下载流程
nexus WSS 请求 → ChannelManager 按 key 调用 Messenger.get_attachment_data() → 返回数据
