# kissbot-agent-nexus 技术规格

> 说明 kissbot-agent 的配置格式、管理命令、与外部组件的接口约定与关键数据结构。
> 模块调用关系见 [agent-modules.md](agent-modules.md)；station 侧约定见 [kissbot-agent-station.md](kissbot-agent-station.md)。

## 配置文件格式

kissbot-agent 使用三份配置：

| 文件 | 内容 | 管理者 |
|------|------|--------|
| 公共配置（`KISSBOT_CONFIG`，默认 `./config.json`） | 各组件共享：`security` / `api` / `agent` / `memory` / 通道等段 | kissbot-config（只读，进程级单例） |
| `<data_dir>/nexus.json` | agent 可改配置：channels / providers / memory_structs / agents / sessions | ConfigManager（读写，落盘） |
| `<data_dir>/station.json` | station 可改配置：station_id / toolkits / sub_stations | ConfigManager（读写，落盘） |

`data_dir` 来自公共配置 `agent` 段。

### 公共配置（agent 相关段）

```jsonc
{
  "security": {
    "api_key": "user-key-456",        // 组件间互调（含 channel / station HTTP）的 API key
    "admin_api_key": "admin-key-123"  // 管理 API 的 key
  },
  "api": {
    "memory_store_url": "http://127.0.0.1:8082",  // memory-store 地址（空 = 关闭读写）
    "memory_ego_url": "http://127.0.0.1:3001"     // memory-ego 地址（空 = ego 未配置）
  },
  "agent": {
    "data_dir": "agent-data",                 // nexus.json / station.json / 会话上下文的根目录
    "mgmt_host": "127.0.0.1",
    "mgmt_port": 9090,                        // 管理 API 监听
    "station_host": "127.0.0.1",
    "station_port": 9100,                     // station 对外 HTTP 监听（被其他 station 作为 sub 调用）
    "ws_reconnect_interval_secs": 5           // channel 断线重连间隔
  }
}
```

### nexus.json（NexusRepo）

```jsonc
{
  "channels": {
    "web1": {
      "channel_id": "web1",
      "ws_url": "ws://127.0.0.1:8201",
      "admins": [ { "messenger_id": "web", "user_id": "admin" } ],
      "bind_users": [ { "messenger_id": "web", "user_id": "u1" } ],
      "agent_id": "0",          // 绑定的 agent；缺省/空串归一化为保留 agent "0"
      "role_name": "",
      "enabled": true
    }
  },
  "providers": {
    "deepseek": {
      "provider_config": {
        "provider_type": "openai",              // "openai" | "anthropic"
        "base_url": "https://api.deepseek.com",
        "api_key": "sk-..."
      },
      "default_model_config": { "max_tokens_usage": 128000, "timeout_secs": 60, "retry_count": 3 },
      "model_configs": {
        "deepseek-flash": { "max_tokens_usage": 131072, "timeout_secs": 30 }
      }
    }
  },
  "memory_structs": {},
  "agents": {
    "0": {                       // key = agent_id（"0" = 保留 agent）
      "config_map": {            // agent 级默认
        "llm": { "provider": "deepseek", "model": "deepseek-flash", "max_tokens": 4096, "has_max_tokens": true,
                 "temperature": 0.7, "has_temperature": true,
                 "thinking": "enabled", "has_thinking": true,
                 "reasoning_effort": "low", "has_reasoning_effort": true },
        "compress": { "compress_prompt": "…", "compress_threshold": 0.8 },
        "memory_recover": { "memory_time_secs": 3600, "memory_count": 50 },
        "channel_batch": { "channel_batch_interval_secs": 3 },
        "out_channel": { "out_channel": { "channel_id": "web1",
                                          "user": { "messenger_id": "web", "user_id": "u1" },
                                          "group_id": "g1" } },
        "toolkit_set": { "toolkit_set": ["filesystem"] },
        "pipeline": { "preset": "preset_role",
                      "input_processor": ["batch_input_processor"],
                      "system_prompter": ["memory_ego_system_prompter"],
                      "message_sender": "memory_recover_message_sender",
                      "tool_caller": "station_tool_caller",
                      "output_processor": "channel_output_processor" }
      },
      "roles": { "dev": { "llm": { "model": "deepseek-v4-pro" } } }
    }
  },
  "sessions": [
    [ { "agent_id": "0", "role_name": "", "mode": "Role" },
      { "llm": { "provider": "deepseek", "model": "deepseek-flash", "max_tokens": null,
                 "temperature": null, "thinking": null, "reasoning_effort": null } } ]
  ]
}
```

说明：

- `has_*` 标志区分「未配置」与「显式置空」：`set_*` 置 `has_* = true`（取值可为 null），`unset_*` 置回 false；合并时只覆盖 `has_*` 为 true 的字段。
- session 条目里未覆盖的配置项序列化为 `null`（`SessionConfigMap` 各字段均为 `Option`）。
- `input_processor` / `system_prompter` 为**数组**（同一会话可挂多个输入处理器与系统提示词器）；`message_sender` / `tool_caller` / `output_processor` 为单个名称。
- `sessions` 的 key 是 `SessionKey`（结构体），JSON 不支持结构体作 map 键，故以 `[[key, value], ...]` 数组对承载。

### 配置继承与合成

| 配置 | 继承链 | 合成结果 |
|------|--------|----------|
| 模型参数 | 全局常量 ← provider 默认（`default_model_config`）← model 覆盖（`model_configs`） | `EffectiveModelConfig`（max_tokens_usage / timeout_secs / retry_count） |
| LLM 参数 | LLMConfig 默认 ← agent 级 ← role 级 ← session 覆盖 | `EffectiveLLMConfig`（provider / model / max_tokens / temperature / thinking / reasoning_effort） |
| 其它会话配置 | 默认值 ← agent 级 ← role 级（compress / memory_recover / channel_batch / out_channel / toolkit_set / pipeline） | 各自 Effective 形态 |
| 模型参数兜底 | 模型未在 `model_configs` 配置时直接用 provider 默认值合成 | — |

查询入口：`ConfigManager::session_config::<C, E>(session_key)`（带缓存，首次按 agent+role 合成并写入 sessions）、`ConfigManager::provider_model_config(provider, model)`。运行中修改 agent 级配置不会回刷已合成的 session 配置（需 `set_session_config` 覆盖）。

例外：`out_channel` 走 `ConfigManager::out_channel_config(agent_id, role_name)`，每次按 agent+role 实时合成而不读 session 快照——`/bind-outgoing`、`/unbind-outgoing` 对已存在的会话立即生效。

## 会话上下文本地缓存与归档（session_manager）

| 项 | 约定 |
|----|------|
| 缓存文件 | `<data_dir>/context/<session_key 编码>.jsonl`，每行一条 Message（JSON），追加写、不截断 |
| session_key 编码 | 角色模式 `{agent_id}-{role_name}`；事件模式 `{agent_id}-{role_name}-{event}` |
| 行序 | 首行为当前系统消息（如有），其后为消息行 |
| 恢复 | 正序全量回读；崩溃一致性清理——丢弃末尾悬挂的 assistant tool_calls 行与开头的孤立 Tool 行；文件不存在视为空上下文 |
| 归档 | `<data_dir>/context-history/<session_key 编码>-<时间戳>.jsonl`（时间戳 `%Y-%m-%d-%H%M%S`），格式与缓存一致 |

- 系统消息的 set 只写入待定值（多次 set 保留最近一次）；发送前对比应用：与当前一致仅清空待定，不一致则先归档并清空缓存再以新系统消息重建。
- 归档与清空缓存文件配对执行；内存为空时不产生归档文件。

## 管理命令格式

### 通用规则

- 管理命令以 `/` 为前缀，参数以空格分隔（`split_whitespace`，不 trim 前置空格）
- 发送者必须在来源 channel 的 `admins` 列表中，否则命令被忽略（不回复也不进入 pipeline）
- 命令执行结果回复到原通道（不走 out_channel）
- 命令与命令回复不写入记忆、不进入会话上下文

### 命令列表

| 命令 | 语法 | 说明 |
|------|------|------|
| bind | `/bind messenger <messenger_id> <user_id>` | 绑定 channel 用户（bind_users 追加，幂等） |
| unbind | `/unbind messenger <messenger_id> <user_id>` | 解绑 channel 用户 |
| admin | `/admin <messenger_id> <user_id>` | 添加管理权限 |
| unadmin | `/unadmin <messenger_id> <user_id>` | 移除管理权限 |
| agent | `/agent [agent_id] [role]` | 设置 agent（缺省 = 保留 agent "0"）与角色（缺省 = 保留 role 空串）；经 ego 校验 |
| role | `/role [role_name]` | 切换角色（缺省 = 保留 role） |
| mode | `/mode event [event-id]` | 进入事件模式（缺省生成新 event-id） |
| mode | `/mode role` | 回到角色模式 |
| bind-outgoing | `/bind-outgoing <messenger_id> <user_id> <group_id>` | 设 (agent, role) 的回复通道（要求该身份已绑定来源 channel） |
| unbind-outgoing | `/unbind-outgoing` | 清除回复通道（回到只存不回复） |
| model | `/model <provider> <model>` | 切换当前会话模型（先拉模型列表校验，失败保持原模型） |

agent/role/mode 变更与 bind/unbind 都经 channel 变更队列串行执行（写-写无竞态）；bind-outgoing/unbind-outgoing 为纯配置写。命令回复文案形如：`✅ 已设置 agent: {agent_id} / role: {role}`、`✅ 已切换模型为: {provider}/{model}`。

### 命令处理结果

| 结果 | 表现 |
|------|------|
| 成功 | `✅ …`（各命令自带描述） |
| 无权限 | 忽略，不回复 |
| 参数错误 | `⚠️ {提示}`（InvalidCommand） |
| 执行失败 | `❌ 命令执行失败: {错误描述}` |

## WS 通信协议

agent 通过 `kissbot-channel-client` 连接消息通道（kai-ws 客户端，JSON 消息 + 二进制心跳）：

1. `ChannelClient::connect(url, api_key)` 建立连接（header 携带 `X-Api-Key`）
2. 对 `bind_users` 中每个身份调用 `ChannelClient::bind(BindRequest { messenger_id, user_id })`（TYPE_BIND_AGENT_USER）
3. 心跳由 kai-ws 的 `WsHeartbeatHandler` 负责（二进制帧）
4. 上行：TYPE_INCOMING_MESSAGE → `Terminal::incoming_message`；群组/用户事件 → `join_group` / `leave_group` / `user_removed`（当前 no-op）；连接关闭 → `closed`（触发重连）
5. 下行：`ChannelClient::send_message(OutgoingMessage)`（TYPE_OUTGOING_MESSAGE），附件经上传分片 / 下载请求接口

断线后等待 `ws_reconnect_interval_secs` 重连，重连成功后重新绑定 `bind_users`（每次重连实时读取配置，`/bind` 后重连即生效）。

## Memory-Store API 交互

### 写入（MemoryStoreClient，经 appender 批量投递）

| 方法 | 路径 | 请求体 |
|------|------|--------|
| POST | `/store/channel` | ChannelRequest（数组） |
| POST | `/store/think` | ThinkRequest（数组） |
| POST | `/store/tool-call` | ToolCallRequest（数组） |
| POST | `/store/tool-result` | ToolResultRequest（数组） |

请求结构体定义在 kissbot-api；`base_url` 为空时写入直接跳过（Ok），鉴权用 `X-Api-Key`。

### 读取（MemoryStoreClient）

| 方法 | 路径 | 请求体 |
|------|------|--------|
| POST | `/store/query/channel/recent` | RecentQuery — 按 (agent_id, role_name) 取最近 count 条（跨日期文件） |
| POST | `/store/query/channel` | QueryRequest — 按 (agent_id, role_name) + 时间范围查询（所有 channel 记录同文件，无需组合枚举） |

`read_recent_for_context` 用两次查询并集：① recent 取最近 N 条，`T_N` = 最旧一条时间；② 时间范围 `[M, T_N]`（`M = min(时间窗起点, T_N)`）取该段全部；结果按 (time, sn) 去重后时间正序，打包为交替的 User/Assistant 消息（结尾为 User 时补空 Assistant）。读路径在 `base_url` 为空时报错（不静默跳过）。

## Memory-Ego API 交互

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/agent/get` | 获取 agent 元数据（按 agent_id）；`data` 为 null 表示不存在 |
| POST | `/individual/get-all` | 获取个体识别信息（按 agent_id） |
| POST | `/role/get` | 获取角色设定（agent_id + role_name） |

鉴权同样用 `X-Api-Key`（`security.api_key`）。ego 未配置 / 请求失败返回 Err，调用方按"失败静默跳过"处理。

### agent_id 绑定与解析

- channel 绑定直接使用 **agent_id**（配置字段 `ChannelConfig.agent_id`），会话 key、memory-store 目录与 ego 查询均直接用该值，无需二次解析
- **保留 agent**：`agent_id == "0"` 表示保留 agent（不调 memory-ego，系统提示词回退默认值）
- **校验**：`/agent` 设置前经 ego `/agent/get` 校验存在（`verify_agent_exists`），role 非空时另经 `/role/get` 校验（`verify_role_exists`）；失败保持原值并报错

## LLM API 适配

### 支持的提供商

| provider_type | 兼容 API | 请求路径 | 模型列表 |
|---------------|----------|----------|----------|
| "openai" | OpenAI Chat Completions | `POST {base_url}/chat/completions` | `GET {base_url}/models` |
| "anthropic" | Anthropic Messages | `POST {base_url}/v1/messages` | `GET {base_url}/v1/models` |

### 请求参数（EffectiveLLMConfig）

```jsonc
{
  "model": "deepseek-flash",
  "messages": [ { "role": "system", "content": "…" }, { "role": "user", "content": "…" } ],
  "max_tokens": 4096,          // 未配置时 anthropic 回退 65535
  "temperature": 0.7,
  "thinking": { "type": "enabled" },   // openai：映射为协议枚举；anthropic：原样 { "type": … }
  "reasoning_effort": "low",           // openai：原样字段；anthropic：output_config.effort
  "stream": false                      // 固定非流式
}
```

- messages 直接使用会话上下文的 `Message` 序列（OpenAI 兼容格式）；anthropic 分支把 system 提取为顶层 `system` 字段、丢弃 Tool 消息（本轮不支持工具消息）
- tools 非空时按 provider 协议包装（openai：`{type:"function", function:{name,description,parameters}}`）；为空则不携带该字段

### 响应归一化（ModelResponse）

| 字段 | 说明 |
|------|------|
| raw_content / content | API 原始 content 与剥离 `<think>…</think>` 后的正文 |
| raw_reasoning_content / reasoning_content | API 思考字段原值 / 解析值（openai `reasoning_content`；anthropic thinking block） |
| thinking | 正文中 `<think>` 标签内容（与 reasoning_content 相互独立） |
| raw_tool_calls / tool_calls | API 原始 tool_calls（原样回传上下文）/ 解析出的 `ToolCall` 列表 |
| finish_reason | 停止原因（当前无消费方） |
| total_tokens | openai：`usage.total_tokens`（缺省 0）；anthropic：固定 0 |

调用失败按 `retry_count` 指数退避重试；未知 `provider_type` 立即返回 `ModelProviderNotSupported`（不重试）。

## 数据结构定义

```rust
// 上下文消息（OpenAI 兼容，序列化时 role 为平级标签）
enum Message {
    System { content: Arc<String> },
    User { content: Arc<String> },
    Assistant { content: Value, reasoning_content: Value, tool_calls: Value },  // 后三者原样透传 API 值
    Tool { tool_call_id: Arc<String>, content: Arc<String> },                    // 无 name 字段
}

// 工具调用：arguments 执行前、result / error 执行后（三者均为 JSON）
struct ToolData { arguments: Value, result: Value, error: Value }
struct ToolCall { id: Arc<String>, name: Arc<String>, data: ToolData }

// 会话标识：agent_id + role_name + mode 三元组（所有绑定 channel 的信息按此去重）
struct SessionKey { agent_id: String, role_name: String, mode: Mode }

// 模式状态
enum Mode {
    Role,           // 角色模式（默认）
    Event(String),  // 事件模式，String = 事件 ID
}

// 记忆读写边界的 role 编码：事件模式拼 {role}-{event}，角色模式原样
fn role_mode(role_name: &str, mode: &Mode) -> String;
```

## 错误类型

```rust
enum Error {
    ConfigNotFound(String),
    ConfigParseError(String),
    ModelApiError(String),
    ModelProviderNotFound(String, String),
    ModelProviderNotSupported(String),
    MemoryStoreError(String),
    MemoryTimeWindow(String),
    MemoryEgoError(String),          // 预留
    WsConnectionError(String),       // 预留
    WsBindError(String),             // 预留
    StationConnectionError(String),  // 预留
    StationCycle(String),
    InvalidCommand(String),
    PermissionDenied,                // 预留
    ModeConflict(String),            // 预留
    ContextOverflow,                 // 预留
    SerializationError(String),      // 预留
    IoError(String),
    ReqwestError(reqwest::Error),
    JsonError(serde_json::Error),
    WsError(kai_ws::Error),
    InternalError(String),           // 预留
    PipelineNotFound(String, String, SessionKey),  // (组件类别, 配置名, 会话 key)
}
```

## 管理 API（HttpServer）

认证：`x-api-key` 必须等于 `security.admin_api_key`（AuthLayer 拦截，缺失/错误返回 401）。

| 方法 | 路径 | 请求体 | 说明 |
|------|------|--------|------|
| GET | `/config` | — | 返回 NexusRepo 快照（channels / providers / memory_structs / agents / sessions） |
| POST | `/config/providers` | `{ "name": "...", "config": ProviderModelConfig }` | 新增 provider（重名失败） |
| POST | `/config/providers/remove` | `{ "name": "..." }` | 删除 provider |
| POST | `/config/channels` | `ChannelConfig` | 新增 channel |
| POST | `/config/channels/remove` | `{ "channel_id": "..." }` | 删除 channel |
| POST | `/config/admins` | `{ "channel_id", "messenger_id", "user_id" }` | 添加管理权限 |
| POST | `/config/admins/remove` | `{ "channel_id", "messenger_id", "user_id" }` | 移除管理权限 |

响应统一为 `{ "success": bool, "data": …, "error": … }`。
