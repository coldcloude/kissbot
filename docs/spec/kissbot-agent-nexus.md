# kissbot-agent-nexus 技术规格

## 配置文件格式

配置文件使用 JSON 格式（由 ConfigManager 管理），存储在 kissbot-agent 的配置目录下。

```json
{
  "agent_id": "my-agent",
  "llm": {
    "provider": "openai",
    "endpoint": "https://api.openai.com/v1",
    "api_key": "sk-...",
    "model": "gpt-4o",
    "max_tokens": 4096,
    "temperature": 0.7,
    "timeout_secs": 60,
    "retry_count": 3
  },
  "current_role": "assistant",
  "current_mode": {
    "type": "role"
  },
  "channel_bindings": [
    {
      "messenger_id": "qq-bot",
      "user_id": "123456"
    }
  ],
  "admin_users": [
    {
      "messenger_id": "qq-bot",
      "user_id": "admin_user_001"
    }
  ],
  "stations": [
    {
      "station_id": "local-station",
      "base_url": "http://127.0.0.1:9001",
      "timeout_secs": 30
    }
  ],
  "memory_store_url": "http://127.0.0.1:8080",
  "memory_ego_url": "http://127.0.0.1:8081",
  "memory_struct_url": "http://127.0.0.1:8082",
  "ws_reconnect_interval_secs": 5
}
```

### 配置键说明

| 字段 | 说明 | 默认值 |
|------|------|--------|
| agent_id | Agent 唯一标识 | 必填 |
| llm | LLM API 配置 | 必填 |
| current_role | 当前角色名称，空串表示无角色 | "" |
| current_mode.type | "role" 或 "event" | "role" |
| current_mode.event_id | 事件模式时的事件 ID | - |
| channel_bindings | 绑定的 channel 用户列表 | [] |
| admin_users | 管理权限用户列表 | [] |
| stations | Station 地址列表 | [] |
| memory_store_url | Memory-Store 服务地址 | 必填 |
| memory_ego_url | Memory-Ego 服务地址 | 必填 |
| memory_struct_url | Memory-Struct 服务地址 | 可选 |
| ws_reconnect_interval_secs | WS 断线重连间隔 | 5 |

## 管理命令格式

### 通用规则

- 管理命令以 `/` 为前缀，参数以空格分隔
- 发送者必须在 admin_users 列表中
- 发送者不在 admin_users 中的命令被忽略（不回复也不进入 agentic loop）
- 命令执行结果回复到原通道

### 命令列表

| 命令 | 语法 | 说明 |
|------|------|------|
| bind | `/bind messenger <messenger_id> <user_id>` | 绑定 channel 用户 |
| unbind | `/unbind messenger <messenger_id>` | 解绑 channel |
| admin | `/admin <messenger_id> <user_id>` | 添加管理权限 |
| unadmin | `/unadmin <messenger_id> <user_id>` | 移除管理权限 |
| role | `/role` | 取消角色（无角色） |
| role | `/role <role_name>` | 切换角色 |
| mode | `/mode event` | 生成新 event-id 进入事件模式 |
| mode | `/mode role` | 回到角色模式 |
| reenter | `/reenter <event-id>` | 重进指定事件会话 |
| events | `/events` | 查询事件列表 |
| reset | `/reset` | 上下文重置 |

### 命令处理状态码

| 结果 | 回复内容 |
|------|----------|
| 成功 | "✅ 命令执行成功：[描述]" |
| 无权限 | 忽略，不回复 |
| 参数错误 | "⚠️ 命令格式错误：[提示]" |
| 内部错误 | "❌ 命令执行失败：[错误描述]" |

## WS 通信协议

### 连接流程

1. Nexus 作为 WS 客户端，连接到消息通道的 WSS 服务器（按配置的 URL 选择 ws:// 或 wss://）
2. 连接建立后，发送 MessengerInfo 请求（TYPE_MESSENGER_INFO_REQUEST = 0x00010001）
3. 获取 MessengerInfo 后，发送 BindRequest（TYPE_BIND_AGENT_USER = 0x00020002）
4. 绑定成功后进入正常消息收发状态

### 心跳

使用 kai-ws 库的 WsHeartbeatHandler，发送 bin 格式心跳包。

### 重连策略

- 连接断开后等待 `ws_reconnect_interval_secs` 后自动重连
- 重连后重新执行连接流程（MessengerInfo → BindRequest）

## Memory-Store API 交互

### 写入（MemoryWriter）

| 方法 | 路径 | 请求体 |
|------|------|--------|
| POST | /think | ThinkRequest（数组） |
| POST | /tool-call | ToolCallRequest（数组） |
| POST | /tool-result | ToolResultRequest（数组） |

请求结构体定义在 kissbot-api 的 store.rs 中。

### 读取（MemoryReader）

| 方法 | 路径 | 请求体 |
|------|------|--------|
| POST | /query/channel | QueryRequest — 按 (agent_id, role_name) + 时间范围查询（所有 channel 记录同文件，无需组合枚举） |
| POST | /query | QueryRequest — 按 (agent_id, role_name) + 时间范围查询 |

### 记忆索引读取（MemoryReader → Memory-Struct）

在启动、模式切换、上下文重置时通过 HTTP 调用 memory-struct 获取顶层记忆索引，作为初始上下文的一部分。memory-struct 未配置时跳过。

| 方法 | 路径 | 请求体 |
|------|------|--------|
| POST | /index | { "agent_id": "...", "role_name": "..." } — 返回记忆索引列表（如摘要） |

### 后续记忆搜索（Agentic Loop 内 Tool Call）

在 agentic loop 中，当 LLM 需要回顾更早的历史时，通过内置工具 call 调用 memory-struct 的搜索接口。由 ToolCallDispatcher 识别为内置工具，不经过 station。由 ToolCallDispatcher 实现。

### 事件查询

| 方法 | 路径 | 请求体 |
|------|------|--------|
| POST | /events | { "agent_id": "...", "role_name": "..." } — 返回事件列表 |

## Memory-Ego API 交互

### 读取自我认知

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | /role/get | 获取角色设定（role_name） |
| POST | /agent/get | 获取 agent 元数据（按 agent_id） |
| POST | /agent/search-name | 按 individual_name 全匹配返回 agent_id（Option） |
| POST | /role/retrieve | 批量获取角色信息 |

### agent_name 绑定与解析

- channel 绑定使用 **agent_name**（= memory-ego 的 `AgentMetadata.individual_name`，代号）记录，配置字段为 `ChannelConfig.agent_name`
- **保留 agent**：`agent_name == ""` 表示保留 agent（建会话、用 `default_system_prompt`、不调 memory-ego），其内部 agent_id 固定为 `"0"`
- **agent_name == "0"** 为普通代号（正常解析，与保留无关）；不再有脱离态（`session_key_of` 去掉 Option，始终返回 SessionKey）
- **解析**：coordinator 维护 `agent_name -> agent_id` 缓存（`resolve_agent_id`）；非空 agent_name 调 `/agent/search-name` 全匹配得 agent_id，失败/不可用回退 `agent_id="0"`
- **memory-store/ego 用 agent_id（UUID）**：会话 key 用 agent_name（同步、无需解析），memory-store 目录与 ego 读取用解析后的 agent_id

## LLM API 适配

### 支持的提供商

| provider 值 | 兼容 API |
|-------------|----------|
| "openai" | OpenAI Chat Completions API（/v1/chat/completions） |
| "anthropic" | Anthropic Messages API（/v1/messages） |

### 通用请求参数

```json
{
  "provider": "openai",
  "model": "gpt-4o",
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "..." },
    { "role": "assistant", "content": "..." }
  ],
  "max_tokens": 4096,
  "temperature": 0.7,
  "stream": false
}
```

### 响应格式（归一化）

```rust
struct LlmResponse {
    content: String,            // 回复文本
    tool_calls: Vec<ToolCall>,  // 工具调用列表
    finish_reason: String,      // "stop", "tool_use", "length" 等
}
```

## 数据结构定义

### ToolCall

```rust
struct ToolCall {
    id: String,
    tool_name: String,
    parameters: serde_json::Value,
}
```

### WriteTask（MemoryWriter 内部队列）

```rust
enum WriteTask {
    Think {
        agent_id: String,
        role_name: Option<String>,
        content: String,
        time: String,
    },
    ToolCall {
        agent_id: String,
        role_name: Option<String>,
        tool_name: String,
        tool_params: serde_json::Value,
        time: String,
    },
    ToolResult {
        agent_id: String,
        role_name: Option<String>,
        tool_result: serde_json::Value,
        time: String,
    },
}
```

### 模式状态

```rust
enum Mode {
    Role,                    // 角色模式（默认）
    Event(String),           // 事件模式，String = 事件 ID
}
```

### 管理命令类型

```rust
enum AdminCommand {
    Bind { messenger_id: String, user_id: String },
    Unbind { messenger_id: String },
    Admin { messenger_id: String, user_id: String },
    Unadmin { messenger_id: String, user_id: String },
    SetRole(Option<String>),     // None = 取消角色
    ModeEvent(Option<String>),   // None = 新事件，Some = 进入指定事件
    ModeRole,
    Reenter(String),
    Events,
    Reset,
}
```

## 错误类型

```rust
enum Error {
    ConfigNotFound,
    ConfigParseError(String),
    LllmApiError(String),
    LllmProviderNotSupported(String),
    MemoryStoreError(String),
    MemoryEgoError(String),
    WsConnectionError(String),
    WsBindError(String),
    StationConnectionError(String),
    InvalidCommand(String),
    PermissionDenied,
    ModeConflict(String),
    ContextOverflow,
    SerializationError(String),
    IoError(String),
}
```

## MemoryWriter 队列参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| 队列容量 | 1024 | 队列满时阻塞推送方 |
| 后台重试间隔 | - | 不重试，失败记录日志 |
