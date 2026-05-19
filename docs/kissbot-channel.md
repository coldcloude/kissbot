# kissbot-channel 模块设计

## 模块概述
消息通道框架，抽象出messenger（通讯应用）、group（群组）、user（用户）三层概念，连接外部系统（如web界面、QQ、Matrix等），向agent输入消息，并接收agent的输出消息。

- **Messenger**：表示一个通讯应用（如QQ、Web、Matrix），每个kissbot-channel-xxx模块都是对一个Messenger trait的实现，同时实现对应的Channel trait。Messenger负责管理可用的user和group信息，接收外部消息并路由到对应的channel实例，并为agent创建channel实例。
- **Group**：表示一个群组/会话，是消息的组织单位。一个group中有多个user，可以接收到group中所有user发的消息。
- **User**：表示一个通讯应用中的用户账号。一个agent在一个messenger中可以绑定多个user，一个user在一个messenger中可以收发多个group的消息。

**Channel实例**：一个agent绑定的一个user加入的一个group，构成一个消息收发单元，即一个channel实例。channel-id唯一标识一个messenger+group+user的组合，例如`channel_id = "qq:交流群:id123456"`。Channel实例是Channel trait实现类的实例，由具体的kissbot-channel-xxx模块实现。

**ChannelManager**：模块的核心统筹协调层，可以注册多个Messenger实例，管理Channel实例的生命周期，协调WSS连接与Channel之间的消息路由，维护消息队列驱动推送至agent和memory-store。

## 职责
- 定义Messenger trait，便于实现不同的通讯应用接入（web、QQ、Matrix等）
- 定义Channel trait，表示messenger+group+user组合的消息收发通道
- **ChannelManager统筹管理**：
  - 管理多个Messenger实例的注册
  - 管理Channel实例的创建和查询（提供获取全部channel的方法）
  - 管理WSS Server（每个agent一个连接）
  - 协调WSS连接与Channel实例之间的消息路由
  - 维护消息队列，驱动推送至agent（WSS）和memory-store
  - 处理agent的附件下载请求（附件由各实现模块管理）
  - 向agent传递group变化事件
- Messenger负责维护user和group信息、接收外部消息并路由到channel实例
- 维护消息队列暂存待推送的消息（不持久化存储）

## 架构设计

### 核心架构
```
┌──────────────────────────────────────────────────────────────────────┐
│                          kissbot-channel                             │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │                      ChannelManager                          │    │
│  │  ┌──────────────┐  ┌──────────┐  ┌──────────┐               │    │
│  │  │  Messengers  │  │ 消息队列  │  │ WSS      │               │    │
│  │  │  (注册管理)   │  │ (待推送)  │  │ Server   │               │    │
│  │  └──────┬───────┘  │ agent/   │  │ 管理     │               │    │
│  │         │          │ memory   │  └──────────┘               │    │
│  │         │          └──────────┘                              │    │
│  └─────────┼────────────────────────────────────────────────────┘    │
│            │                                                          │
│  ┌─────────▼──────────────────────────────────────────────────────┐  │
│  │  ┌───────────────────────┐    ┌───────────────────────┐       │  │
│  │  │  Messenger实现(M)     │    │  Channel实现(C)       │       │  │
│  │  │  (kissbot-channel-*)  │    │  (kissbot-channel-*)  │       │  │
│  │  │  - 管理user/group     │    │  - 消息收发           │       │  │
│  │  │  - 外部消息路由       │    │  - 回调通知           │       │  │
│  │  │  - 管理附件存储(内部) │    │  - 附件管理(内部)     │       │  │
│  │  └───────────────────────┘    └───────────────────────┘       │  │
│  │                           │                                  │  │
│  │                           ▼                                  │  │
│  │                on_message_received回调                        │  │
│  │                on_group_change回调                            │  │
│  │                (ChannelManager注册)                           │  │
│  └──────────────────────────────────────────────────────────────┘  │
│                                                                      │
└─────────────────────────────────────────┬────────────────────────────┘
                                          │
                                  ┌───────▼────────┐
                                  │   Agent(WSS)   │
                                  └────────────────┘

ChannelManager注册多个Messenger，通过Messenger和Channel的抽象函数完成各项功能的协调。
各具体kissbot-channel-xxx模块同时实现Messenger trait和Channel trait，
```

## Messenger Trait设计

### Messenger trait

Messenger trait 定义通讯应用接入的标准接口，由各 kissbot-channel-xxx 模块实现。

**回调函数类型（由 ChannelManager 注册）：**

- `OnMessageReceived`：外部消息到达时的回调函数，接收 `MessageRecord`，由 ChannelManager 注册。
- `OnGroupChange`：用户加入或离开 group 的事件回调，接收 `GroupChangeEvent`，由 ChannelManager 注册。

**Messenger trait 方法：**

- `messenger_id`：返回 messenger 的唯一标识（如 "qq"、"web"、"matrix"）。
- `get_available_users`：获取该 messenger 中可用的用户列表，返回 `UserInfo` 列表。
- `get_user_groups`：根据 user_id 获取指定用户可用的群组列表，返回 `GroupInfo` 列表。
- `create_channel`：为指定的 agent、user、group 创建 Channel trait 实现类的实例，返回 Channel 实例。
- `register_on_group_change`：注册 group 变化事件回调，由 ChannelManager 调用。
- `get_attachment_metadata`：通过 key 获取附件元数据（供 ChannelManager 调用）。
- `get_attachment_data`：通过 key 获取附件数据（供 ChannelManager 调用）。

**相关数据结构：**

- `UserInfo`：用户信息，包含 user_id（用户标识）、user_name（用户名）、avatar（头像，可选）。
- `GroupInfo`：群组信息，包含 group_id（群组标识）、group_name（群组名称）、group_type（群组类型，如群组/私聊/频道）。
- `GroupChangeEvent`：群组变化事件，包含 messenger_id、user_id、group_id、group_name、change_type（变化类型）、timestamp（时间戳）。
- `GroupChangeType`：变化类型枚举，包含 Joined（用户加入）和 Left（用户离开）。

Messenger的实现由具体的kissbot-channel-xxx模块完成，自主管理附件存储，维护自身状态，销毁即停止。

## Channel Trait设计

### Channel trait

Channel trait 表示一个（messenger, group, user）组合的消息收发通道，由各 kissbot-channel-xxx 模块实现。

**Channel trait 方法：**

- `channel_id`：返回 channel 的唯一标识（格式："{messenger_id}:{group_id}:{user_id}"）。
- `messenger_id`：返回所属 messenger 的标识。
- `agent_id`：返回绑定的 agent 标识。
- `group_id`：返回所属群组标识。
- `user_id`：返回所属用户标识。
- `register_on_message_received`：注册外部消息到达回调，由 ChannelManager 在创建 channel 时调用。
- `send_message`：发送消息到外部系统，由 ChannelManager 调用。消息中的附件携带原始数据。
- `get_status`：获取 channel 状态，返回 `ChannelStatus`。

Channel实例由具体的kissbot-channel-xxx模块实现。Channel不维护消息列表，不存储消息历史。外部消息通过Messenger或Channel注册的回调通知ChannelManager，agent消息通过send_message发送到外部系统。

消息中的附件处理：
- **外部消息的附件**：由Messenger接收外部消息时自行保存，将key放入消息记录
- **agent消息的附件**：agent发送消息时携带附件原始数据（Attachment），Messenger在send_message中保存附件后发送到外部系统，将key放入消息记录

## 核心组件设计

### 1. ChannelManager - 统筹管理器
ChannelManager 是 kissbot-channel 模块的核心统筹层，通过调用 Messenger 和 Channel 的抽象方法完成各项功能。

**ChannelManager 内部持有：**
- `messengers`：已注册的 Messenger 实例集合（按 messenger_id 索引）。
- `channels`：已创建的 Channel 实例集合（按 channel_id 索引）。
- `wss_server`：WSS Server 实例。
- `memory_store`：MemoryStoreClient 实例。
- `message_queue`：消息队列，暂存待推送的消息。

**ChannelManager 方法：**

- `register_messenger`：注册一个 Messenger 实例。
- `get_messenger`：根据 messenger_id 获取已注册的 Messenger 实例。
- `handle_agent_bind`：处理 agent 绑定请求，传入 agent_id、messenger_id 和多个 user_id，由 Messenger 决定每个 user 加入的 group，为每个 (user, group) 组合创建 Channel 实例，返回 Channel 信息列表。
- `get_all_channels`：获取指定 agent 在指定 messenger（可选，不传则返回所有）下的所有 channel 实例信息，返回 Channel 信息列表。
- `handle_agent_message`：处理来自 agent 的消息（通过 WSS 接收）。
- `handle_attachment_download`：处理 agent 的附件下载请求，根据 messenger_id 和 key 查找附件数据并返回。
- `enqueue_message`：将消息加入消息队列，按 channel_id 区分。
- `process_message_queue`：处理消息队列，将指定 channel（可选，不传则处理所有）的队列中消息依次推送给 agent（WSS）和 memory-store。
- `start`：启动 WSS Server 等组件。
- `stop`：停止所有组件。

**职责：**
- **Messenger管理**：注册多个Messenger实例；根据messenger_id查找已注册的Messenger
- **Channel管理**：通过Messenger创建Channel实例；根据channel_id查找Channel实例；维护Channel实例的全局索引；提供`get_all_channels`方法（按agent_id和messenger_id筛选）
- **Agent绑定**：处理agent的bind请求（指定多个user_id），由Messenger决定每个user的group列表，为每个(user,group)组合创建Channel实例，注册回调，返回绑定结果
- **注册回调**：向Messenger注册`on_message_received`和`on_group_change`回调
- **消息队列**：维护消息队列暂存待推送的消息，依次推送给agent（WSS）和memory-store
- **WSS Server管理**：持有WSS Server实例；管理agent连接的生命周期
- **消息路由**：
  - 外部消息处理链路：Messenger收到外部消息 → 调用`on_message_received`回调（ChannelManager注册）→ 回调将消息入队 → 处理队列：推送memory-store、通过WSS发给agent
  - agent消息处理链路：WSS收到agent消息 → ChannelManager → 消息入队 → 处理队列：推送memory-store、调用channel.send_message发到外部系统
- **Group变化事件**：Messenger检测到user加入或离开group → 调用`on_group_change`回调 → ChannelManager通过WSS通知agent
- **附件下载**：收到agent的附件下载请求，根据messenger_id查找对应的Messenger，调用其`get_attachment_data`获取附件数据并返回

### 2. Messenger实现 - 通讯应用接入
- 每个具体的kissbot-channel-xxx模块实现Messenger trait，负责：
  - 维护该通讯应用中的user信息和group信息
  - 管理附件存储
  - 接收来自对应外部系统的消息
  - 收到外部消息后，调用ChannelManager注册的`on_message_received`回调
  - 检测user加入或离开group，调用ChannelManager注册的`on_group_change`回调
  - 通过Channel的send_message将agent回复发送到外部系统，并在发送前保存附件
- Messenger销毁即停止，不需要单独的start/stop接口

### 3. ChannelInstance - Channel实例（由kissbot-channel-xxx模块实现）
- 每个channel实例对应一个（messenger, group, user）三元组
- 是Channel trait实现类的实例，由具体的kissbot-channel-xxx模块实现
- 不维护消息列表，不存储消息历史
- 消息中的附件处理：
  - 外部消息中的附件由Messenger接收时自行保存，将key放入消息记录
  - agent回复中的附件携带原始数据（Attachment），send_message时保存附件后发送

### 4. WSSServer - WSS服务器
- 作为WSS服务器，等待agent连接
- 每个agent对应唯一的WSS连接
- 接收来自agent的消息，交给ChannelManager处理
- 将ChannelManager转发的消息发送给agent
- 管理多个agent连接
- 心跳检测和连接重连

### 5. MemoryStoreClient - 记忆存储通信组件
- 与memory-store通信，将消息推送至记忆系统
- 由ChannelManager驱动，从消息队列中读取消息并推送

## 数据结构定义

### MessageRecord（消息队列中的记录）
消息队列中的记录，用于暂存待推送给 agent 或 memory-store 的消息。

- `channel_id`：所属 channel 标识。
- `user_id`：发送者用户标识。外部用户消息为对应用户的 user_id，agent 发出的消息为 agent 标识。
- `is_self`：是否为自己（agent）发送的消息。1 表示 agent 自己发的，0 表示外部用户发的。
- `msg_type`：消息类型，可填任意字符串。内置默认类型为 `text`（纯文本）和 `file`（文件/附件），后续可扩展 `image`、`audio`、`video` 等。agent 也可自定义类型（如 `think`、`tool_call`），channel 可选择识别，也可透传不识别。
- `content`：
  - 当 `msg_type` 为 `text` 时，content 为纯文本字符串。
  - 当 `msg_type` 为其他类型时，content 为 JSON 字符串，包含该类型所需的结构化信息。
- `attachments`：附件原始数据列表（仅 `file` 类型消息携带，需要 channel 保存处理后发送）。每个附件包含 filename、mime_type、size_bytes、data。
- `timestamp`：时间戳。

**content 非 text 类型时的 JSON 结构说明：**

当 `msg_type` 不为 `text` 时，content 为 JSON 字符串，由 `msg_type` 的值决定内容结构。channel 仅需识别 `file` 类型（处理附件），其余类型可透传。

- **file 类型消息**：content 中包含附件 key 列表（附件数据已由 Messenger/Channel 保存），channel 需要识别并处理。
- **agent 自定义类型**（如 `think`、`tool_call` 等）：content 格式由 agent 自行定义，channel 不解析，仅透传。

### OutgoingMessage（发送到外部系统的消息）

每条 OutgoingMessage 表示一条独立的消息，`msg_type` 决定其类型，不会混杂多种类型。

- `channel_id`：目标 channel 标识。
- `target_user_id`：目标用户标识（可选，不传表示广播给 group 中所有可见成员）。
- `msg_type`：消息类型，可填任意字符串（与 MessageRecord 一致）。
- `content`：
  - 当 `msg_type` 为 `text` 时，content 为纯文本字符串。
  - 当 `msg_type` 为其他类型时，content 为 JSON 字符串（与 MessageRecord 一致）。
- `attachments`：附件原始数据。非text消息可以选择携带一条或多条attachment，channel能识别对应 `msg_type` 时，可以根据content的JSON解析原始数据，比如保存文件等。

### Attachment（附件原始数据）

- `name`：文件名。
- `mime_type`：MIME 类型。
- `size_bytes`：文件大小（字节）。
- `data`：附件原始数据。

### AttachmentRef（附件引用，存储后的引用信息）

- `key`：附件 key。
- `name`：文件名。
- `mime_type`：MIME 类型。
- `size_bytes`：文件大小（字节）。

### ChannelStatus

- `channel_id`：channel 标识。
- `messenger_id`：所属 messenger 标识。
- `group_id`：所属群组标识。
- `user_id`：所属用户标识。
- `agent_id`：绑定的 agent 标识。
- `is_running`：是否正在运行。

### ChannelInfo（用于返回给 agent 的 channel 信息）

- `channel_id`：channel 标识。
- `messenger_id`：messenger 标识。
- `group_id`：群组标识。
- `group_name`：群组名称。
- `user_id`：用户标识。

## 消息流程

### 外部系统 → Agent 流程
```
1. 外部系统发送消息到messenger（如QQ收到群消息）
   ↓
2. Messenger收到消息，保存附件（如果有），构建MessageRecord
   ↓
3. Messenger调用ChannelManager注册的on_message_received回调
   ↓
4. 回调将消息入队：ChannelManager.enqueue_message()
   ↓
5. ChannelManager处理消息队列：
   ├─ 调用MemoryStoreClient.push_message_record()推送至memory-store
   └─ 通过WSS Server将消息发送给agent
   ↓
6. Agent接收并处理消息
```

### Agent → 外部系统 流程
```
1. Agent生成回复消息，通过WSS发送到kissbot-channel
   （attachments中携带附件原始数据Attachment）
   ↓
2. WSS Server接收消息，交给ChannelManager处理
   ↓
3. ChannelManager解析消息，按channel_id查找对应的Channel实例
   ↓
4. ChannelManager构建MessageRecord，调用enqueue_message()入队
   ↓
5. ChannelManager处理消息队列：
   ├─ 调用MemoryStoreClient.push_message_record()推送至memory-store
   └─ 调用channel.send_message()（Channel实例保存附件后发送到外部系统）
```

### Agent绑定流程
```
1. Agent通过WSS发送bind请求（指定messenger_id和多个user_id）
   ↓
2. WSS Server接收bind消息，交给ChannelManager
   ↓
3. ChannelManager调用messenger.get_available_users()验证用户是否存在
   ↓
4. 对每个user_id，调用messenger.get_user_groups()获取用户所在的group列表
   ↓
5. ChannelManager调用messenger.create_channel()为每个(user,group)组合创建Channel实例
   ↓
6. ChannelManager向Messenger注册on_message_received和on_group_change回调
   ↓
7. ChannelManager将Channel实例加入全局索引
   ↓
8. ChannelManager通过WSS返回绑定确认（含所有channel实例的信息）
```

### Agent获取全部channel流程
```
1. Agent通过WSS发送get_channels请求
   ↓
2. ChannelManager返回当前所有channel实例的ChannelInfo列表
```

### Group变化通知流程
```
1. Messenger检测到user加入或离开group
   ↓
2. Messenger调用ChannelManager注册的on_group_change回调
   ↓
3. ChannelManager通过WSS将GroupChangeEvent发送给agent
   ↓
4. Agent更新自己的channel列表
```

### Agent附件下载流程
```
1. Agent通过WSS发送附件下载请求（携带key）
   ↓
2. WSS Server接收请求，交给ChannelManager
   ↓
3. ChannelManager调用messenger.get_attachment_data(key)获取附件数据
   ↓
4. ChannelManager通过WSS将附件数据返回给agent
```

### 说明
- **ChannelManager是消息路由的中央协调者**，所有消息的流转都经过ChannelManager
- **消息队列暂存待推送的消息**，由ChannelManager依次推送给agent（WSS）和memory-store，不持久化存储
- **agent不能通过channel查询历史消息**，但可以通过key下载附件文件
- **回调机制**：ChannelManager向Messenger注册`on_message_received`和`on_group_change`回调
- **附件随消息走**：外部消息的附件由Messenger保存；agent发送的附件携带原始数据，由Channel在send_message时保存
- **消息类型统一**：所有消息统一通过 `msg_type` + `content` 表示，不再使用独立的附件字段。`msg_type` 可填任意字符串，内置默认类型为 `text`（纯文本）和 `file`（文件/附件）。非 text 类型的 content 为 JSON 字符串，由 `msg_type` 决定其结构
- **agent 处理思考与工具调用**：agent 可自定义 msg_type（如 `think`、`tool_call`），将相关内容写入 content（JSON 格式），channel 可以解析，不能解析也可透传或丢弃
- **channel 的职责边界**：channel 仅需识别 `text` 和 `file` 两种消息类型，对 `file` 或其他类型的消息，可以识别处理，也可以透传或丢弃。
- 消息发送者采用 `user_id` + `is_self` 标识，与 memory-store 的 API 数据结构保持一致

## 附件存储设计

附件存储为各kissbot-channel-xxx实现模块的内部实现，不在channel基础模块中详细设计。

基础模块仅要求：
- Messenger提供`get_attachment_metadata`和`get_attachment_data`接口供ChannelManager调用
- 附件的来源为channel消息（无论是接收的还是发送的），统一存储不做区分

各实现模块自行决定附件存储的实现方式（文件系统、数据库等）、目录结构和清理策略。

## 与memory-store通信设计

ChannelManager通过MemoryStoreClient与memory-store通信，在处理消息队列时统一推送。
注意：记忆不按channel区分，所有channel的记忆按时间顺序混合存储。

### 推送方式

ChannelManager持有MemoryStoreClient，在处理消息队列时驱动推送：

```
                    ChannelManager
                         │
          ┌──────────────┼──────────────┐
          │              │              │
          ▼              ▼              ▼
   Messenger     消息队列(msg_queue)   WSS Server
          │              │              │
          │              │              │
          │    ┌─────────▼─────────┐    │
          └───►│  MemoryStoreClient│◄───┘
               │  (逐条推送)       │
               └─────────┬─────────┘
                         │
                         ▼
                   memory-store
```

### 推送内容

消息队列中的每条 MessageRecord 记录推送至 memory-store，包含：

1. **msg_type 和 content**：text 类型推送纯文本内容；其他类型推送 content 中的 JSON 结构化信息。

### 记忆结构

根据记录的类型，每条 MessageRecord 在 memory-store 中形成结构化存储： agent_id、role_name、channel_id、user_id、is_self、msg_type、content、时间等字段一一对应。

### MemoryStoreClient接口

MemoryStoreClient 负责将消息推送至记忆系统，由 ChannelManager 驱动调用。一次可推送多条

## WSS消息协议

每个agent对应唯一的WSS连接，所有channel实例共享此连接与agent通信。
消息中通过`channel_id`字段区分消息属于哪个channel实例。

### Agent → Channel 消息（type: "outgoing_message"）

agent 发送消息时，每条消息为独立类型，通过 `msg_type` 区分。消息类型为 `outgoing_message`，data 中携带：
- `channel_id`：目标 channel 标识。
- `msg_type`：消息类型，可填任意字符串（与 MessageRecord 一致）。内置默认类型为 `text` 和 `file`。
- `content`：
  - `text` 类型：纯文本字符串。
  - 其他类型：JSON 字符串，由 `msg_type` 决定其结构。
- `attachments`：附件原始数据列表。

### Channel → Agent 消息（type: "incoming_message"）

channel 转发外部消息给 agent 时，消息类型为 `incoming_message`，data 中携带：
- `channel_id`：来源 channel 标识。
- `user_id`：发送者用户标识。
- `is_self`：0（外部用户消息，始终为 0）。
- `msg_type`：消息类型，可填任意字符串（与 MessageRecord 一致）。
- `content`：
  - `text` 类型：纯文本字符串。
  - 其他类型：JSON 字符串，由 `msg_type` 决定其结构。
- `timestamp`：消息时间。

### Agent 绑定消息（type: "bind"）

agent 接入时发送绑定请求，消息类型为 `bind`，data 中携带：
- `agent_id`：agent 标识。
- `messenger_id`：目标 messenger 标识。
- `user_ids`：要绑定的用户标识列表（多个）。由 Messenger 决定每个 user 的 group 列表。

### 绑定确认（type: "bind_ack"）

ChannelManager 完成绑定后返回确认，消息类型为 `bind_ack`，data 中携带：
- `agent_id`：agent 标识。
- `channels`：创建的 channel 信息列表，每个元素包含 channel_id、messenger_id、group_id、group_name、user_id。

### 获取全部 channel（type: "get_channels" / "channels"）

agent 请求获取当前所有 channel，发送 type 为 `get_channels` 的消息（无额外参数）。
ChannelManager 返回 type 为 `channels` 的消息，data 中携带 channels 列表（每个元素同上）。

### Group 变化通知（type: "group_change"）

Messenger 检测到 group 变化后，ChannelManager 向 agent 推送通知，消息类型为 `group_change`，data 中携带：
- `messenger_id`：messenger 标识。
- `user_id`：用户标识。
- `group_id`：群组标识。
- `group_name`：群组名称。
- `change_type`：变化类型（"joined" 或 "left"）。
- `timestamp`：时间戳。

### Agent 附件下载请求（type: "attachment_download"）

agent 请求下载附件，消息类型为 `attachment_download`，data 中携带：
- `key`：附件 key。

### 附件下载响应（type: "attachment_data"）

ChannelManager 返回附件数据，消息类型为 `attachment_data`，data 中携带：
- `key`：附件 key。
- `filename`：文件名。
- `mime_type`：MIME 类型。
- `data`：base64 编码的附件数据。

### 心跳消息（type: "ping" / "pong"）

agent 发送 `ping`，ChannelManager 回复 `pong`，均无额外 data 字段。

## 实现决策

### 框架实现
- 定义Messenger trait，便于实现不同的通讯应用接入
- 定义Channel trait，表示一个（messenger, group, user）组合的消息通道
- **ChannelManager作为核心统筹层**，可以注册多个Messenger实例
- 提供channel-web作为参考实现（同时实现Messenger trait和Channel trait）
- 使用tokio作为异步运行时
- 使用tokio-tungstenite实现WSS服务器
- 使用serde进行JSON序列化
- 使用dashmap实现高并发数据结构
- 使用futures实现异步任务
- 支持从配置文件加载证书
- 与memory-store通信使用kissbot-memory-store提供的SDK/API

### 附件存储实现
- 各kissbot-channel-xxx实现模块的内部实现
- 基础模块仅要求Messenger提供`get_attachment_metadata`和`get_attachment_data`接口
- 各实现模块自行决定存储方式、目录结构、key格式和清理策略

### Messenger与Channel的关系
- Messenger trait和Channel trait均在channel基础模块中定义
- 每个kissbot-channel-xxx模块**同时实现**Messenger trait和Channel trait
- Messenger的`create_channel`方法返回该模块实现的Channel实例
- Messenger负责管理user和group信息，接收外部消息
- Channel实例不存储消息，通过Messenger的回调通知ChannelManager
- 附件处理：外部消息的附件由Messenger保存；agent消息的附件在send_message时由Channel保存

### ChannelManager的定位
- ChannelManager是模块的入口和核心统筹层，对外提供统一的操作接口
- ChannelManager注册多个Messenger实例，通过messenger_id进行查找和路由
- ChannelManager调用Messenger的抽象函数管理通讯应用接入、回调注册和附件操作
- ChannelManager调用Channel的抽象函数管理消息发送
- ChannelManager持有WSS Server、MemoryStoreClient，维护消息队列协调消息推送
- ChannelManager处理agent绑定（传多个user_id，group由Messenger决定）
- ChannelManager向agent传递group变化事件，提供获取全部channel的方法（按agent_id和messenger_id筛选）

## 开发计划

### 第1阶段：基础结构搭建
- [ ] 配置Cargo.toml，添加依赖
- [ ] 定义Messenger trait（含回调注册接口和附件管理接口）
- [ ] 定义Channel trait
- [ ] 定义核心数据结构（MessageRecord, OutgoingMessage, Attachment等）
- [ ] 定义错误类型和事件类型

### 第2阶段：核心组件实现
- [ ] 实现ChannelManager（注册多个Messenger，消息队列，回调注册，agent绑定，group变化通知）
- [ ] 实现WSS Server（每agent一个连接，支持附件下载协议）
- [ ] 实现MemoryStoreClient（对接消息队列统一推送）

### 第3阶段：Messenger/Channel实现示例
- [ ] 实现channel-web作为参考实现（同时实现Messenger和Channel trait）
- [ ] 实现agent绑定流程（传user_ids，Messenger决定group列表）
- [ ] 实现channel实例创建与回调注册
- [ ] 实现附件存储（作为实现模块内部组件）

### 第4阶段：与memory-store集成
- [ ] 实现MemoryStoreClient与kissbot-memory-store的通信
- [ ] 实现ChannelManager消息队列驱动批量推送

### 第5阶段：Channel Web完善
- [ ] 完善与前端Web界面的通信
- [ ] 实现group变化事件通知
- [ ] 实现消息收发

### 第6阶段：测试和完善
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化