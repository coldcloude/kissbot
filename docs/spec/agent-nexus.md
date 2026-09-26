# kissbot-agent-nexus 技术规格

> 说明 kissbot-agent 的配置格式、管理命令、与外部组件的接口约定与关键数据结构。
> 模块调用关系见 [agent-modules.md](agent-modules.md)；station 侧约定见 [kissbot-agent-station](agent-station.md)。

## 配置文件格式

kissbot-agent 使用三份配置：

| 文件 | 内容 | 管理者 |
|------|------|--------|
| 公共配置（`KISSBOT_CONFIG`，默认 `./config.json`） | 各组件共享：`security` / `api` / `agent` / `memory` / 通道等段 | kissbot-config（只读，进程级单例） |
| `<data_dir>/nexus.json` | agent 可改配置：channels / providers / memory_structs / agents / sessions | ConfigManager（读写，落盘） |
| `<data_dir>/station.json` | station 可改配置：station_id / groups / sub_stations | ConfigManager（读写，落盘） |

`data_dir` 来自公共配置 `agent` 段。

### 公共配置（agent 相关段）

```jsonc
{
  "security": {
    "api_key": "user-key-456",        // 组件间互调（含 channel / station MCP）的 API key
    "admin_api_key": "admin-key-123"  // 管理 API 的 key
  },
  "api": {
    "memory_store_url": "http://127.0.0.1:8082",           // memory-store 地址（空 = 关闭读写）
    "memory_ego_url": "http://127.0.0.1:3001",             // memory-ego 地址（空 = ego 未配置）
    "memory_struct_abstract_url": "http://127.0.0.1:8083"  // 摘要记忆地址（空 = 未部署，输入处理器降级为普通合批）
  },
  "agent": {
    "data_dir": "agent-data",                 // nexus.json / station.json / 会话上下文的根目录
    "mgmt_host": "127.0.0.1",
    "mgmt_port": 9090,                        // 管理 API 监听
    "station_host": "127.0.0.1",
    "station_port": 9100,                     // station MCP 端点监听（被其他 station 作为 sub 调用）
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
        "group_set": { "group_set": ["filesystem"] },
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

- `providers` 段的配置结构（`provider_config` / `default_model_config` / `model_configs`）与「provider 默认 ← model 覆盖」的合成由 kissbot-llm 定义，结构与合成规则见 [kissbot-llm 技术规格](llm.md)；agent 只负责其存储、落盘与 CRUD。
- `has_*` 标志区分「未配置」与「显式置空」：`set_*` 置 `has_* = true`（取值可为 null），`unset_*` 置回 false；合并时只覆盖 `has_*` 为 true 的字段。
- session 条目里未覆盖的配置项序列化为 `null`（`SessionConfigMap` 各字段均为 `Option`）。
- `input_processor` / `system_prompter` 为**数组**（同一会话可挂多个输入处理器与系统提示词器）；`message_sender` / `tool_caller` / `output_processor` 为单个名称。
- `input_processor` 可选 `batch_input_processor`（默认，按会话合批）与 `abstract_input_processor`（摘要记忆：按通道攒批 + 超时触发 + case 上下文重建）。
- `sessions` 的 key 是 `SessionKey`（结构体），JSON 不支持结构体作 map 键，故以 `[[key, value], ...]` 数组对承载。

### 配置继承与合成

配置结构（`ProviderModelConfig` / `LLMConfig` / `EffectiveModelConfig` / `EffectiveLLMConfig`）与继承合并的通用约定由 kissbot-llm / kissbot-api 提供；agent 负责按下列继承链合成并缓存：

| 配置 | 继承链 | 合成结果 |
|------|--------|----------|
| 模型参数 | 全局常量 ← provider 默认（`default_model_config`）← model 覆盖（`model_configs`） | `EffectiveModelConfig`（max_tokens_usage / timeout_secs / retry_count） |
| LLM 参数 | LLMConfig 默认 ← agent 级 ← role 级 ← session 覆盖 | `EffectiveLLMConfig`（provider / model / max_tokens / temperature / thinking / reasoning_effort） |
| 其它会话配置 | 默认值 ← agent 级 ← role 级（compress / memory_recover / channel_batch / out_channel / group_set / pipeline） | 各自 Effective 形态 |
| 模型参数兜底 | 模型未在 `model_configs` 配置时直接用 provider 默认值合成 | — |

查询入口：`ConfigManager::session_config::<C, E>(session_key)`（带缓存，首次按 agent+role 合成并写入 sessions）、`ConfigManager::provider_model_config(provider, model)`。运行中修改 agent 级配置不会回刷已合成的 session 配置（需 `set_session_config` 覆盖）。

例外：`out_channel` 走 `ConfigManager::out_channel_config(agent_id, role_name)`，每次按 agent+role 实时合成而不读 session 快照——`/bind-outgoing`、`/unbind-outgoing` 对已存在的会话立即生效。

## 会话集合与上下文本地缓存（session_manager）

| 项 | 约定 |
|----|------|
| 会话集合 | 按 (agent_id, role_name, mode) 去重持有；绑定信息变化时仅保留仍在绑定集合中的会话 |
| session_id 编码 | 角色模式 `{agent_id}-{role_name}`；事件模式 `{agent_id}-{role_name}-{event}`（作为 kissbot-llm 的会话标识，决定缓存/归档文件名） |
| 上下文本体 | 交给 kissbot-llm 的 Session/SessionContext（内存消息 + 本地缓存 + 历史归档一体） |
| 恢复 | 新建会话时从 `<data_dir>/context/<session_id>.jsonl` 正序全量回读；崩溃一致性清理（丢弃悬挂 tool_calls 尾部与开头孤立 Tool 行）在 kissbot-llm 内完成 |

缓存文件、行序、归档文件命名与系统消息的待定/应用语义见 [kissbot-llm 技术规格](llm.md)。

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

## Memory-Struct-Abstract API 交互

摘要记忆输入处理器（`abstract_input_processor`，需在 `pipeline.input_processor` 显式启用）经 `AbstractClient` 调用摘要记忆，鉴权用 `X-Api-Key`：

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/abstract/locate` | 定位一批上行消息涉及的 case（请求含 agent_id、role 与带通道三元组与 msg_id 的消息） |
| POST | `/abstract/case/summary` | 取 case 摘要（节点本身：摘要文本 + 层级 + 记录数 + 起止时间） |
| POST | `/abstract/case/records-tail` | 取 case 结尾 count 条原文副本（升序） |

- `role` 与会话写记忆同一编码，即 **role-event 记忆后缀**：角色模式为角色名，事件模式为 `{role}-{event}`（`role_mode`）。摘要记忆按**第一个** `-` 拆分出角色名与事件标识（角色名不允许含 `-`，事件标识可以含 `-`）：后缀决定原始记录读取、水位、当前 case 指针与模型会话，前缀（角色名）决定 case 树；因此同一角色的角色模式与事件模式共用一棵 case 树（不同角色之间相互隔离）。
- 触发策略：消息按通道（messenger + 接收方用户 + 群组）攒批，每秒检测一次，最后一条消息超过 `channel_batch.channel_batch_interval_secs` 即触发该通道批次。
- 触发时用「涉及 case 的全部摘要 + 各 case 结尾 `memory_recover.memory_count` 条、且时间落在 `memory_recover.memory_time_secs` 内的通道原文副本」重置会话上下文；还没有归档内容的 case 跳过、不进上下文：记忆条数为 0 的 case 正是本批消息刚定位建出的（它概括的就是本批消息，原文也会随本批进 pipeline），摘要为空的只可能是历史遗留数据。查出的记录与本批新消息按「通道三元组 + `msg_id`」去重（case 原文副本可能来自多个通道，故带通道三元组；`msg_id` 为空的不参与去重、原样保留），去重后本批新消息整体进 pipeline。定位请求本身按通道发出（每批消息同属一个通道，消息仍带通道三元组）。
- `memory_struct_abstract_url` 为空或任一调用失败：记 warn，不重置上下文，直接以合批消息运行 pipeline（与 `batch_input_processor` 行为一致）。

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

provider 实现（OpenAI 兼容 / Anthropic）的构造（`create_*_provider`）、调用、重试与响应归一化由 kissbot-llm 提供；agent 侧的 ProviderManager 负责按 provider 名缓存实现实例（工厂去重）、解析有效模型配置（超时 / 重试 / token 上限）并按 `provider_type` 分派。

| provider_type | 兼容 API | 请求路径 | 模型列表 |
|---------------|----------|----------|----------|
| "openai" | OpenAI Chat Completions | `POST {base_url}/chat/completions` | `GET {base_url}/models` |
| "anthropic" | Anthropic Messages | `POST {base_url}/v1/messages` | `GET {base_url}/v1/models` |

请求参数（`EffectiveLLMConfig`）、响应归一化（`ModelResponse`）与重试语义见 [kissbot-llm 技术规格](llm.md)；未知 `provider_type` 由 ProviderManager 在分派处返回 `ModelProviderNotSupported`（不重试）。

## 数据结构定义

```rust
// 上下文消息、工具调用与模型响应结构（Message / ToolData / ToolCall / ModelResponse / ToolConfig）
// 由 kissbot-llm 定义（与 MCP 工具的互转在 agent 侧），见 [kissbot-llm 技术规格](llm.md)

// 会话标识：agent_id + role_name + mode 三元组（所有绑定 channel 的信息按此去重）
struct SessionKey { agent_id: String, role_name: String, mode: Mode }

// 模式状态
enum Mode {
    Role,           // 角色模式（默认）
    Event(String),  // 事件模式，String = 事件 ID
}

// 记忆读写边界的 role 编码：事件模式拼 {role}-{event}，角色模式原样
fn role_mode(role_name: &str, mode: &Mode) -> String;

// 会话标识 → kissbot-llm 会话标识（缓存/归档文件名段）：{agent_id}-{role_name}[-{event}]
impl SessionKey { fn encode(&self) -> String; }
```

## 错误类型

模型调用相关错误由 kissbot-llm 定义（`ModelApiError` / `ModelProviderNotFound` / `ModelProviderNotSupported` / IO 与解析类），agent 在自己的错误类型上逐变体映射，保持本枚举的对外错误表面不变。

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
    AbstractError(String),           // 摘要记忆客户端（未配置 / 请求 / 解析 / 业务失败）
    WsConnectionError(String),       // 预留
    WsBindError(String),             // 预留
    StationConnectionError(String),  // 预留
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
