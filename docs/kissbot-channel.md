# kissbot-channel 模块设计

## 模块概述
消息通道框架，连接外部系统（如web界面、QQ、Matrix等），向agent输入消息，并接收agent的输出消息。

## 职责
- 定义channel框架的trait，便于实现不同的channel（web、QQ、Matrix等）
- 管理多个channel实例
- 处理来自外部系统的消息
- 向外部系统发送消息
- 与agent通过WSS通信
- 将消息推送到memory-store
- 管理非文本内容（图片、音频、视频、二进制文件等）的本地存储
- 为非文本内容生成反查用的key

## 架构设计

### 核心架构
```
┌─────────────────────────────────────────────────────────┐
│                    kissbot-channel                      │
├─────────────────────────────────────────────────────────┤
│  ┌──────────────────┐     ┌──────────────────┐        │
│  │ Channel Manager  │     │ Attachment Store │        │
│  └────────┬─────────┘     └────────┬─────────┘        │
│           │                         │                  │
│  ┌────────▼─────────┐     ┌────────▼─────────┐        │
│  │   WSS Server     │     │ HTTPS API Server │        │
│  └────────┬─────────┘     └────────┬─────────┘        │
│           │                         │                  │
└───────────┼─────────────────────────┼──────────────────┘
            │                         │
    ┌───────▼────────┐      ┌─────────▼────────┐
    │   Agent(WSS)   │      │ External Systems │
    └────────────────┘      └──────────────────┘
```

## Channel Trait设计

### Channel trait
```rust
trait Channel: Send + Sync + 'static {
    /// 获取channel的唯一标识
    fn channel_id(&self) -> &str;

    /// 启动channel，开始接收和处理消息
    async fn start(&self, event_sender: EventSender) -> Result<(), ChannelError>;

    /// 停止channel
    async fn stop(&self) -> Result<(), ChannelError>;

    /// 发送消息到外部系统
    async fn send_message(&self, message: OutgoingMessage) -> Result<(), ChannelError>;

    /// 获取channel状态
    async fn get_status(&self) -> Result<ChannelStatus, ChannelError>;
}
```

### 事件类型
```rust
enum ChannelEvent {
    /// 接收到用户消息
    UserMessage {
        user_id: String,
        content: String,
        attachments: Vec<AttachmentRef>,
        timestamp: String,
    },
    /// 用户加入
    UserJoin {
        user_id: String,
        user_name: String,
        timestamp: String,
    },
    /// 用户离开
    UserLeave {
        user_id: String,
        timestamp: String,
    },
    /// 错误
    Error {
        error: ChannelError,
        timestamp: String,
    },
}
```

## 核心组件设计

### 1. ChannelManager - Channel管理器
- 管理多个channel实例
- 注册、启动、停止channel
- 分发channel事件
- 维护channel状态

### 2. AttachmentStore - 附件存储管理器
- 管理非文本内容的本地存储
- 为附件生成唯一的key
- 存储附件元数据
- 通过key检索附件
- 清理过期附件

### 3. WSSServer - WSS服务器
- 作为WSS服务器，等待agent连接
- 接收来自agent的消息
- 向agent发送消息
- 管理多个agent连接
- 心跳检测和连接重连

### 4. HTTPSAPIServer - HTTPS API服务器
- 提供channel管理API
- 提供消息查询API
- 提供附件上传和下载API
- 提供状态查询API

### 5. MessageRouter - 消息路由器
- 接收来自channel的消息
- 将消息推送到memory-store
- 将消息转发到agent（通过WSS）
- 接收来自agent的消息
- 将消息推送到memory-store
- 将消息转发到相应的channel

## 数据结构定义

### IncomingMessage（来自外部系统的消息）
```rust
struct IncomingMessage {
    channel_id: String,
    user_id: String,
    user_name: Option<String>,
    content: String,
    attachments: Vec<Attachment>,
    timestamp: String,
    sequence: u64,
}
```

### OutgoingMessage（发送到外部系统的消息）
```rust
struct OutgoingMessage {
    channel_id: String,
    target_user_id: Option<String>,  // None表示广播
    content: String,
    attachments: Vec<AttachmentRef>,
    thinking_key: Option<String>,
    tool_call_keys: Vec<String>,
}
```

### Attachment（附件）
```rust
struct Attachment {
    key: String,
    filename: String,
    mime_type: String,
    size_bytes: u64,
    data: Vec<u8>,
}
```

### AttachmentRef（附件引用）
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
    is_running: bool,
    connected_users: usize,
    last_message_time: Option<String>,
    uptime_seconds: u64,
}
```

## 消息流程

### 外部系统 → Agent 流程
```
1. 外部系统发送消息到channel
   ↓
2. channel接收消息，调用Channel trait的事件
   ↓
3. ChannelManager接收事件，转给MessageRouter
   ↓
4. MessageRouter处理附件（如果有）：
   ├─ 保存附件到AttachmentStore
   ├─ 生成附件key
   └─ 构建AttachmentRef
   ↓
5. MessageRouter将消息推送到memory-store（HTTPS）
   ↓
6. MessageRouter将消息发送到agent（WSS）
   ↓
7. Agent接收并处理消息
```

### Agent → 外部系统 流程
```
1. Agent发送消息到channel（WSS）
   ↓
2. WSSServer接收消息，转给MessageRouter
   ↓
3. MessageRouter将消息推送到memory-store（HTTPS）
   ↓
4. MessageRouter查找目标channel
   ↓
5. MessageRouter调用channel的send_message
   ↓
6. channel将消息发送到外部系统
```

## 附件存储设计

### 附件存储目录结构
```
channel-data/
├── {channel-id}/
│   ├── attachments/
│   │   ├── {date}/                    # 按日期分组（格式：yyyy-MM-dd）
│   │   │   ├── {key}.{ext}           # 附件文件
│   │   │   └── ...
│   │   └── ...
│   └── attachment-metadata.jsonl      # 附件元数据（JSON Lines格式）
└── ...
```

### 附件Key格式
```
{channel-id}-{timestamp}-{uuid}
```

### 附件元数据
```rust
struct AttachmentMetadata {
    key: String,
    filename: String,
    mime_type: String,
    size_bytes: u64,
    storage_path: String,
    uploaded_at: String,
    message_sequence: u64,
    user_id: String,
}
```

## API设计

### Channel管理API

#### 注册Channel
- **路径**：POST /channel/register
- **输入**：RegisterChannelRequest
- **输出**：ApiResponse&lt;()&gt;

#### 启动Channel
- **路径**：POST /channel/start
- **输入**：ChannelIdRequest
- **输出**：ApiResponse&lt;()&gt;

#### 停止Channel
- **路径**：POST /channel/stop
- **输入**：ChannelIdRequest
- **输出**：ApiResponse&lt;()&gt;

#### 列出Channel
- **路径**：GET /channel/list
- **输入**：无
- **输出**：ApiResponse&lt;Vec&lt;ChannelStatus&gt;&gt;

#### 获取Channel状态
- **路径**：POST /channel/status
- **输入**：ChannelIdRequest
- **输出**：ApiResponse&lt;ChannelStatus&gt;

### 消息查询API

#### 查询消息
- **路径**：POST /channel/query-messages
- **输入**：QueryMessagesRequest
- **输出**：ApiResponse&lt;Vec&lt;MessageRecord&gt;&gt;

### 附件API

#### 上传附件
- **路径**：POST /channel/attachment/upload
- **输入**：multipart/form-data
- **输出**：ApiResponse&lt;AttachmentRef&gt;

#### 下载附件
- **路径**：GET /channel/attachment/download
- **输入**：AttachmentKeyRequest（query参数）
- **输出**：文件流

#### 获取附件信息
- **路径**：POST /channel/attachment/info
- **输入**：AttachmentKeyRequest
- **输出**：ApiResponse&lt;AttachmentMetadata&gt;

### 数据结构

#### RegisterChannelRequest
```rust
struct RegisterChannelRequest {
    channel_id: String,
    channel_type: String,  // "web", "qq", "matrix"等
    config: serde_json::Value,
}
```

#### ChannelIdRequest
```rust
struct ChannelIdRequest {
    channel_id: String,
}
```

#### QueryMessagesRequest
```rust
struct QueryMessagesRequest {
    channel_id: String,
    start_time: Option&lt;String&gt;,
    end_time: Option&lt;String&gt;,
    user_ids: Option&lt;Vec&lt;String&gt;&gt;,
    limit: Option&lt;usize&gt;,
}
```

#### MessageRecord
```rust
struct MessageRecord {
    channel_id: String,
    user_id: String,
    is_agent: bool,
    content: String,
    attachment_keys: Vec&lt;String&gt;,
    timestamp: String,
    sequence: u64,
}
```

#### AttachmentKeyRequest
```rust
struct AttachmentKeyRequest {
    key: String,
}
```

## WSS消息协议

### Agent → Channel消息
```json
{
    "type": "message",
    "data": {
        "channel_id": "web-1",
        "target_user_id": "user-123",
        "content": "你好！",
        "attachments": [
            {
                "key": "web-1-20240101-abc123",
                "filename": "image.png",
                "mime_type": "image/png",
                "size_bytes": 102400,
                "storage_path": "/path/to/file"
            }
        ],
        "thinking_key": "thinking-20240101-123456",
        "tool_call_keys": ["tool-20240101-789012"]
    }
}
```

### Channel → Agent消息
```json
{
    "type": "user_message",
    "data": {
        "channel_id": "web-1",
        "user_id": "user-123",
        "user_name": "张三",
        "content": "你好！",
        "attachments": [
            {
                "key": "web-1-20240101-abc123",
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
- 定义Channel trait，便于实现不同的channel
- 提供channel-web作为参考实现
- 使用tokio作为异步运行时
- 使用tokio-tungstenite实现WSS服务器
- 使用axum实现HTTPS API服务器
- 使用serde进行JSON序列化
- 使用dashmap实现高并发数据结构
- 使用futures实现异步任务
- 支持从配置文件加载证书
- 单例通过关联函数获取

### 附件存储实现
- 使用文件系统存储附件
- 使用JSON Lines格式存储附件元数据
- 按日期分组存储，便于管理和清理
- 支持配置附件过期时间和自动清理
- 为附件生成唯一的key

## 开发计划

### 第1阶段：基础结构搭建
- [ ] 配置Cargo.toml，添加依赖
- [ ] 定义Channel trait
- [ ] 定义核心数据结构
- [ ] 定义错误类型
- [ ] 定义事件类型

### 第2阶段：核心组件实现
- [ ] 实现ChannelManager
- [ ] 实现MessageRouter
- [ ] 实现WSS服务器
- [ ] 实现HTTPS API服务器基础

### 第3阶段：附件存储实现
- [ ] 实现AttachmentStore
- [ ] 实现附件上传和下载
- [ ] 实现附件元数据管理
- [ ] 实现附件过期清理

### 第4阶段：API实现
- [ ] 实现channel管理API
- [ ] 实现消息查询API
- [ ] 实现附件API
- [ ] 完整HTTPS API服务器

### 第5阶段：Channel Web实现
- [ ] 实现channel-web参考实现
- [ ] 实现与前端的通信
- [ ] 实现消息收发

### 第6阶段：与memory-store集成
- [ ] 实现消息推送到memory-store
- [ ] 实现与kissbot-memory-store的HTTPS通信

### 第7阶段：测试和完善
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化
