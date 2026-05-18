# kissbot-channel 模块设计

## 模块概述
消息通道框架，抽象出messenger（通讯应用）、group（群组）、user（用户）三层概念，连接外部系统（如web界面、QQ、Matrix等），向agent输入消息，并接收agent的输出消息。

- **Messenger**：表示一个通讯应用（如QQ、Web、Matrix），每个kissbot-channel-xxx模块都是对一个Messenger trait的实现，同时实现对应的Channel trait。Messenger负责管理可用的user和group信息，接收外部消息并路由到对应的channel实例，并为agent创建channel实例。
- **Group**：表示一个群组/会话，是消息的组织单位。一个group中有多个user，可以接收到group中所有user发的消息。
- **User**：表示一个通讯应用中的用户账号。一个agent在一个messenger中可以绑定多个user，一个user在一个messenger中可以收发多个group的消息。

**Channel实例**：一个agent绑定的一个user加入的一个group，构成一个消息收发单元，即一个channel实例。channel-id唯一标识一个messenger+group+user的组合，例如`channel_id = "qq:交流群:id123456"`。Channel实例是Channel trait实现类的实例，由具体的kissbot-channel-xxx模块实现。

**ChannelManager**：模块的核心统筹协调层，以Messenger为构造入参（一个ChannelManager对应一个Messenger），管理Channel实例的生命周期，协调WSS连接与Channel之间的消息路由，维护消息队列驱动推送至agent和memory-store。

## 职责
- 定义Messenger trait，便于实现不同的通讯应用接入（web、QQ、Matrix等）
- 定义Channel trait，表示messenger+group+user组合的消息收发通道
- **ChannelManager统筹管理**：
  - 以Messenger为构造入参，一个ChannelManager对应一个Messenger
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
│  │  │  Messenger   │  │ 消息队列  │  │ WSS      │               │    │
│  │  │  (构造入参)   │  │ (待推送)  │  │ Server   │               │    │
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

ChannelManager以Messenger为构造入参，通过Messenger和Channel的抽象函数完成各项功能的协调。
各具体kissbot-channel-xxx模块同时实现Messenger trait和Channel trait，
```

## Messenger Trait设计

### Messenger trait
```rust
/// 外部消息到达时的回调函数类型，由ChannelManager注册
type OnMessageReceived = Arc<dyn Fn(MessageRecord) -> Result<(), ChannelError> + Send + Sync>;

/// 用户加入或离开group的事件回调，由ChannelManager注册
type OnGroupChange = Arc<dyn Fn(GroupChangeEvent) -> Result<(), ChannelError> + Send + Sync>;

trait Messenger: Send + Sync + 'static {
    /// 获取messenger的唯一标识（如 "qq", "web", "matrix"）
    fn messenger_id(&self) -> &str;

    /// 获取该messenger中可用的用户列表
    async fn get_available_users(&self) -> Result<Vec<UserInfo>, ChannelError>;

    /// 获取指定用户可用的群组列表
    async fn get_user_groups(&self, user_id: &str) -> Result<Vec<GroupInfo>, ChannelError>;

    /// 为指定的agent、user、group创建channel实例
    /// 返回Channel trait实现类的实例（由kissbot-channel-xxx模块实现）
    async fn create_channel(
        &self,
        agent_id: &str,
        user_id: &str,
        group_id: &str,
    ) -> Result<Arc<dyn Channel>, ChannelError>;

    /// 注册group变化事件回调（由ChannelManager调用）
    async fn register_on_group_change(&self, callback: OnGroupChange) -> Result<(), ChannelError>;

    // ---- 附件查询接口（供ChannelManager调用） ----
    /// 通过key获取附件元数据
    async fn get_attachment_metadata(&self, key: &str) -> Result<AttachmentMetadata, ChannelError>;

    /// 通过key获取附件数据
    async fn get_attachment_data(&self, key: &str) -> Result<Vec<u8>, ChannelError>;
}

struct UserInfo {
    user_id: String,
    user_name: String,
    avatar: Option<String>,
}

struct GroupInfo {
    group_id: String,
    group_name: String,
    group_type: GroupType,  // 如群聊、私聊、频道等
}

enum GroupType {
    Group,      // 群组
    Private,    // 私聊
    Channel,    // 频道/讨论组
}

struct GroupChangeEvent {
    messenger_id: String,
    user_id: String,
    group_id: String,
    group_name: String,
    change_type: GroupChangeType,
    timestamp: String,
}

enum GroupChangeType {
    Joined,     // 用户加入群组
    Left,       // 用户离开群组
}
```

Messenger的实现由具体的kissbot-channel-xxx模块完成，自主管理附件存储，维护自身状态，销毁即停止。

## Channel Trait设计

### Channel trait
```rust
trait Channel: Send + Sync + 'static {
    /// 获取channel的唯一标识（格式："{messenger_id}:{group_id}:{user_id}"）
    fn channel_id(&self) -> &str;

    /// 获取所属messenger的ID
    fn messenger_id(&self) -> &str;

    /// 获取绑定的agent ID
    fn agent_id(&self) -> &str;

    /// 获取群组ID
    fn group_id(&self) -> &str;

    /// 获取用户ID
    fn user_id(&self) -> &str;

    /// 注册外部消息到达回调（由ChannelManager在创建channel时调用）
    async fn register_on_message_received(&self, callback: OnMessageReceived) -> Result<(), ChannelError>;

    /// 发送消息到外部系统（由ChannelManager调用）
    /// 消息中的attachments包含附件原始数据（Attachment）
    async fn send_message(&self, message: OutgoingMessage) -> Result<(), ChannelError>;

    /// 获取channel状态
    async fn get_status(&self) -> Result<ChannelStatus, ChannelError>;
}
```

Channel实例由具体的kissbot-channel-xxx模块实现。Channel不维护消息列表，不存储消息历史。外部消息通过Messenger或Channel注册的回调通知ChannelManager，agent消息通过send_message发送到外部系统。

消息中的附件处理：
- **外部消息的附件**：由Messenger接收外部消息时自行保存，将key放入消息记录
- **agent消息的附件**：agent发送消息时携带附件原始数据（Attachment），Messenger在send_message中保存附件后发送到外部系统，将key放入消息记录

## 核心组件设计

### 1. ChannelManager - 统筹管理器
ChannelManager是kissbot-channel模块的核心统筹层，以Messenger为构造入参，通过调用Messenger和Channel的抽象函数完成各项功能。

```rust
struct ChannelManager {
    messenger: Arc<dyn Messenger>,
    channels: HashMap<String, Arc<dyn Channel>>,  // channel_id -> Channel
    wss_server: Arc<WssServer>,
    memory_store: Arc<dyn MemoryStoreClient>,
    message_queue: Vec<MessageRecord>,  // 消息队列，暂存待推送的消息
}

impl ChannelManager {
    /// 创建ChannelManager，绑定指定的Messenger
    async fn new(messenger: Arc<dyn Messenger>, wss_server: Arc<WssServer>, memory_store: Arc<dyn MemoryStoreClient>) -> Self;

    /// 处理agent绑定请求：指定messenger_id和多个user_id，
    /// 由messenger决定每个user加入的group，为每个(user,group)组合创建Channel实例
    async fn handle_agent_bind(&self, agent_id: &str, messenger_id: &str, user_ids: Vec<String>) -> Result<Vec<ChannelInfo>, ChannelError>;

    /// 获取当前所有channel实例的信息
    async fn get_all_channels(&self) -> Result<Vec<ChannelInfo>, ChannelError>;

    /// 处理来自agent的消息（通过WSS）
    async fn handle_agent_message(&self, message: AgentMessage) -> Result<(), ChannelError>;

    /// 处理agent的附件下载请求
    async fn handle_attachment_download(&self, key: &str) -> Result<Vec<u8>, ChannelError>;

    /// 消息入队：外部消息或agent消息进入消息队列
    async fn enqueue_message(&self, record: MessageRecord) -> Result<(), ChannelError>;

    /// 处理消息队列：将队列中的消息依次推送给agent和memory-store
    async fn process_message_queue(&self) -> Result<(), ChannelError>;

    /// 启动WSS Server
    async fn start(&self) -> Result<(), ChannelError>;

    /// 停止所有组件
    async fn stop(&self) -> Result<(), ChannelError>;
}
```

**职责：**
- **构造时绑定Messenger**：一个ChannelManager对应一个Messenger
- **Channel管理**：通过Messenger创建Channel实例；根据channel_id查找Channel实例；维护Channel实例的全局索引；提供`get_all_channels`方法
- **Agent绑定**：处理agent的bind请求（指定多个user_id），由Messenger决定每个user的group列表，为每个(user,group)组合创建Channel实例，注册回调，返回绑定结果
- **注册回调**：向Messenger注册`on_message_received`和`on_group_change`回调
- **消息队列**：维护消息队列暂存待推送的消息，依次推送给agent（WSS）和memory-store
- **WSS Server管理**：持有WSS Server实例；管理agent连接的生命周期
- **消息路由**：
  - 外部消息处理链路：Messenger收到外部消息 → 调用`on_message_received`回调（ChannelManager注册）→ 回调将消息入队 → 处理队列：推送memory-store、通过WSS发给agent
  - agent消息处理链路：WSS收到agent消息 → ChannelManager → 消息入队 → 处理队列：推送memory-store、调用channel.send_message发到外部系统
- **Group变化事件**：Messenger检测到user加入或离开group → 调用`on_group_change`回调 → ChannelManager通过WSS通知agent
- **附件下载**：收到agent的附件下载请求，调用Messenger的`get_attachment_data`获取附件数据并返回

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
消息队列中的记录，用于暂存待推送给agent或memory-store的消息。
```rust
struct MessageRecord {
    record_id: String,
    channel_id: String,
    direction: MessageDirection,
    sender_type: SenderType,
    sender_id: String,
    content: String,          // 文本全文，或非文本内容的反查key
    attachment_keys: Vec<String>,  // 附件key列表（附件数据已由Messenger/Channel保存）
    think_key: Option<String>,
    tool_call_keys: Vec<String>,
    timestamp: String,
    sequence: u64,
}

enum MessageDirection {
    Incoming,   // 从外部系统接收的消息
    Outgoing,   // 发送到外部系统的消息
}

enum SenderType {
    User,       // 外部用户
    Agent,      // agent自己
}
```

### OutgoingMessage（发送到外部系统的消息）
```rust
struct OutgoingMessage {
    channel_id: String,
    target_user_id: Option<String>,  // None表示广播给group中所有可见成员
    content: String,
    attachments: Vec<Attachment>,    // 附件原始数据（由agent发送时携带）
    think_key: Option<String>,
    tool_call_keys: Vec<String>,
}
```

### Attachment（附件原始数据）
```rust
struct Attachment {
    filename: String,
    mime_type: String,
    size_bytes: u64,
    data: Vec<u8>,
}
```

### AttachmentRef（附件引用 - 存储后的引用信息）
```rust
struct AttachmentRef {
    key: String,
    filename: String,
    mime_type: String,
    size_bytes: u64,
    storage_path: String,
}
```

### ChannelStatus
```rust
struct ChannelStatus {
    channel_id: String,
    messenger_id: String,
    group_id: String,
    user_id: String,
    agent_id: String,
    is_running: bool,
    last_message_time: Option<String>,
    uptime_seconds: u64,
}
```

### ChannelInfo（用于返回给agent的channel信息）
```rust
struct ChannelInfo {
    channel_id: String,
    messenger_id: String,
    group_id: String,
    group_name: String,
    user_id: String,
}
```

## 消息流程

### 外部系统 → Agent 流程
```
1.  ```
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
- agent的思考内容key和工具调用key对channel来说是普通文本内容，直接放入消息记录的content字段
- agent将思考内容全文和工具调用详细记录直接发送至memory-store，channel只需处理key的流转

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

消息队列中的每条MessageRecord记录推送至memory-store，包含：

1. **channel内的文本**：content字段（文本全文，或非文本内容的反查key）
2. **附件key列表**：attachment_keys字段
3. **思考内容key**：think_key字段（agent通过WSS发来的key，对channel透明）
4. **工具调用key列表**：tool_call_keys字段（agent通过WSS发来的key，对channel透明）

### 记忆结构

根据记录的类型，每条MessageRecord在memory-store中形成结构化存储：
1. 消息文本记录：包括channel-id、方向、发送者类型、发送者ID、时间、序号、原文（非文本内容为反查key）
2. 思考内容原文：通过key关联，原文由agent直接发送至memory-store
3. 工具调用记录：通过key关联，工具的name、parameter、返回信息由agent直接发送至memory-store

### MemoryStoreClient接口

```rust
trait MemoryStoreClient: Send + Sync + 'static {
    /// 推送单条消息记录到记忆系统
    async fn push_message_record(&self, record: MessageRecord) -> Result<(), MemoryError>;

    /// 批量推送消息记录
    async fn push_message_records(&self, records: Vec<MessageRecord>) -> Result<(), MemoryError>;
}
```

## WSS消息协议

每个agent对应唯一的WSS连接，所有channel实例共享此连接与agent通信。
消息中通过`channel_id`字段区分消息属于哪个channel实例。

### Agent → Channel消息
agent发送消息时，attachments中携带附件原始数据（base64编码）。
```json
{
    "type": "message",
    "data": {
        "channel_id": "qq:交流群:id123456",
        "target_user_id": "user-abc",
        "content": "你好！",
        "attachments": [
            {
                "filename": "image.png",
                "mime_type": "image/png",
                "size_bytes": 102400,
                "data": "<base64编码的附件数据>"
            }
        ],
        "think_key": "think-20240101-123456",
        "tool_call_keys": ["tool-20240101-789012"]
    }
}
```

### Channel → Agent消息
```json
{
    "type": "user_message",
    "data": {
        "channel_id": "qq:交流群:id123456",
        "user_id": "user-abc",
        "user_name": "张三",
        "content": "你好！",
        "attachments": [
            {
                "key": "qq:交流群:id123456-20240101-abc123",
                "filename": "image.png",
                "mime_type": "image/png",
                "size_bytes": 102400
            }
        ],
        "timestamp": "2024-01-01 12:00:00",
        "sequence": 1
    }
}
```

### Agent绑定消息
agent接入时，指定messenger_id和多个user_id，由messenger决定每个user的group列表。
```json
{
    "type": "bind",
    "data": {
        "agent_id": "agent-001",
        "messenger_id": "qq",
        "user_ids": ["id123456", "id789012"]
    }
}
```

### 绑定确认
```json
{
    "type": "bind_ack",
    "data": {
        "agent_id": "agent-001",
        "channels": [
            {
                "channel_id": "qq:交流群:id123456",
                "messenger_id": "qq",
                "group_id": "交流群",
                "group_name": "交流群",
                "user_id": "id123456"
            },
            {
                "channel_id": "qq:工作群:id123456",
                "messenger_id": "qq",
                "group_id": "工作群",
                "group_name": "工作群",
                "user_id": "id123456"
            },
            {
                "channel_id": "qq:项目群:id789012",
                "messenger_id": "qq",
                "group_id": "项目群",
                "group_name": "项目群",
                "user_id": "id789012"
            }
        ]
    }
}
```

### 获取全部channel
```json
{
    "type": "get_channels"
}
```

```json
{
    "type": "channels",
    "data": {
        "channels": [
            {
                "channel_id": "qq:交流群:id123456",
                "messenger_id": "qq",
                "group_id": "交流群",
                "group_name": "交流群",
                "user_id": "id123456"
            }
        ]
    }
}
```

### Group变化通知
```json
{
    "type": "group_change",
    "data": {
        "messenger_id": "qq",
        "user_id": "id123456",
        "group_id": "新群组",
        "group_name": "新群组",
        "change_type": "joined",
        "timestamp": "2024-01-01 12:00:00"
    }
}
```

### Agent附件下载请求
```json
{
    "type": "attachment_download",
    "data": {
        "key": "qq:交流群:id123456-20240101-abc123"
    }
}
```

### 附件下载响应
```json
{
    "type": "attachment_data",
    "data": {
        "key": "qq:交流群:id123456-20240101-abc123",
        "filename": "image.png",
        "mime_type": "image/png",
        "data": "<base64编码的附件数据>"
    }
}
```

### 心跳消息
```json
{
    "type": "ping"
}
```

```json
{
    "type": "pong"
}
```

## 实现决策

### 框架实现
- 定义Messenger trait，便于实现不同的通讯应用接入
- 定义Channel trait，表示一个（messenger, group, user）组合的消息通道
- **ChannelManager作为核心统筹层**，以Messenger为构造入参，一个ChannelManager对应一个Messenger
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
- ChannelManager以Messenger为构造入参，一个ChannelManager对应一个Messenger
- ChannelManager调用Messenger的抽象函数管理通讯应用接入、回调注册和附件操作
- ChannelManager调用Channel的抽象函数管理消息发送
- ChannelManager持有WSS Server、MemoryStoreClient，维护消息队列协调消息推送
- ChannelManager处理agent绑定（传多个user_id，group由Messenger决定）
- ChannelManager向agent传递group变化事件，提供获取全部channel的方法

## 开发计划

### 第1阶段：基础结构搭建
- [ ] 配置Cargo.toml，添加依赖
- [ ] 定义Messenger trait（含回调注册接口和附件管理接口）
- [ ] 定义Channel trait
- [ ] 定义核心数据结构（MessageRecord, OutgoingMessage, Attachment等）
- [ ] 定义错误类型和事件类型

### 第2阶段：核心组件实现
- [ ] 实现ChannelManager（以Messenger为构造入参，消息队列，回调注册，agent绑定，group变化通知）
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