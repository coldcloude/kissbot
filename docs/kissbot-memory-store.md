# kissbot-memory-store 模块设计

## 模块概述
记忆存储模块，负责收集和存储agent接收和产生的所有原始消息数据，作为一切agent活动的完整记录。

## 职责
- 收集agent和channel的全部原始消息
- 按统一方式管理三种agent模式下的记忆
- 将记忆结构化存储为三个记忆存储文件
- 提供HTTPS API接口供其他模块查询和推送记忆
- 当有新数据时，通过HTTPS通知注册的memory-struct模块
- 调用memory基础模块的DirectoryManager进行目录管理

## 记忆来源

### 1. Channel消息
- 包含channel中的全部文本内容
- 非文本内容（图片、音频、视频、二进制文件等）以附件形式存储在channel本地
- 记忆中仅存储用于反查非文本内容的key
- 由channel发送给记忆系统

### 2. 大模型输出
- **思考内容**：全文发送至记忆系统单独存储，仅发送用于反查的key至channel
- **工具调用指令**：tool call的name和parameter全部发送至记忆系统，仅发送用于反查的key至channel
- **回复文本**：全文发送至channel，由channel发送至记忆系统
- **生成的非文本内容**：全文发送至channel，由channel发送至记忆系统

### 3. 工具输出
- 仅包含工具API直接返回的内容，不包含副产物（如工具写入的文件等）
- 全文发送至记忆系统，将摘要信息和调用指令的key发送至channel
- 特殊情况：由记忆工具输出的内容不再发送记忆系统
- 工具输出没有独立的key，应和工具调用指令（含key）一并存储

## 记忆存储方式

### 存储组织
- 记忆不按照channel区分，所有channel的记忆按时间顺序混合存储
- **问答模式**：每个问答会话存一个记忆
- **工程模式**：切分多个会话时，每个会话一个记忆
- **自主模式**：每个agent-id一个记忆
- 实现上，一个记忆按日期拆分多个文件或目录存储

### 结构化存储文件
根据记忆来源，每个记忆形成3个记忆存储文件，文件内按时间顺序存储一条条记录：

#### 1. channel文本记录文件
存储channel内的文本内容，每条记录包含：
- `channel_id`：channel标识
- `user_id`：用户标识
- `is_agent_self`：是否为agent自己
- `timestamp`：时间戳（格式：yyyy-MM-dd HH:mm:ss）
- `sequence`：序号
- `content`：原文（非文本内容或思考内容为反查用的key）

#### 2. 思考内容记录文件
存储思考内容原文，每条记录包含：
- `content`：原文
- `key`：反查用的key
- `timestamp`：时间戳

#### 3. 工具调用记录文件
存储工具调用和返回信息，每条记录包含：
- `tool_name`：工具名称
- `tool_params`：工具参数
- `tool_result`：工具返回结果
- `key`：反查用的key
- `timestamp`：时间戳

## 文件存储目录结构
根据记忆系统的文件存储目录设计：

```
记忆系统根目录
├── {agent-id}/
│   ├── agent-{agent-id}                    # agent存在标识文件（由memory模块管理）
│   ├── metadata.json                       # agent元数据JSON文件
│   ├── memory-ego/                         # memory-ego模块的设定信息
│   ├── memory-store/                       # memory-store收集的原始记忆
│   │   ├── {date}/                         # 按日期分组（格式：yyyy-MM-dd）
│   │   │   ├── channel-records.jsonl       # channel文本记录（JSON Lines格式）
│   │   │   ├── thinking-records.jsonl      # 思考内容记录（JSON Lines格式）
│   │   │   └── tool-records.jsonl          # 工具调用记录（JSON Lines格式）
│   │   └── ...
│   └── memory-struct-*/                    # memory-struct-*实现产生的数据
└── ...
```

## 核心组件设计

### 1. RecordManager - 记录管理器
- 负责管理三种类型记录的存储和读取
- 按日期自动创建目录和文件
- 提供追加记录、按时间范围查询等功能
- 支持JSON Lines格式的高效读写

### 2. KeyGenerator - Key生成器
- 生成唯一的key用于反查
- key格式：`{type}-{timestamp}-{uuid}`（type: thinking/tool）

### 3. SubscriptionManager - 订阅管理器
- 管理memory-struct模块的订阅
- 当有新数据时，通过HTTPS通知所有订阅者
- 支持订阅注册、取消、心跳检测

### 4. HTTPS API服务器
- 提供记忆推送API
- 提供记忆查询API
- 提供订阅管理API

## API设计

### 记忆推送API

#### 推送Channel文本记录
- **路径**：POST /store/channel-record
- **输入**：ChannelRecord
- **输出**：ApiResponse&lt;()&gt;

#### 推送思考内容记录
- **路径**：POST /store/thinking-record
- **输入**：ThinkingRecord
- **输出**：ApiResponse&lt;String&gt;（返回生成的key）

#### 推送工具调用记录
- **路径**：POST /store/tool-record
- **输入**：ToolRecord
- **输出**：ApiResponse&lt;String&gt;（返回生成的key）

### 记忆查询API

#### 查询Channel文本记录
- **路径**：POST /store/query-channel
- **输入**：QueryChannelRequest（包含时间范围、channel_id等过滤条件）
- **输出**：ApiResponse&lt;Vec&lt;ChannelRecord&gt;&gt;

#### 查询思考内容记录
- **路径**：POST /store/query-thinking
- **输入**：QueryThinkingRequest（包含key、时间范围等过滤条件）
- **输出**：ApiResponse&lt;Vec&lt;ThinkingRecord&gt;&gt;

#### 查询工具调用记录
- **路径**：POST /store/query-tool
- **输入**：QueryToolRequest（包含key、时间范围等过滤条件）
- **输出**：ApiResponse&lt;Vec&lt;ToolRecord&gt;&gt;

### 订阅管理API

#### 注册订阅
- **路径**：POST /store/subscribe
- **输入**：SubscribeRequest（包含回调URL、订阅类型）
- **输出**：ApiResponse&lt;()&gt;

#### 取消订阅
- **路径**：POST /store/unsubscribe
- **输入**：UnsubscribeRequest（包含回调URL）
- **输出**：ApiResponse&lt;()&gt;

#### 列出订阅
- **路径**：GET /store/subscriptions
- **输入**：无
- **输出**：ApiResponse&lt;Vec&lt;Subscription&gt;&gt;

## 数据结构定义

### ChannelRecord
```rust
struct ChannelRecord {
    channel_id: String,
    user_id: String,
    is_agent_self: bool,
    timestamp: String,      // yyyy-MM-dd HH:mm:ss
    sequence: u64,
    content: String,
}
```

### ThinkingRecord
```rust
struct ThinkingRecord {
    content: String,
    key: String,
    timestamp: String,      // yyyy-MM-dd HH:mm:ss
}
```

### ToolRecord
```rust
struct ToolRecord {
    tool_name: String,
    tool_params: serde_json::Value,
    tool_result: serde_json::Value,
    key: String,
    timestamp: String,      // yyyy-MM-dd HH:mm:ss
}
```

### QueryChannelRequest
```rust
struct QueryChannelRequest {
    agent_id: String,
    start_time: Option&lt;String&gt;,    // yyyy-MM-dd HH:mm:ss
    end_time: Option&lt;String&gt;,      // yyyy-MM-dd HH:mm:ss
    channel_ids: Option&lt;Vec&lt;String&gt;&gt;,
    limit: Option&lt;usize&gt;,
}
```

### QueryThinkingRequest
```rust
struct QueryThinkingRequest {
    agent_id: String,
    keys: Option&lt;Vec&lt;String&gt;&gt;,
    start_time: Option&lt;String&gt;,
    end_time: Option&lt;String&gt;,
    limit: Option&lt;usize&gt;,
}
```

### QueryToolRequest
```rust
struct QueryToolRequest {
    agent_id: String,
    keys: Option&lt;Vec&lt;String&gt;&gt;,
    start_time: Option&lt;String&gt;,
    end_time: Option&lt;String&gt;,
    limit: Option&lt;usize&gt;,
}
```

### SubscribeRequest
```rust
struct SubscribeRequest {
    callback_url: String,
    subscribe_types: Vec&lt;SubscribeType&gt;,  // Channel/Thinking/Tool
}
```

### Subscription
```rust
struct Subscription {
    callback_url: String,
    subscribe_types: Vec&lt;SubscribeType&gt;,
    registered_at: String,
    last_heartbeat: Option&lt;String&gt;,
}
```

## 通信接口
- **输入**：通过HTTPS API接收agent和channel推送的记忆
- **输出**：通过HTTPS API返回查询结果，通过HTTPS通知订阅者
- **文件系统**：与其他记忆模块共享文件系统，调用DirectoryManager进行目录管理

## 实现决策
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 使用serde进行JSON序列化
- 使用JSON Lines格式存储记录，便于追加和流式读取
- 使用DashMap实现高并发数据结构
- 支持从配置文件加载证书
- 单例通过关联函数获取
- 目录自动创建：当需要使用某个日期目录时自动创建

## 开发计划

### 第1阶段：基础结构搭建
- [ ] 配置Cargo.toml，添加依赖
- [ ] 调用memory基础模块库（DirectoryManager）
- [ ] 定义模块结构
- [ ] 定义错误类型

### 第2阶段：记录管理实现
- [ ] 实现RecordManager
- [ ] 按日期自动创建目录和文件
- [ ] 实现JSON Lines格式读写
- [ ] 实现追加记录功能
- [ ] 实现按时间范围查询功能

### 第3阶段：Key生成和订阅管理
- [ ] 实现KeyGenerator
- [ ] 实现SubscriptionManager
- [ ] 实现订阅注册和取消
- [ ] 实现新数据通知机制

### 第4阶段：API实现
- [ ] 实现记忆推送API
- [ ] 实现记忆查询API
- [ ] 实现订阅管理API
- [ ] HTTPS服务器启动

### 第5阶段：测试和完善
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化
