# kissbot-agent ProviderConfig 抽象层设计

## 概述

在 `ModelConfig` / `ModelClient` 之上增加一层 `ProviderConfig` 抽象，把"连接哪个模型服务商、用什么协议"与"具体某个模型"分离：

1. **ProviderConfig（provider 级）**：`provider_type`（openai / anthropic）、`base_url`（URL 前缀，原 `endpoint` 改名且语义变为前缀）、`api_key`、各参数默认值（`default_context_length` / `default_max_tokens` / `default_temperature` / `default_timeout_secs` / `default_retry_count`），以及该 provider 下的 `models` 集合。
2. **ModelConfig（model 级，嵌套在 ProviderConfig 内）**：只保留 `model` 标识 + 可继承参数（`Option`，不配时用 provider 默认）。去掉 `name` / `provider` / `endpoint` / `api_key`。
3. **Provider trait**：`send` 函数，Anthropic 与 OpenAI 各自实现（协议差异：路径、请求头、请求/响应结构）。`ModelClient` 每次调用时按 `provider_type` 构建对应 Provider 实现。
4. **每次调用现场合成**：`ModelClient` 不再持有配置快照，每次调用经 `ConfigManager::resolve_effective_config` 合并出 `EffectiveModelConfig`（provider 默认 + model 覆盖），配置永远最新，删除 `update_config` 热更新。
5. **ProviderModel**：`(provider, model)` 固定打包一起出现，用于函数调用、`current` 运行状态、`default` 配置。
6. 顺带修正：`ChannelConfig` 去掉 `messenger_id` 字段，改用 agent 内部唯一标识 `channel_id` 作为 map key。

## 现状与问题

### 现状

- `ModelConfig`（config_manager.rs）平铺 9 个字段：`name` / `provider` / `endpoint` / `api_key` / `model` / `max_tokens` / `temperature` / `timeout_secs` / `retry_count`，存于 `NexusRepo.models`（`ArcSwapHashMap<String, ModelConfig>`，key = name）。
- `ModelClient`（model_client.rs）持有 `ModelConfig` 快照 + `reqwest::Client`，`call_inner` 按 `provider` 字符串分发到 `call_openai` / `call_anthropic`，两实现大量重复（URL 拼装、错误处理、响应解析）。
- `Coordinator` 持 `Arc<Mutex<ModelClient>>`，`/model` 切换时 `update_config` 热更新。
- `ContextBuilder` 超长检测为硬编码 `MAX_CONTEXT_MESSAGES = 100`（按消息条数），与 token 级 `default_context_length` 语义不同。
- `ChannelConfig` 含 `messenger_id` 字段，channels map key = `messenger_id`；但 `messenger_id` 是消息方标识，不能唯一标识 agent 的 channel（同一 messenger 可对应多个 channel）。

### 问题

- 模型服务的公共参数（base_url、协议类型、默认上下文长度）与模型私有参数混在一个扁平结构里，新增 provider 需重复配置。
- Provider 协议差异靠字符串分支 + 两份重复实现，扩展新 provider 需要改 `ModelClient` 的分发与解析。
- `ModelClient` 持有配置快照，配置变更需要 `update_config` 热更新同步，存在过期配置窗口。
- 无"默认上下文长度"概念落位，后续 token 级截断无从配置。
- `ChannelConfig` 用 `messenger_id` 作 key 无法唯一标识 channel。

## 目标结构

| 概念 | 实体 | 位置 | 说明 |
|------|------|------|------|
| (provider, model) 打包 | `ProviderModel` | config_manager.rs | 函数调用 / current / default 统一携带 |
| provider 配置 | `ProviderConfig` | `NexusRepo.providers`（key = provider 名） | 含 models 嵌套集合与参数默认值 |
| model 配置 | `ModelConfig` | `ProviderConfig.models`（key = model 标识） | 可继承参数 Option |
| 合并后有效配置 | `EffectiveModelConfig` | 运行时合成，不持久化 | Provider.send 的输入 |
| Provider 实现 | `Provider` trait + `OpenAiProvider` / `AnthropicProvider` | 新文件 provider.rs | 协议差异封装 |
| 模型客户端 | `ModelClient` | model_client.rs | 每次调用现场合成 + 按 provider_type 构建 Provider |
| 运行状态 | `current_model: ArcSwap<ProviderModel>` | AgentCoordinator | 替代 current_provider + current_model 两个状态 |
| 默认配置 | `default_model: Arc<ProviderModel>` | NexusRepo | 种子 current_model |

### 关键决策

- **base_url 语义**：URL 前缀（如 `https://api.deepseek.com`），路径由各 Provider 实现拼装（openai 拼 `/chat/completions`，anthropic 拼 `/v1/messages`）。原 `endpoint` 配置值若含路径需迁移为前缀。
- **api_key 归 provider 级**：不同 model 共用 provider 密钥；如需 per-model 密钥，后续在 ModelConfig 增加可选覆盖。
- **default_context_length 本期只落位**：作为配置字段存在（provider 默认 + model 可覆盖），不接入截断逻辑；现有 `MAX_CONTEXT_MESSAGES` 条数截断保持不变。
- **ProviderConfig 字段必填显式**：反序列化缺字段报错（延续 c5094b8 风格），无隐式默认。
- **ProviderModel 打包**：provider 名 + model 标识固定一起出现，不做"由 model 反查 provider"（无 provider_for_model）。
- **channel_id 语义**：agent 内部唯一标识，与 messenger 无关（同一 messenger 可对应多个 channel）；`ChannelClient::new` 的 id 参数、`channel_clients` / `disconnect_notify` / `bound_channels` 索引全部改用 `channel_id`；`incoming_message` 回调的 id 参数（= channel_id）用于消息过滤，消息身份（`ChannelUser.messenger_id`、`check_admin`）仍用消息自带的 `messenger_id`。

## 配置数据结构（config_manager.rs）

```rust
// (provider, model) 固定一起出现：函数调用、current、default 共用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
}

// ProviderConfig 定义在本文件，供 provider / model_client 与本文件的 NexusRepo.providers 共用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: Arc<String>,               // provider 名（providers map 的 key）
    pub provider_type: String,           // "openai" | "anthropic"，决定 Provider 实现
    pub base_url: String,                // URL 前缀，如 https://api.deepseek.com（原 endpoint）
    pub api_key: String,                 // provider 级密钥（从 model 级移上）
    pub default_context_length: u32,     // 默认上下文长度（token），本期只落位
    pub default_max_tokens: u32,
    pub default_temperature: f32,
    pub default_timeout_secs: u64,
    pub default_retry_count: u32,
    pub models: Arc<ArcSwapHashMap<String, ModelConfig>>,  // key = model 标识
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model: String,                   // 与 map key 相同（去掉 name/provider/endpoint/api_key）
    pub max_tokens: Option<u32>,         // 以下 5 个不配时继承 provider 默认
    pub temperature: Option<f32>,
    pub timeout_secs: Option<u64>,
    pub retry_count: Option<u32>,
    pub context_length: Option<u32>,
}

// 合并后的有效配置，运行时合成、不持久化
#[derive(Debug, Clone)]
pub struct EffectiveModelConfig {
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub timeout_secs: u64,
    pub retry_count: u32,
    pub context_length: u32,
}

// ChannelConfig：messenger_id 去掉，map key = channel_id（agent 内部唯一标识，与 messenger 无关）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_id: Arc<String>,         // agent 内部唯一标识，与 messenger 无关
    pub ws_url: Arc<String>,
    pub admins: Arc<HashSet<ChannelUser>>,
    pub default_bind_user: Option<ChannelUser>,
    pub enabled_by_default: bool,
}
```

### NexusRepo 变化

```rust
pub struct NexusRepo {
    pub channels: Arc<ArcSwapHashMap<String, ChannelConfig>>,   // key = channel_id（原 messenger_id）
    pub providers: Arc<ArcSwapHashMap<String, ProviderConfig>>, // 原 models
    pub memory_structs: Arc<ArcSwapHashMap<String, MemoryStructConfig>>,
    pub stations: Arc<ArcSwapHashMap<String, StationConfig>>,
    pub default_agent_id: Arc<String>,
    pub default_role: Arc<String>,
    pub default_model: Arc<ProviderModel>,   // 原 Arc<String>，打包 provider+model
}
```

- `AgentConfig.init_model: Arc<String>` -> `Arc<ProviderModel>`（种子 NexusRepo.default_model）。
- Option 字段序列化 `#[serde(skip_serializing_if = "Option::is_none")]`，缺省即继承。

### ConfigManager 新增 / 修改

- `resolve_effective_config(&self, pm: &ProviderModel) -> Option<EffectiveModelConfig>`：
  1. `providers.get(pm.provider)` 定位 ProviderConfig
  2. `provider.models.get(pm.model)` 定位 ModelConfig
  3. 合并：model 的 Option 覆盖 provider 的 default_*，未配用 provider 默认
  4. 组装 EffectiveModelConfig（含 provider_type / base_url / api_key）
- `add_model` 等 models CRUD 相应改为在 provider 内操作（`provider_for_model` 不需要）。
- channels CRUD：`add_channel` / `channel_ws_url` / `add_admin` / `remove_admin` 的 key 由 messenger_id 改为 channel_id（admin 的 ChannelUser.messenger_id 字段保留，其值对应消息层身份）。

## Provider trait 与实现（新文件 provider.rs）

```rust
use async_trait::async_trait;

#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse>;
}

pub struct OpenAiProvider {
    client: Arc<reqwest::Client>,   // 共享，无固定 timeout（per-request 设置）
    base_url: String,
    api_key: String,
}

pub struct AnthropicProvider {
    client: Arc<reqwest::Client>,
    base_url: String,
    api_key: String,
}
```

- 请求超时：per-request `.timeout(Duration::from_secs(effective.timeout_secs))`。
- OpenAI：`POST {base_url}/chat/completions`，`Authorization: Bearer <api_key>`；body 含 `model` / `messages` / `max_tokens` / `temperature` / `stream: false`；解析 `choices[0].message.content`、`choices[0].finish_reason`（system 消息按 role 直接进 messages）。
- Anthropic：`POST {base_url}/v1/messages`，`x-api-key` + `anthropic-version: 2023-06-01`；system 消息合并为 `system` 字段；解析 `content[0].text`、`stop_reason`。
- 非 2xx：`Error::ModelApiError(status + body)`。
- 两实现无状态、不依赖 ConfigManager，可独立单测（请求构造与响应解析）。

## ModelClient（model_client.rs）

```rust
pub struct ModelClient {
    config_manager: Arc<ConfigManager>,
    client: Arc<reqwest::Client>,   // 共享，无固定 timeout
}

impl ModelClient {
    pub fn new(config_manager: Arc<ConfigManager>) -> Self;

    /// 每次调用：ConfigManager 现场合成最新 EffectiveModelConfig（配置永远最新，无 update_config）
    pub async fn call(&self, pm: &ProviderModel, messages: &[MessageItem]) -> Result<ModelResponse> {
        let effective = self.config_manager.resolve_effective_config(pm).await
            .ok_or_else(|| Error::ModelProviderNotSupported(format!(
                "provider/model 不存在: {}/{}", pm.provider, pm.model)))?;
        let provider: Box<dyn Provider> = match effective.provider_type.as_str() {
            "openai" => Box::new(OpenAiProvider::new(self.client.clone(), &effective.base_url, &effective.api_key)),
            "anthropic" => Box::new(AnthropicProvider::new(self.client.clone(), &effective.base_url, &effective.api_key)),
            other => return Err(Error::ModelProviderNotSupported(other.to_string())),
        };
        // 指数退避重试 effective.retry_count 次（沿用现有逻辑），成功后返回
        ...
    }
}
```

删除：`ModelConfig` 快照字段、`update_config()`、`call_inner` / `call_openai` / `call_anthropic`。

## Coordinator（coordinator.rs）

- 运行状态：`current_provider: ArcSwap<String>` + `current_model: ArcSwap<String>` 合并为 `current_model: ArcSwap<ProviderModel>`；`current_model()` getter 返回 `ProviderModel`。
- 初始化：`default_model`（ProviderModel）直接种子 current_model。
- `set_current_model(&self, pm: ProviderModel) -> Result<()>`：校验 `providers.get(pm.provider).models.get(pm.model)` 存在 -> store；不存在报 `Error::ModelProviderNotSupported`。
- 调用处：`model.call(&self.current_model(), &messages).await`。
- 命令：`/model <provider> <model>` 双参数 -> `AdminCommand::Model(ProviderModel)`（command_router 解析 3 段，不足/多余参数报 `InvalidCommand`）。
- channel 侧：`connect_channels` / `bound_channels_from_channels` 中 `ch.messenger_id` -> `ch.channel_id`；`incoming_message` 回调的 id 参数（= channel_id）传入消息过滤（`bound_channels.contains_key(channel_id)`），消息身份处理（`check_admin`、回复）仍用 `incoming.messenger_id`。

## nexus.json 新格式与迁移

```json
{
  "channels": {},
  "providers": {
    "deepseek": {
      "name": "deepseek",
      "provider_type": "openai",
      "base_url": "https://api.deepseek.com",
      "api_key": "sk-test-ok",
      "default_context_length": 65536,
      "default_max_tokens": 4096,
      "default_temperature": 0.7,
      "default_timeout_secs": 60,
      "default_retry_count": 3,
      "models": {
        "deepseek-4-flash": { "model": "deepseek-4-flash" }
      }
    }
  },
  "memory_structs": {},
  "stations": {},
  "default_agent_id": "",
  "default_role": "",
  "default_model": { "provider": "deepseek", "model": "deepseek-4-flash" }
}
```

迁移范围（手动改，旧配置不兼容）：
- `script/template/nexus.json`、`workspace/agent-data/nexus.json`：平铺 `models` -> `providers` 嵌套；`endpoint` 全路径 -> `base_url` 前缀；`default_model` 字符串 -> `ProviderModel` 对象。
- `config.json`（root / script / test/workspace 各份）：agent 段 `init_model` 字符串 -> 对象 `{ "provider": ..., "model": ... }`。

## 文件组织与测试

- `main.rs` 新增 `mod provider;`。
- `config_manager.rs` 测试：
  - resolve 合并：model 覆盖优先、缺省继承 provider 默认、provider 不存在、model 不存在、未知 provider_type。
  - init_model（ProviderModel）种子 default_model。
  - channel_id 序列化/反序列化、channels CRUD 按 channel_id。
- `provider.rs` 测试：OpenAi / Anthropic 请求构造与响应解析（本地 mock HTTP server 或直接测请求/解析纯函数）。
- `model_client.rs`：retry 逻辑测试。
- `coordinator.rs` / `command_router.rs`：`/model <provider> <model>` 双参数解析、set_current_model 校验。

## 代码影响清单

| 文件 | 改动 |
|------|------|
| `kissbot-agent/src/config_manager.rs` | ProviderModel / ProviderConfig / ModelConfig 重构，EffectiveModelConfig，resolve_effective_config，NexusRepo.providers，default_model 类型，ChannelConfig.channel_id |
| `kissbot-agent/src/provider.rs` | 新增：Provider trait + OpenAiProvider + AnthropicProvider |
| `kissbot-agent/src/model_client.rs` | 删快照/update_config/协议实现，call(pm, messages) 现场合成 + 构建 Provider |
| `kissbot-agent/src/coordinator.rs` | current_model: ArcSwap<ProviderModel>，set_current_model(pm)，channel_id 连锁 |
| `kissbot-agent/src/command_router.rs` | `/model <provider> <model>`，AdminCommand::Model(ProviderModel) |
| `kissbot-agent/src/types.rs` | `AdminCommand::Model(ProviderModel)`（ProviderModel 定义在 config_manager.rs，types.rs 引用） |
| `kissbot-agent/src/main.rs` | `mod provider;` |
| `script/template/nexus.json`、`workspace/agent-data/nexus.json` | 迁移新格式 |
| `config.json`（root / script / test/workspace） | agent 段 init_model 改对象 |
