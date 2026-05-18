# kissbot-channel 模块设计

## 模块概述
消息通道框架，抽象出messenger（通讯应用）、group（群组）、user（用户）三层概念，连接外部系统（如web界面、QQ、Matrix等），向agent输入消息，并接收agent的输出消息。

- **Messenger**：表示一个通讯应用（如QQ、Web、Matrix），每个kissbot-channel-xxx模块都是对一个Messenger trait的实现。Messenger负责管理可用的user和group，以及为agent创建channel实例。
- **Group**：表示一个群组/会话，是消息的组织单位。一个group中有多个user，可以接收到group中所有user发的消息。
- **User**：表示一个通讯应用中的用户账号。一个agent在一个messenger中可以绑定多个user，一个user在一个messenger中可以收发多个group的消息。

**Channel实例**：一个agent绑定的一个user加入的一个group，构成一个消息收发单元，即一个channel实例。channel-id唯一标识一个messenger+group+user的组合，例如`channel_id = "qq:交流群:id123456"`。

**ChannelManager**：模块的核心统筹协调层，管理Messenger实例和Channel实例的生命周期，协调WSS连接与Channel之间的消息路由，驱动MemoryStoreClient从Channel消息列表中统一推送记忆。

## 职责
- 定义Messenger trait，便于实现不同的通讯应用接入（web、QQ、Matrix等）
- 定义Channel trait，表示messenger+group+user组合的消息收发通道
- **ChannelManager统筹管理**：
  - 管理Messenger实例的注册、启动和停止
  - 管理Channel实例的创建和查询
  - 管理WSS Server（每个agent一个连接）
  - 协调WSS连接与Channel实例之间的消息路由
  - 驱动MemoryStoreClient从Channel消息列表统一推送至memory-store
  - 统一协调AttachmentStore进行附件存储
- Messenger负责维护user和group信息，为agent创建channel实例
- 处理来自外部系统的消息
- 向外部系统发送消息
- 维护每个channel实例的消息列表（同时包含接收和发送的消息）
- 管理非文本内容（图片、音频、视频、二进制文件等）的本地存储
- 为非文本内容生成反查用的key

## 架构设计

### 核心架构
```
┌──────────────────────────────────────────────────────────────────────┐
│                          kissbot-channel                             │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │                      ChannelManager                          │    │
│  │  ┌─────────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  │    │
│  │  │ Messenger   │  │ WSS      │  │MemoryStore│  │Attachment│  │    │
│  │  │ 管理        │  │ Server   │  │Client    │  │Store     │  │    │
│  │  └──────┬──────┘  │ 管理     │  │ 管理     │  │ 管理     │  │    │
│  │         │         └──────────┘  └──────────┘  └──────────┘  │    │
│  └─────────┼────────────────────────────────────────────────────┘    │
│            │                                                          │
│  ┌─────────▼──────────────────────────────────────────────────────┐  │
│  │  ┌──────────────────┐     ┌──────────────────┐                │  │
│  │  │  Messenger A     │     │  Messenger B     │   ...          │  │
│  │  │  (如QQ)          │     │  (如Web)         │                │  │
│  │  └────────┬─────────┘     └────────┬─────────┘                │  │
│  │           │                        │                           │  │
│  │  ┌────────▼────────┐     ┌────────▼────────┐                 │  │
│  │  │ ChannelInstance │     │ ChannelInstance │   ...            │  │
│  │  │ (qq:交流群:     │     │ (web:首页:     │                 │  │
│  │  │  id123456)      │     │  admin)        │                 │  │
│  │  │ ┌──────────────┐│     │ ┌──────────────┐│                 │  │
│  │  │ │ MessageList  ││     │ │ MessageList  ││                 │  │
│  │  │ └──────────────┘│     │ └──────────────┘│                 │  │
│  │  └─────────────────┘     └─────────────────┘                 │  │
│  └──────────────────────────────────────────────────────────────┘  │
│                                                                      │
└─────────────────────────────────────────┬────────────────────────────┘
                                          │
                                  ┌───────▼────────┐
                                  │   Agent(WSS)   │
                                  └────────────────┘

ChannelManager调用Messenger和Channel的抽象函数完成各项功能的协调。
各具体Messenger实现自行决定与外部系统的通信方式（如HTTP API、WebSocket、平台SDK等）
```

## Messenger Trait设计

### Messenger trait
```rust
trait Messenger: Send + Sync + 'static {
    /// 获取messenger的唯一标识（如 "qq", "web", "matrix"）
    fn messenger_id(&self) -> &str;

    /// 获取该messenger中可用的用户列表
    async fn get_available_users(&self) -> Result<Vec<UserInfo>, ChannelError>;

    /// 获取指定用户可用的群组列表
    async fn get_user_groups(&self, user_id: &str) -> Result<Vec<GroupInfo>, ChannelError>;

    /// 为指定的agent、user、group创建channel实例
    async fn create_channel(
        &self,
        agent_id: &str,
        user_id: &str,
        group_id: &str,
    ) -> Result<Arc<dyn Channel>, ChannelError>;

    /// 获取该messenger下所有的channel实例
    async fn get_channels(&self) -> Result<Vec<Arc<dyn Channel>>, ChannelError>;

    /// 启动messenger，开始接收外部消息并回调ChannelManager
    async fn start(&self, event_callback: MessengerEventCallback) -> Result<(), ChannelError>;

    /// 停止messenger
    async fn stop(&self) -> Result<(), ChannelError>;
}

/// Messenger接收到外部消息后的回调，由ChannelManager提供
/// Messenger调用此回调将外部消息交给ChannelManager处理
type MessengerEventCallback = Arc<dyn Fn(MessengerEvent) -> Result<(), ChannelError> + Send + Sync>;

enum MessengerEvent {
    /// 收到外部用户消息
    UserMessage {
        messenger_id: String,
        channel_id: String,
        user_id: String,
        user_name: Option<String>,
        content: String,
        attachments: Vec<Attachment>,
        timestamp: String,
        sequence: u64,
    },
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
```

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

    /// 发送消息到外部系统（由ChannelManager调用）
    async fn send_message(&self, message: OutgoingMessage) -> Result<(), ChannelError>;

    /// 获取消息列表（由ChannelManager驱动MemoryStoreClient调用）
    async fn get_message_list(&self) -> Result<Vec<MessageRecord>, ChannelError>;

    /// 将消息追加到消息列表（由ChannelManager调用）
    async fn append_message(&self, record: MessageRecord) -> Result<(), ChannelError>;

    /// 获取channel状态
    async fn get_status(&self) -> Result<ChannelStatus, ChannelError>;
}
```

## 核心组件设计

### 1. ChannelManager - 统筹管理器
ChannelManager是kissbot-channel模块的核心统筹层，通过调用Messenger和Channel的抽象函数完成各项功能。

**职责：**
- **Messenger管理**：注册、启动、停止Messenger实例；查询所有可用Messenger
- **Channel管理**：通过Messenger创建Channel实例；根据channel_id查找Channel实例；维护Channel实例的全局索引
- **WSS Server管理**：持有WSS Server实例；管理agent连接的生命周期
- **消息路由**：
  - 外部消息路由：Messenger收到外部消息 → 回调ChannelManager → 保存附件 → 追加到消息列表 → 推送memory-store → 通过WSS发给agent
  - agent消息路由：WSS收到agent消息 → ChannelManager → 保存附件 → 追加到消息列表 → 推送memory-store → 通过Channel.send_message发到外部系统
- **记忆推送**：定期或按事件驱动，调用MemoryStoreClient从Channel消息列表中读取记录并推送
- **Agent绑定**：处理agent的bind请求，通过Messenger创建Channel实例，返回绑定结果
- **生命周期协调**：启动时依次初始化WSS Server、MemoryStoreClient、AttachmentStore，注册Messenger

```rust
struct ChannelManager {
    messengers: HashMap<String, Arc<dyn Messenger>>,
    channels: HashMap<String, Arc<dyn Channel>>,  // channel_id -> Channel
    wss_server: Arc<WssServer>,
    memory_store: Arc<dyn MemoryStoreClient>,
    attachment_store: Arc<AttachmentStore>,
}

impl ChannelManager {
    /// 注册Messenger实例
    async fn register_messenger(&self, messenger: Arc<dyn Messenger>) -> Result<(), ChannelError>;

    /// 处理agent绑定请求：查询Messenger的user/group，创建Channel实例
    async fn handle_agent_bind(&self, agent_id: &str, messenger_id: &str, user_id: &str, group_ids: Vec<String>) -> Result<Vec<ChannelInfo>, ChannelError>;

    /// 处理来自Messenger的外部消息
    async fn handle_external_message(&self, event: MessengerEvent) -> Result<(), ChannelError>;

    /// 处理来自agent的消息（通过WSS）
    async fn handle_agent_message(&self, message: AgentMessage) -> Result<(), ChannelError>;

    /// 启动所有组件
    async fn start(&self) -> Result<(), ChannelError>;

    /// 停止所有组件
    async fn stop(&self) -> Result<(), ChannelError>;
}
```

### 2. Messenger实现 - 通讯应用接入
- 每个具体的Messenger实现（如channel-qq、channel-web）负责：
  - 维护该通讯应用中的user信息和group信息
  - 接收来自对应外部系统的消息
  - 收到外部消息后，通过MessengerEventCallback回调ChannelManager
  - 将来自agent的消息发送到外部系统（通过Channel.send_message的调用链）
- Messenger不直接管理其他Messenger，不同Messenger彼此独立
- Messenger不直接操作WSS、memory-store或附件存储，这些由ChannelManager统一协调

### 3. ChannelInstance - Channel实例
- 每个channel实例对应一个（messenger, group, user）三元组
- 维护一个**消息列表**，收到的外部消息和agent发送的消息统一按时间顺序放入列表
- 消息列表中每条记录标记消息方向（incoming/outgoing）和发送者身份（用户/agent）
- Channel实例的方法由ChannelManager调用，不主动发起操作

### 4. AttachmentStore - 附件存储管理器
- 管理非文本内容的本地存储
- 为附件生成唯一的key
- 存储附件元数据
- 通过key检索附件
- 清理过期附件
- 附件的来源为channel消息（无论是接收的还是发送的），统一存储不做区分

### 5. WSSServer - WSS服务器
- 作为WSS服务器，等待agent连接
- 每个agent对应唯一的WSS连接
- 接收来自agent的消息，交给ChannelManager处理
- 将ChannelManager转发的消息发送给agent
- 管理多个agent连接
- 心跳检测和连接重连

### 6. MemoryStoreClient - 记忆存储通信组件
- 与memory-store通信，将消息推送至记忆系统
- 由ChannelManager驱动，从channel实例的消息列表中统一读取消息并推送
- 不对接agent单独发送

## 数据结构定义

### MessageRecord（消息列表中的记录）
消息列表中无论是接收的消息还是发送的消息，统一使用此结构。
```rust
struct MessageRecord {
    channel_id: String,
    message_id: String,       // 消息在该channel中的唯一ID
    direction: MessageDirection,
    sender_type: SenderType,
    sender_id: String,
    content: String,          // 文本全文，或非文本内容的反查key
    attachments: Vec<AttachmentRef>,
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
    attachments: Vec<AttachmentRef>,
    think_key: Option<String>,
    tool_call_keys: Vec<String>,
}
```

### Attachment（附件 - 来自外部系统的原始数据）
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

## 消息流程

### 外部系统 → Agent 流程
```
1. 外部系统发送消息到messenger（如QQ收到群消息）
   ↓
2. Messenger收到消息，通过MessengerEventCallback回调ChannelManager
   ↓
3. ChannelManager处理MessengerEvent：
   ├─ 解析channel_id、user_id、content、attachments等
   ├─ 如果附件存在，调用AttachmentStore保存附件，生成AttachmentRef
   └─ 构建MessageRecord（direction=Incoming, sender_type=User）
   ↓
4. ChannelManager调用channel.append_message()将记录追加到消息列表
   ↓
5. ChannelManager驱动MemoryStoreClient从消息列表读取记录，推送至memory-store
   ↓
6. ChannelManager通过WSS Server将消息发送给agent
   ↓
7. Agent接收并处理消息
```

### Agent → 外部系统 流程
```
1. Agent生成回复消息，通过WSS发送到kissbot-channel
   ↓
2. WSS Server接收消息，交给ChannelManager处理
   ↓
3. ChannelManager解析消息，按channel_id查找对应的Channel实例
   ↓
4. 如果消息包含附件数据，调用AttachmentStore保存附件，生成AttachmentRef
   ↓
5. ChannelManager构建MessageRecord（direction=Outgoing, sender_type=Agent），
   调用channel.append_message()追加到消息列表
   ↓
6. ChannelManager驱动MemoryStoreClient从消息列表读取记录，推送至memory-store
   ↓
7. ChannelManager调用channel.send_message()，Channel实例通过所属Messenger将消息发送到外部系统
```

### Agent绑定流程
```
1. Agent通过WSS发送bind请求
   ↓
2. WSS Server接收bind消息，交给ChannelManager
   ↓
3. ChannelManager调用messenger.get_available_users()获取可用用户
   ↓
4. ChannelManager调用messenger.get_user_groups()获取用户可用群组
   ↓
5. Agent选择要绑定的user和group
   ↓
6. ChannelManager调用messenger.create_channel()为每个(user,group)组合创建Channel实例
   ↓
7. ChannelManager将Channel实例加入全局索引
   ↓
8. ChannelManager通过WSS返回绑定确认（含所有channel实例的信息）
```

### 说明
- **ChannelManager是消息路由的中央协调者**，所有消息的流转都经过ChannelManager
- **消息列表是所有消息的统一存储**，无论是接收还是发送的消息，都由ChannelManager先追加到消息列表
- **MemoryStoreClient由ChannelManager驱动**，从消息列表中统一读取记录推送至memory-store
- agent的思考内容key和工具调用key对channel来说是普通文本内容，直接放入消息列表的content字段
- agent将思考内容全文和工具调用详细记录直接发送至memory-store，channel只需处理key的流转

## 附件存储设计

### 附件存储目录结构
```
channel-data/
├── attachments/
│   ├── {date}/                    # 按日期分组（格式：yyyy-MM-dd）
│   │   ├── {key}.{ext}           # 附件文件
│   │   └── ...
│   └── ...
└── attachment-metadata.jsonl      # 附件元数据（JSON Lines格式）
```

附件存储不再按channel-id分目录，因为附件本身已在消息列表中通过channel_id关联。按日期平铺存储更便于统一管理和清理。

### 附件Key格式
```
{channel-id}-{timestamp}-{uuid}
```

### 附件元数据
```rust
struct AttachmentMetadata {
    key: String,
    channel_id: String,
    message_id: String,
    direction: MessageDirection,   // 标识是接收还是发送的附件
    filename: String,
    mime_type: String,
    size_bytes: u64,
    storage_path: String,
    uploaded_at: String,
    sequence: u64,
}
```

## 与memory-store通信设计

ChannelManager通过MemoryStoreClient与memory-store通信，统一从channel实例的消息列表中读取记录并推送至记忆系统。
注意：记忆不按channel区分，所有channel的记忆按时间顺序混合存储。

### 推送方式

ChannelManager持有MemoryStoreClient，在消息流转过程中驱动推送：

```
                    ChannelManager
                         │
          ┌──────────────┼──────────────┐
          │              │              │
          ▼              ▼              ▼
   Messenger A    Messenger B    WSS Server
          │              │              │
          │    ┌─────────▼─────────┐    │
          └───►│  ChannelInstance  │◄───┘
               │                   │
               │  ┌─────────────┐  │
               │  │ MessageList │──│── MemoryStoreClient ──→ memory-store
               │  └─────────────┘  │
               └───────────────────┘
```

### 推送内容

消息列表中的每条MessageRecord记录推送至memory-store，包含：

1. **channel内的文本**：content字段（文本全文，或非文本内容的反查key）
2. **附件key列表**：attachments字段中的key
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
```json
{
    "type": "message",
    "data": {
        "channel_id": "qq:交流群:id123456",
        "target_user_id": "user-abc",
        "content": "你好！",
        "attachments": [
            {
                "key": "qq:交流群:id123456-20240101-abc123",
                "filename": "image.png",
                "mime_type": "image/png",
                "size_bytes": 102400,
                "storage_path": "/path/to/file"
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
                "size_bytes": 102400,
                "storage_path": "/path/to/file"
            }
        ],
        "timestamp": "2024-01-01 12:00:00",
        "sequence": 1
    }
}
```

### Agent绑定消息
agent接入时，查询可用messenger和user，选择绑定后建立channel实例。
```json
{
    "type": "bind",
    "data": {
        "agent_id": "agent-001",
        "messenger_id": "qq",
        "user_id": "id123456",
        "group_ids": ["交流群", "工作群"]
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
                "user_id": "id123456"
            },
            {
                "channel_id": "qq:工作群:id123456",
                "messenger_id": "qq",
                "group_id": "工作群",
                "user_id": "id123456"
            }
        ]
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
- **ChannelManager作为核心统筹层**，调用Messenger和Channel的抽象函数协调所有功能
- 提供channel-web作为参考实现（实现Messenger trait）
- 使用tokio作为异步运行时
- 使用tokio-tungstenite实现WSS服务器
- 使用serde进行JSON序列化
- 使用dashmap实现高并发数据结构
- 使用futures实现异步任务
- 支持从配置文件加载证书
- 与memory-store通信使用kissbot-memory-store提供的SDK/API

### 附件存储实现
- 使用文件系统存储附件
- 使用JSON Lines格式存储附件元数据
- 按日期分组存储，便于管理和清理
- 支持配置附件过期时间和自动清理
- 为附件生成唯一的key
- 附件按channel消息关联，不区分发送方和接收方

### Messenger与Channel的关系
- Messenger trait是通讯应用接入的抽象接口
- 每个kissbot-channel-xxx模块是对一个Messenger trait的实现
- Messenger负责管理user和group信息
- 当agent接入时，Messenger为每个（user, group）组合创建Channel实例
- Channel实例维护消息列表，通过ChannelManager统一管理

### ChannelManager的定位
- ChannelManager是模块的入口和核心统筹层，对外提供统一的操作接口
- ChannelManager调用Messenger的抽象函数管理通讯应用接入
- ChannelManager调用Channel的抽象函数管理消息列表和消息发送
- ChannelManager持有WSS Server、MemoryStoreClient、AttachmentStore，协调它们的协作
- Messenger不直接感知WSS、memory-store等组件，通过回调与ChannelManager通信

## 开发计划

### 第1阶段：基础结构搭建
- [ ] 配置Cargo.toml，添加依赖
- [ ] 定义Messenger trait
- [ ] 定义Channel trait
- [ ] 定义核心数据结构（MessageRecord, OutgoingMessage, AttachmentRef等）
- [ ] 定义错误类型和事件类型

### 第2阶段：核心组件实现
- [ ] 实现ChannelManager（Messenger管理、Channel管理、消息路由协调）
- [ ] 实现WSS Server（每agent一个连接）
- [ ] 实现ChannelInstance（消息列表、消息路由）
- [ ] 实现MemoryStoreClient（对接消息列表统一推送）
- [ ] 实现AttachmentStore

### 第3阶段：Messenger基础实现
- [ ] 实现Messenger基础框架（user/group管理、外部消息回调）
- [ ] 实现agent绑定流程（ChannelManager协调）
- [ ] 实现channel实例创建与管理

### 第4阶段：与memory-store集成
- [ ] 实现MemoryStoreClient与kissbot-memory-store的通信
- [ ] 实现ChannelManager驱动消息列表批量推送

### 第5阶段：Channel Web实现
- [ ] 实现channel-web（作为Messenger trait的参考实现）
- [ ] 实现与前端Web界面的通信
- [ ] 实现消息收发

### 第6阶段：测试和完善
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化
