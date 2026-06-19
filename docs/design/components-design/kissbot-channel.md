# kissbot-channel 组件设计

## 概述
消息通道框架组件，定义 Messenger 接口（包含原 Channel 接口的消息收发功能），提供通道管理器作为核心协调层。负责连接外部系统和 nexus，协调消息的路由与推送。具体的通讯应用接入由通道实现模块完成。

## 核心功能

1. **消息路由**：接收外部系统的上行消息转发到 nexus，接收 nexus 的下行消息转发到对应外部系统
2. **Nexus 绑定**：处理 nexus 的绑定请求，为每个 (user, group) 组合创建消息收发通道
3. **群组变化通知**：当通道实现模块检测到群组变化时，通过 WSS 通知 nexus

## 内部模块

### 1. ChannelManager - 通道管理器
模块的核心协调层，内部持有：
- messengers：按 messenger_id 索引的已注册 Messenger 实例集合
- wss_server：WSS Server 实例（管理 nexus 连接）
- memory_store_client：与记忆存储模块通信的客户端
- message_queue：消息队列（暂存待推送消息，不持久化）

职责：
- **Messenger 管理**：注册多个 Messenger 实例，按 ID 查找
- **Nexus 绑定**：处理 nexus 的 bind 请求，指定多个 user_id 后由 Messenger 决定 group 列表，为每个 (user,group) 组合创建对应的消息收发通道
- **消息队列**：维护消息队列，依次推送至 nexus（WSS）和记忆存储模块
- **WSS Server 管理**：管理 nexus 连接的生命周期
- **消息路由**：外部消息和 nexus 消息均通过 ChannelManager 路由
- **群组变化事件**：通过 WSS 向 nexus 推送 group 变化通知
- **附件下载**：根据 key 从对应 Messenger 获取附件数据

### 2. Messenger 接口（由通道实现模块实现）
代表一个通讯应用接入。Messenger 负责管理用户的群组信息和附件存储，并可通过特定标识（如 messenger_id + group_id + user_id）向对应外部用户发送消息。接收外部消息后通过回调通知 ChannelManager。

Messenger 接口包含以下能力：
- 读取 MessengerInfo（包括 messenger 标识、用户列表、用户的群组列表）
- 向指定 (user, group) 组合的目标发送消息
- 支持注册消息到达回调（外部消息到达时通知 ChannelManager）
- 支持注册群组变化回调
- 支持附件数据下载

### 3. WSSServer - WSS 服务器
- 作为 WSS 服务器等待 nexus 连接
- 每个 nexus 对应唯一的 WSS 连接
- 管理多个 nexus 连接，支持心跳检测和重连

### 4. MemoryStoreClient - 记忆存储通信组件
- 与记忆存储模块通信，由 ChannelManager 驱动
- 从消息队列读取消息并推送至记忆存储模块

## 关键设计

### 消息结构
使用统一的公共消息格式。msg_type 为 text 时 content 为实际文本，否则 content 为全局唯一 key。
在 channel 的实现中，应通过 key 关联附件。

## 内部流程

### 消息上行（外部系统 → nexus）
1. 外部系统（如 Web 页面）发送消息
2. 通道实现组件接收消息，保存附件（如有），构建消息记录
3. 通过回调通知通道管理器将消息入队
4. ChannelManager 处理消息队列：推送消息到记忆存储组件 + 通过 WSS 将消息发送给 nexus

### 消息下行（nexus → 外部系统）
1. nexus 生成回复消息，通过 WSS 发送到通道管理器
2. ChannelManager 接收，按 messenger_id + group_id + user_id 查找对应 Messenger 的发送方法
3. 消息入队
4. ChannelManager 处理消息队列：推送消息到记忆存储组件 + 调用 Messenger 的发送方法发送到外部系统

### Nexus 绑定流程
1. nexus 发送绑定请求（指定 messenger_id 和 user_ids）
2. ChannelManager 验证用户身份
3. 对每个用户，查询其所在的群组列表
4. 为每个 (用户, 群组) 组合建立消息收发通道
5. 注册消息到达回调和群组变化回调

### 群组变化通知流程
通道实现组件检测到用户加入或离开群组 → 调用通道管理器注册的群组变化回调 → 通道管理器通过 WSS 将变化事件发送给 nexus → nexus 更新本地的通道列表。

### 附件下载流程
nexus 发送附件下载请求（携带 key）→ 通道管理器根据 key 调用对应通道实现组件的附件获取接口 → 获取附件数据 → 将附件数据返回给 nexus。
