# ProviderConfig 抽象层实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 kissbot-agent 中为模型调用增加 ProviderConfig 抽象层：ProviderConfig（provider 级默认参数 + 嵌套 models）、ModelConfig（Option 继承参数）、Provider trait（openai/anthropic 各自实现）、ModelClient 每次调用现场合成 EffectiveModelConfig，并顺带把 ChannelConfig 的 messenger_id 改为 agent 内部唯一标识 channel_id。

**Architecture:** `NexusRepo.providers` 替代 `models`，`ProviderModel { provider, model }` 固定打包出现（函数调用 / current / default）。`ConfigManager::resolve_effective_config(pm)` 合并 provider 默认 + model 覆盖，`ModelClient::call(pm, messages)` 每次现场合成并按 `provider_type` 构建 `Box<dyn Provider>`（OpenAiProvider / AnthropicProvider），重试留在 ModelClient。通道侧 `channel_id` 用于所有 agent 内部索引，消息身份（`messenger_id`）保持消息层语义。

**Tech Stack:** Rust 2024、tokio、reqwest 0.12、async-trait、arc-swap、dashmap、serde。

**Spec:** `docs/superpowers/specs/2026-07-31-provider-config-abstraction-design.md`

## Global Constraints

- 工作目录：`/home/admin/project/kissbot`；组件：`kissbot-agent`
- 不删除代码中的注释；文本 UTF-8、`\n` 换行
- 提交 comment 用中文，包含该提交所有改动内容
- 读写文件必须用 Read/Write/Edit 工具，禁止 sed/python 改文件
- 配置结构体不加 `#[serde(default)]`（字段缺失反序列化报错，延续 c5094b8 风格）；Option 字段用 `#[serde(skip_serializing_if = "Option::is_none")]`
- 每任务结束运行 `cargo test -p kissbot-agent` 与 `cargo build -p kissbot-agent` 通过后提交
- 模板参考：现有 nexus.json（`script/template/nexus.json`、`workspace/agent-data/nexus.json`）与 `config.json`（root / `script/config.json` / `test/workspace/config.json`）

---

### Task 1: 新增配置数据结构（ProviderModel / ProviderConfig / EffectiveModelConfig / NexusRepo.providers / resolve_effective_config）

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`
- Test: `kissbot-agent/src/config_manager.rs`（`#[cfg(test)]` 段）

**Interfaces:**
- Consumes: 现有 `ModelConfig`（旧结构，本任务不改）、`ArcSwapHashMap`（kissbot-api）
- Produces: `ProviderModel { provider: String, model: String }`、`ProviderConfig`、`EffectiveModelConfig`、`NexusRepo.providers`、`ConfigManager::resolve_effective_config(&self, pm: &ProviderModel) -> Option<EffectiveModelConfig>`。后续 Task 2 的 provider.rs、Task 4 的 model_client.rs 依赖这些类型与函数。

**说明：** 本任务纯新增，旧 `ModelConfig` / `NexusRepo.models` 保留不动（Task 4 才删除），保证编译与现有测试通过。`resolve_effective_config` 在本任务先按旧 ModelConfig 的必填字段合并（继承/覆盖逻辑在 Task 4 随 Option 化落地）。

- [ ] **Step 1: 写失败测试**

在 `kissbot-agent/src/config_manager.rs` 的 `#[cfg(test)] mod tests` 内新增两个测试（放在文件末尾 `nexus_repo_default_empty` 之后）：

```rust
    // ---------- Provider 配置 ----------

    fn sample_provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: Arc::new(name.into()),
            provider_type: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "sk-test".into(),
            default_context_length: 65536,
            default_max_tokens: 4096,
            default_temperature: 0.7,
            default_timeout_secs: 60,
            default_retry_count: 3,
            models: Arc::new(ArcSwapHashMap::new()),
        }
    }

    #[tokio::test]
    async fn resolve_effective_config_merges_provider_and_model() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        // 构造 provider + model（Task 1 阶段 ModelConfig 仍为旧结构，字段必填）
        let mut provider = sample_provider("deepseek");
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("deepseek-4-flash".to_string(), ArcSwap::new(Arc::new(ModelConfig {
                name: Arc::new("deepseek-4-flash".into()),
                provider: "openai".into(),
                endpoint: "https://api.deepseek.com".into(),
                api_key: "sk-test".into(),
                model: "deepseek-4-flash".into(),
                max_tokens: 2048,
                temperature: 0.3,
                timeout_secs: 30,
                retry_count: 2,
            })));
        }
        {
            let mut repo = manager.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            map.insert("deepseek".to_string(), ArcSwap::new(Arc::new(provider)));
        }
        let pm = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let eff = manager.resolve_effective_config(&pm).await.expect("应能合成");
        assert_eq!(eff.provider_type, "openai");
        assert_eq!(eff.base_url, "https://api.deepseek.com");
        assert_eq!(eff.api_key, "sk-test");
        assert_eq!(eff.model, "deepseek-4-flash");
        assert_eq!(eff.max_tokens, 2048, "model 的 max_tokens 应生效");
        assert_eq!(eff.temperature, 0.3, "model 的 temperature 应生效");
        assert_eq!(eff.timeout_secs, 30);
        assert_eq!(eff.retry_count, 2);
        assert_eq!(eff.context_length, 65536, "context_length 取 provider 默认");
    }

    #[tokio::test]
    async fn resolve_effective_config_missing_returns_none() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        assert!(manager.resolve_effective_config(&ProviderModel { provider: "nope".into(), model: "m".into() }).await.is_none());
        assert!(manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "nope".into() }).await.is_none());
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent resolve_effective_config 2>&1 | tail -20`
Expected: 编译失败，报 `ProviderModel` / `ProviderConfig` / `EffectiveModelConfig` / `resolve_effective_config` 未定义。

- [ ] **Step 3: 实现**

在 `kissbot-agent/src/config_manager.rs` 中：

(a) 在文件顶部注释附近（`// ========== 配置数据结构 ==========` 段内、旧 `ModelConfig` 定义之前）插入新结构：

```rust
// ========== Provider 配置 ==========

// (provider, model) 固定一起出现：函数调用、current 运行状态、default 配置共用
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

// 合并后的有效配置（provider 默认 + model 覆盖），运行时合成、不持久化
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
```

(b) `NexusRepo` 增加 `providers` 字段（保留旧 `models`，Task 4 删除）：在 `pub models: ...` 行后加：

```rust
    pub providers: Arc<ArcSwapHashMap<String, ProviderConfig>>, // key = provider 名
```

并在 `impl Default for NexusRepo` 中对应加：

```rust
            providers: Arc::new(ArcSwapHashMap::new()),
```

(c) 在 `ConfigManager` 的 `// ---------- models ----------` 段（`model_config_by_name` 之后）新增：

```rust
    // ---------- providers ----------
    /// 合成 provider 默认 + model 配置的有效参数（每次调用现场合成，配置永远最新）
    pub async fn resolve_effective_config(&self, pm: &ProviderModel) -> Option<EffectiveModelConfig> {
        let repo = self.nexus_repo.read().await;
        let provider = repo.providers.get(&pm.provider)?.load_full();
        let model_cfg = provider.models.get(&pm.model)?.load_full();
        Some(EffectiveModelConfig {
            provider_type: provider.provider_type.clone(),
            base_url: provider.base_url.clone(),
            api_key: provider.api_key.clone(),
            model: model_cfg.model.clone(),
            max_tokens: model_cfg.max_tokens,
            temperature: model_cfg.temperature,
            timeout_secs: model_cfg.timeout_secs,
            retry_count: model_cfg.retry_count,
            context_length: provider.default_context_length,
        })
    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent 2>&1 | tail -20`
Expected: 全部 PASS（含原有测试）。

- [ ] **Step 5: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/src/config_manager.rs && git commit -m "feat(agent): 新增 ProviderConfig 抽象层配置结构（ProviderModel/ProviderConfig/EffectiveModelConfig、NexusRepo.providers、resolve_effective_config 合成）
- ProviderModel 打包 (provider, model)，函数调用/current/default 统一携带
- ProviderConfig 含 provider_type/base_url/api_key/default_* 参数与嵌套 models
- EffectiveModelConfig 为运行时合成结果，provider 默认 + model 覆盖
- NexusRepo 新增 providers map（旧 models 保留，后续任务迁移）"
```

---

### Task 2: Provider trait 与实现（provider.rs）+ MessageItem 移入 types.rs

**Files:**
- Create: `kissbot-agent/src/provider.rs`
- Modify: `kissbot-agent/src/types.rs`（新增 `MessageItem`）、`kissbot-agent/src/model_client.rs`（删除 `MessageItem` 定义，改引用）、`kissbot-agent/src/main.rs`（`mod provider;`）
- Test: `kissbot-agent/src/provider.rs`（`#[cfg(test)]` 段）

**Interfaces:**
- Consumes: Task 1 的 `EffectiveModelConfig`；`types::MessageItem`（本任务新增）、`types::ModelResponse`、`types::{Error, Result}`（已存在）
- Produces: `trait Provider: Send + Sync { async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse>; }`、`OpenAiProvider::new(client: Arc<reqwest::Client>, base_url: &str, api_key: &str) -> Self`、`AnthropicProvider::new(...)`。Task 4 的 model_client.rs 依赖 trait 与两实现。

**说明：** `MessageItem` 从 model_client.rs 移到 types.rs（Provider 与 ModelClient 共用）。请求构造与响应解析抽成模块级纯函数便于单测（不依赖真实 HTTP）。

- [ ] **Step 1: 写失败测试（provider.rs 单测）**

创建 `kissbot-agent/src/provider.rs`，先只写测试与空实现骨架：

```rust
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::config_manager::EffectiveModelConfig;
use crate::types::{Error, MessageItem, ModelResponse, Result};

/// Provider 抽象：负责向模型服务商发一次请求并解析响应
#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse>;
}

// ========== OpenAI 兼容协议（/chat/completions） ==========

pub struct OpenAiProvider {
    client: Arc<reqwest::Client>,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(client: Arc<reqwest::Client>, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
        }
    }
}

fn openai_body(effective: &EffectiveModelConfig, messages: &[MessageItem]) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages.iter().map(|m| {
        json!({ "role": m.role, "content": m.content })
    }).collect();
    json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
        "temperature": effective.temperature,
        "stream": false,
    })
}

fn parse_openai_response(data: &serde_json::Value) -> ModelResponse {
    let choice = &data["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("").to_string();
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop").to_string();
    ModelResponse { content, tool_calls: Vec::new(), finish_reason }
}

#[async_trait]
impl Provider for OpenAiProvider {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url)
            .timeout(Duration::from_secs(effective.timeout_secs))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&openai_body(effective, messages))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("OpenAI API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(parse_openai_response(&data))
    }
}

// ========== Anthropic 协议（/v1/messages） ==========

pub struct AnthropicProvider {
    client: Arc<reqwest::Client>,
    base_url: String,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(client: Arc<reqwest::Client>, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
        }
    }
}

fn anthropic_body(effective: &EffectiveModelConfig, messages: &[MessageItem]) -> serde_json::Value {
    // 分离 system 消息
    let system_parts: Vec<String> = messages.iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.clone())
        .collect();
    let system = system_parts.join("\n");

    let msgs: Vec<serde_json::Value> = messages.iter()
        .filter(|m| m.role != "system")
        .map(|m| json!({
            "role": if m.role == "assistant" { "assistant" } else { "user" },
            "content": m.content,
        }))
        .collect();

    let mut body = json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    body
}

fn parse_anthropic_response(data: &serde_json::Value) -> ModelResponse {
    let content = data["content"][0]["text"].as_str().unwrap_or("").to_string();
    let finish_reason = data["stop_reason"].as_str().unwrap_or("end_turn").to_string();
    ModelResponse { content, tool_calls: Vec::new(), finish_reason }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url)
            .timeout(Duration::from_secs(effective.timeout_secs))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&anthropic_body(effective, messages))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("Anthropic API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(parse_anthropic_response(&data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_effective() -> EffectiveModelConfig {
        EffectiveModelConfig {
            provider_type: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "sk-test".into(),
            model: "deepseek-4-flash".into(),
            max_tokens: 2048,
            temperature: 0.3,
            timeout_secs: 30,
            retry_count: 2,
            context_length: 65536,
        }
    }

    #[test]
    fn openai_body_includes_params_and_messages() {
        let eff = sample_effective();
        let msgs = vec![
            MessageItem { role: "system".into(), content: "你是助手".into() },
            MessageItem { role: "user".into(), content: "你好".into() },
        ];
        let body = openai_body(&eff, &msgs);
        assert_eq!(body["model"], "deepseek-4-flash");
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["temperature"], 0.3);
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "你好");
    }

    #[test]
    fn parse_openai_response_extracts_content_and_finish_reason() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content, "答案");
        assert_eq!(resp.finish_reason, "stop");
    }

    #[test]
    fn anthropic_body_separates_system_messages() {
        let eff = sample_effective();
        let msgs = vec![
            MessageItem { role: "system".into(), content: "设定".into() },
            MessageItem { role: "user".into(), content: "hi".into() },
        ];
        let body = anthropic_body(&eff, &msgs);
        assert_eq!(body["system"], "设定");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1, "system 不应出现在 messages");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn parse_anthropic_response_extracts_text_and_stop_reason() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "答复" }],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content, "答复");
        assert_eq!(resp.finish_reason, "end_turn");
    }
}
```

- [ ] **Step 2: 同步 MessageItem 迁移与 mod 声明（让编译通过）**

(a) `kissbot-agent/src/types.rs`：在 `// ========== 模型相关 ==========` 段、`ModelResponse` 定义之后新增：

```rust
/// 模型上下文中的单条消息
#[derive(Debug, Clone)]
pub struct MessageItem {
    pub role: String,
    pub content: String,
}
```

(b) `kissbot-agent/src/model_client.rs`：删除文件末尾的 `MessageItem` 定义（`/// 模型上下文中的单条消息` 到 `}` 块），并确认 `use crate::types::{ModelResponse, Result, Error};` 存在（若原 import 只有 `ModelResponse, Result, Error`，保持不变，因为 `MessageItem` 在本文件仍被 `call` 系列使用——改为在文件顶部加 `use crate::types::MessageItem;`）。

(c) `kissbot-agent/src/main.rs`：在 `mod model_client;` 行后加 `mod provider;`。

- [ ] **Step 3: 运行测试确认失败/编译**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent provider 2>&1 | tail -20`
Expected: 本任务实现已含测试与实现，应直接通过；如 model_client.rs 中 `MessageItem` 引用报未定义，检查 (b) 的 import 是否正确。

（说明：本任务实现与测试同写。若测试阶段出现编译错误，按错误修正后再进入 Step 4。）

- [ ] **Step 4: 运行全部测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent 2>&1 | tail -20`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/src/provider.rs kissbot-agent/src/types.rs kissbot-agent/src/model_client.rs kissbot-agent/src/main.rs && git commit -m "feat(agent): 新增 Provider trait 与 OpenAi/Anthropic 实现，MessageItem 移入 types.rs
- Provider trait 提供 send(effective, messages)，协议差异封装在各自实现
- OpenAiProvider：POST {base_url}/chat/completions，Bearer 认证
- AnthropicProvider：POST {base_url}/v1/messages，x-api-key + anthropic-version，system 分离
- 请求构造/响应解析抽为纯函数便于单测；per-request timeout 按 effective 设置"
```

---

### Task 3: ChannelConfig 去 messenger_id，改用 agent 内部唯一标识 channel_id

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`、`kissbot-agent/src/coordinator.rs`、`kissbot-agent/src/command_router.rs`
- Test: `kissbot-agent/src/config_manager.rs`、`kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: 现有 `ChannelUser { messenger_id, user_id }`（消息层身份，保留）
- Produces: `ChannelConfig { channel_id, ws_url, admins, default_bind_user, enabled_by_default }`（channels map key = channel_id）；`ConfigManager::channel_ws_url(channel_id)`、`add_channel(ch)`、`remove_channel(channel_id)`、`add_admin(channel_id, admin: &ChannelUser)`、`remove_admin(channel_id, user_id)`；`AgentCoordinator::bind_channel(channel_id, binding)`、`unbind_channel(channel_id)`、`set_current_model`（不动）；`CommandRouter::execute(command, config, coordinator, channel_id: &str)`

**说明：** `channel_id` 是 agent 内部唯一标识，与消息方 messenger 无关（同一 messenger 可对应多个 channel）。所有 agent 内部索引（`bound_channels` / `channel_clients` / `disconnect_notify` / `ChannelClient::new` 的 id）改用 `channel_id`；消息过滤用 `incoming_message` 回调的 id 参数（= channel_id）。消息身份（`incoming.messenger_id` 用于 `check_admin`、`OutgoingMessage.messenger_id`）保持不变。

- [ ] **Step 1: 写失败测试（config_manager）**

更新 `kissbot-agent/src/config_manager.rs` 现有测试 `add_remove_admin_missing_channel_errors`（channel 定位参数从 messenger_id 改为 channel_id）：

```rust
    #[tokio::test]
    async fn add_remove_admin_missing_channel_errors() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        // channel 不存在：add_admin / remove_admin 都应返回 ConfigNotFound 而非静默成功
        let admin = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
        let err = manager.add_admin("nope", &admin).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        let err = manager.remove_admin("nope", "u1").await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
    }
```

同时更新 `coordinator.rs` 现有测试 `bound_channels_init_logic`（`messenger_id` 字段 → `channel_id`）：

```rust
    #[test]
    fn bound_channels_init_logic() {
        // 模拟三个 channel：
        // 1) c1: enabled + default_bind_user 非空 → 连接且绑定
        // 2) c3: enabled + default_bind_user 为空 → 连接但不绑定（消息被过滤直到运行时 /bind）
        // 3) c2: disabled + default_bind_user 非空 → 不连接也不绑定
        let enabled_with_bind = ChannelConfig {
            channel_id: Arc::new("c1".into()), ws_url: Arc::new("ws://x".into()),
            admins: Arc::new(HashSet::new()),
            default_bind_user: Some(ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) }),
            enabled_by_default: true,
        };
        let enabled_no_bind = ChannelConfig {
            channel_id: Arc::new("c3".into()),
            default_bind_user: None, ..enabled_with_bind.clone()
        };
        let disabled_with_bind = ChannelConfig {
            channel_id: Arc::new("c2".into()), enabled_by_default: false, ..enabled_with_bind.clone()
        };
        let all = vec![
            (enabled_with_bind.channel_id.to_string(), Arc::new(enabled_with_bind.clone())),
            (enabled_no_bind.channel_id.to_string(), Arc::new(enabled_no_bind.clone())),
            (disabled_with_bind.channel_id.to_string(), Arc::new(disabled_with_bind.clone())),
        ];

        // 绑定集合：仅 enabled_by_default 且 default_bind_user 非空的 channel（key = channel_id）
        let bound = AgentCoordinator::bound_channels_from_channels(all);
        assert_eq!(bound.len(), 1, "只有 c1 应入绑定集合");
        let entry = bound.get("c1").unwrap();
        assert_eq!(*entry.value().messenger_id, "m1");
        assert_eq!(*entry.value().user_id, "u1");
        assert!(!bound.contains_key("c3"), "enabled 无 default_bind_user 不应入绑定集合");
        assert!(!bound.contains_key("c2"), "disabled 不应入绑定集合");

        // 连接集合：enabled_by_default 控制，与绑定无关
        let connect_set: Vec<String> = [
            enabled_with_bind, enabled_no_bind, disabled_with_bind,
        ].iter().filter(|c| c.enabled_by_default)
            .map(|c| c.channel_id.to_string())
            .collect();
        assert!(connect_set.contains(&"c1".to_string()), "c1 应连接");
        assert!(connect_set.contains(&"c3".to_string()), "c3 应连接（仅连接不绑定）");
        assert!(!connect_set.contains(&"c2".to_string()), "c2 不应连接");
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent 2>&1 | tail -20`
Expected: 编译失败，报 `ChannelConfig` 无 `messenger_id` 字段或 `add_admin` 签名不匹配。

- [ ] **Step 3: 实现（config_manager.rs）**

(a) `ChannelConfig` 结构（`messenger_id` 字段改为 `channel_id`）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_id: Arc<String>,         // agent 内部唯一标识，与消息方 messenger 无关
    pub ws_url: Arc<String>,
    pub admins: Arc<HashSet<ChannelUser>>,
    pub default_bind_user: Option<ChannelUser>,
    pub enabled_by_default: bool,
}
```

(b) channels CRUD 段（约 242-270 行）改为按 `channel_id`：

```rust
    // ---------- channels ----------
    /// 返回所有 channel 配置快照（channel_id -> Arc<ChannelConfig>）
    #[allow(dead_code)]
    pub async fn channels(&self) -> Vec<(String, Arc<ChannelConfig>)> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter().map(|(k, v)| (k.clone(), v.load().clone())).collect()
    }
    #[allow(dead_code)]
    pub async fn channel_ws_url(&self, channel_id: &str) -> Option<String> {
        let repo = self.nexus_repo.read().await;
        repo.channels.get(channel_id).map(|s| s.load().ws_url.to_string())
    }
    #[allow(dead_code)]
    pub async fn add_channel(&self, ch: ChannelConfig) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.channels);
            map.insert(ch.channel_id.to_string(), ArcSwap::new(Arc::new(ch)));
        }
        self.save_nexus().await
    }
    #[allow(dead_code)]
    pub async fn remove_channel(&self, channel_id: &str) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.channels);
            map.remove(channel_id);
        }
        self.save_nexus().await
    }
```

(c) admins 段（约 311-336 行）改为按 `channel_id` 定位：

```rust
    /// 添加管理权限（回写 NexusRepo；channel 不存在则报错）
    pub async fn add_admin(&self, channel_id: &str, admin: &ChannelUser) -> Result<()> {
        {
            let repo = self.nexus_repo.write().await;
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load().clone();
            let ch_mut = Arc::make_mut(&mut ch);
            Arc::make_mut(&mut ch_mut.admins).insert(admin.clone());
            swap.store(ch);
        }
        self.save_nexus().await
    }
    /// 移除管理权限（回写 NexusRepo；channel 不存在则报错）
    pub async fn remove_admin(&self, channel_id: &str, user_id: &str) -> Result<()> {
        {
            let repo = self.nexus_repo.write().await;
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load().clone();
            let ch_mut = Arc::make_mut(&mut ch);
            let target = ChannelUser { messenger_id: Arc::new(user_id.into()), user_id: Arc::new(user_id.into()) };
            Arc::make_mut(&mut ch_mut.admins).remove(&target);
            swap.store(ch);
        }
        self.save_nexus().await
    }
```

（注：`remove_admin` 中 `ChannelUser` 需 `Hash/Eq` 匹配集合内元素，`messenger_id` 为消息层身份；此处按现有语义用 user_id 构造 target，保持行为与原来一致——原代码也是 `ChannelUser { messenger_id: Arc::new(user_id.into()), user_id: ... }`。**保留原注释风格，不要删除已有注释。**）

- [ ] **Step 4: 实现（coordinator.rs + command_router.rs）**

(a) `coordinator.rs` 顶部 import 不变（`ChannelConfig, ChannelUser` 已导入）。

(b) `bound_channels_from_channels`（约 173-184 行）：

```rust
    /// 计算启动时绑定集合：enabled_by_default 且 default_bind_user 非空的 channel
    /// （连接由 connect_channels 按 enabled_by_default 独立控制，此处只算绑定集合）
    /// 索引 key 为 agent 内部 channel_id（与消息方 messenger 无关）
    fn bound_channels_from_channels(
        channels: Vec<(String, Arc<ChannelConfig>)>,
    ) -> DashMap<String, Arc<ChannelUser>> {
        let map = DashMap::new();
        for (_, ch) in channels {
            if ch.enabled_by_default {
                if let Some(bu) = &ch.default_bind_user {
                    map.insert(ch.channel_id.to_string(), Arc::new(bu.clone()));
                }
            }
        }
        map
    }
```

(c) `connect_channels`（约 186-240 行）：`ch.messenger_id.to_string()` 全部改为 `ch.channel_id.to_string()`（`messenger_id` 局部变量改名为 `channel_id`，涉及 `ChannelClient::new`、`bound_channels.get`、`disconnect_notify`、`channel_clients`、`BindRequest { messenger_id }`——`BindRequest.messenger_id` 字段承载 agent 内部 channel 标识，代码注释说明）：

```rust
            let channel_id = ch.channel_id.to_string();
            let ws_url = ch.ws_url.to_string();
            // 绑定身份来自运行状态 bound_channels；不在绑定集合则仅连接不绑定
            let bound_user_id = self.bound_channels.get(&channel_id)
                .map(|e| e.value().user_id.to_string());

            let client = ChannelClient::new(
                channel_id.clone(),
                Arc::downgrade(&(coordinator.clone() as Arc<dyn Terminal>)),
            );

            // 断线通知
            let notify = Arc::new(tokio::sync::Notify::new());
            coordinator.disconnect_notify.insert(channel_id.clone(), notify.clone());
            coordinator.channel_clients.insert(channel_id.clone(), client);

            let client_clone = coordinator.channel_clients.get(&channel_id).unwrap().clone();
            let api_key = api_key.clone();

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
                        Ok(()) => {
                            info!("已连接 channel: {}", channel_id);
                            // 绑定用户（仅 bound_channels 中存在的 channel 发送绑定请求）
                            // BindRequest.messenger_id 承载 agent 内部 channel_id（与消息方 messenger 无强绑定）
                            if let Some(user_id) = &bound_user_id {
                                let _ = client_clone.bind(BindRequest {
                                    messenger_id: Arc::new(channel_id.clone()),
                                    user_id: Arc::new(user_id.clone()),
                                }).await;
                            }
                            // 等待断线通知（closed() 回调中 notify_one）
                            notify.notified().await;
                        }
                        Err(e) => {
                            warn!("连接 channel {} 失败: {:?}，{}秒后重连", channel_id, e, reconnect_secs);
                            tokio::time::sleep(Duration::from_secs(reconnect_secs)).await;
                        }
                    }
                }
            });
```

(d) `bind_channel` / `unbind_channel`（约 160-168 行）改为按 channel_id 索引：

```rust
    /// 绑定 channel 用户（仅运行状态，不回写）；key 为 agent 内部 channel_id
    pub async fn bind_channel(&self, channel_id: &str, binding: ChannelUser) {
        self.bound_channels.insert(channel_id.to_string(), Arc::new(binding));
    }

    /// 解绑 channel（仅运行状态，不回写）
    pub async fn unbind_channel(&self, channel_id: &str) {
        self.bound_channels.remove(channel_id);
    }
```

(e) `Terminal` 实现（约 262-289 行）：`incoming_message` 的 `_id` 参数改名为 `channel_id` 并下传；`handle_incoming`、`run_agentic_loop`、`handle_admin_command`、`send_reply` 签名与内部按 channel_id 索引：

```rust
    /// 收到上行消息
    async fn incoming_message(&self, channel_id: &str, message: Arc<IncomingMessage>) {
        // 1. 推上行消息到记忆（使用 Arc 引用避免深复制）
        let agent_id = Arc::new(self.current_agent_id());
        let role_name = Arc::new(self.current_role());
        self.memory_store_client.push_channel_record(ChannelRecord {
            agent_id,
            role_name,
            messenger_id: message.messenger_id.clone(),
            user_id: message.user_id.clone(),
            group_id: message.group_id.clone(),
            is_self: message.is_self,
            content: message.content.clone(),
            time: message.time.clone(),
        }).await;

        // 2. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
        self.handle_incoming(channel_id, message).await;
    }
```

`handle_incoming`（约 306-336 行）：

```rust
    async fn handle_incoming(&self, channel_id: &str, incoming: Arc<IncomingMessage>) {
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let is_self = incoming.is_self;
        let content_text = extract_text(&incoming.content);

        // 1. 检查 channel 是否在绑定范围内（key = agent 内部 channel_id）
        if !self.bound_channels.contains_key(channel_id) {
            return; // 非绑定 channel 的消息丢弃
        }

        // 2. 检查 is_self
        if is_self == 1 {
            let ctx = self.context_builder.lock().await;
            if ctx.is_self_echo(&content_text) {
                return; // 自己发出的回显，丢弃
            }
            return;
        }

        // 3. 检查管理命令
        if CommandRouter::is_command(&content_text) {
            if CommandRouter::check_admin(&self.config, &messenger_id, &user_id).await {
                self.handle_admin_command(channel_id, &content_text, &messenger_id, &user_id, &group_id).await;
            }
            // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
            return;
        }

        // 4. 普通消息 → agentic loop
        self.run_agentic_loop(channel_id, incoming).await;
    }
```

`handle_admin_command`（约 338-373 行）：签名加 `channel_id: &str`，内部 `send_reply` 调用改为 `self.send_reply(channel_id, messenger_id, user_id, group_id, reply).await;`，`CommandRouter::execute(&cmd, &self.config, self, channel_id)`：

```rust
    async fn handle_admin_command(
        &self,
        channel_id: &str,
        content: &str,
        messenger_id: &str,
        user_id: &str,
        group_id: &str,
    ) {
        match CommandRouter::parse(content) {
            Ok(cmd) => {
                match CommandRouter::execute(&cmd, &self.config, self, channel_id).await {
                    Ok((reply, cmd_needs_reset)) => {
                        self.send_reply(channel_id, messenger_id, user_id, group_id, reply).await;

                        // 处理需要触发上下文重建的命令
                        if cmd_needs_reset {
                            match &cmd {
                                AdminCommand::ModeEvent(event_id) => {
                                    let eid = event_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                                    self.set_current_mode(Mode::Event(eid.clone()));
                                    self.reset_context().await;
                                }
                                AdminCommand::ModeRole => {
                                    self.set_current_mode(Mode::Role);
                                    self.reset_context().await;
                                }
                                AdminCommand::Reenter(event_id) => {
                                    self.set_current_mode(Mode::Event(event_id.clone()));
                                    self.reset_context().await;
                                }
                                AdminCommand::Reset => {
                                    self.reset_context().await;
                                }
                                // SetRole / Agent 已在 coordinator setter 内重建上下文，此处不重复 reset
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        self.send_reply(channel_id, messenger_id, user_id, group_id,
                            format!("❌ 命令执行失败: {}", e)).await;
                    }
                }
            }
            Err(e) => {
                self.send_reply(channel_id, messenger_id, user_id, group_id,
                    format!("⚠️ {}", e)).await;
            }
        }
    }
```

`run_agentic_loop`（约 375 行起）签名加 `channel_id: &str`，第 5 步 `self.send_reply(channel_id, &messenger_id, &user_id, &group_id, model_resp.content).await;`；错误分支同样传 channel_id：

```rust
    async fn run_agentic_loop(&self, channel_id: &str, incoming: Arc<IncomingMessage>) {
        // ... 1-4 步不变 ...
        // 5. 发送回复到通道
        self.send_reply(channel_id, &messenger_id, &user_id, &group_id, model_resp.content).await;
        // ... 6. 检查上下文超长 不变 ...
    }
```

`send_reply`（约 457 行起）签名改为 `(channel_id, messenger_id, user_id, group_id, content)`，`channel_clients.get(channel_id)`，`OutgoingMessage.messenger_id` 保持消息层 messenger_id：

```rust
    /// 发送回复消息到通道，成功后推记忆（is_self=1）
    /// channel_id 定位连接（agent 内部标识），messenger_id 为消息方身份（回复目标）
    async fn send_reply(&self, channel_id: &str, messenger_id: &str, user_id: &str, group_id: &str, content: String) {
        let Some(client) = self.channel_clients.get(channel_id) else {
            warn!("send_reply: 未找到 channel client: {}", channel_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: Arc::new(messenger_id.to_string()),
            user_id: Arc::new(user_id.to_string()),
            group_id: Arc::new(group_id.to_string()),
            content: Content::Text(Arc::new(content.clone())),
        };

        match client.send_message(msg).await {
            Ok(response) => {
                // 下行成功后推记忆（is_self=1，使用返回的 content）
                let agent_id = Arc::new(self.current_agent_id());
                let role_name = Arc::new(self.current_role());
                self.memory_store_client.push_channel_record(ChannelRecord {
                    agent_id,
                    role_name,
                    messenger_id: Arc::new(messenger_id.to_string()),
                    user_id: Arc::new(user_id.to_string()),
                    group_id: Arc::new(group_id.to_string()),
                    is_self: 1,
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;

                // 记录已发送内容（用于 is_self echo 检测）
                let mut ctx = self.context_builder.lock().await;
                ctx.record_sent_content(content);
            }
            Err(e) => {
                warn!("send_reply 失败: {:?}", e);
            }
        }
    }
```

(f) `command_router.rs`：`execute` 签名加 `channel_id: &str`，Bind/Unbind/Admin/Unadmin 分支改为传 channel_id：

```rust
    pub async fn execute(
        command: &AdminCommand,
        config: &ConfigManager,
        coordinator: &AgentCoordinator,
        channel_id: &str,
    ) -> Result<(String, bool)> {
        match command {
            AdminCommand::Bind { messenger_id, user_id } => {
                coordinator.bind_channel(channel_id, ChannelUser {
                    messenger_id: Arc::new(messenger_id.clone()),
                    user_id: Arc::new(user_id.clone()),
                }).await;
                Ok((format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id), false))
            }
            AdminCommand::Unbind { messenger_id } => {
                coordinator.unbind_channel(channel_id).await;
                Ok((format!("✅ 已解绑 channel: {}", messenger_id), false))
            }
            AdminCommand::Admin { messenger_id, user_id } => {
                config.add_admin(channel_id, &ChannelUser {
                    messenger_id: Arc::new(messenger_id.clone()),
                    user_id: Arc::new(user_id.clone()),
                }).await?;
                Ok((format!("✅ 已添加管理权限: {} / {}", messenger_id, user_id), false))
            }
            AdminCommand::Unadmin { messenger_id, user_id } => {
                config.remove_admin(channel_id, user_id).await?;
                Ok((format!("✅ 已移除管理权限: {} / {}", messenger_id, user_id), false))
            }
            // ... 其余分支不变 ...
        }
    }
```

（其余分支 `SetRole` / `Model` / `Agent` / `ModeEvent` / `ModeRole` / `Reenter` / `Events` / `Reset` 保持原样，仅签名多一个 `channel_id` 参数。）

- [ ] **Step 5: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent 2>&1 | tail -20`
Expected: 全部 PASS。

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/src/config_manager.rs kissbot-agent/src/coordinator.rs kissbot-agent/src/command_router.rs && git commit -m "refactor(agent): ChannelConfig 去 messenger_id，改用 agent 内部唯一标识 channel_id
- channels map key 与 ChannelConfig.channel_id 对齐（与消息方 messenger 无强绑定）
- coordinator 索引（bound_channels/channel_clients/disconnect_notify/ChannelClient id）改用 channel_id
- 消息过滤用 incoming_message 回调 id（= channel_id），消息身份仍用 messenger_id
- add_admin/remove_admin/bind/unbind/execute 增加 channel_id 定位参数"
```

---

### Task 4: 模型链路切换到新抽象（删旧 ModelConfig、ModelClient 重构、coordinator 运行状态、/model 双参数）

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`、`kissbot-agent/src/model_client.rs`、`kissbot-agent/src/coordinator.rs`、`kissbot-agent/src/command_router.rs`、`kissbot-agent/src/types.rs`
- Test: 各文件 `#[cfg(test)]` 段

**Interfaces:**
- Consumes: Task 1 的 `ProviderModel`/`ProviderConfig`/`EffectiveModelConfig`/`resolve_effective_config`；Task 2 的 `Provider` trait、`OpenAiProvider`、`AnthropicProvider`、`types::MessageItem`
- Produces: 新 `ModelConfig { model, max_tokens: Option, temperature: Option, timeout_secs: Option, retry_count: Option, context_length: Option }`；`NexusRepo` 无 `models` 字段、`default_model: Arc<ProviderModel>`；`AgentConfig.init_model: Arc<ProviderModel>`；`ConfigManager::resolve_effective_config`（Option 覆盖语义）；`ModelClient::new(config_manager: Arc<ConfigManager>) -> Self`、`ModelClient::call(&self, pm: &ProviderModel, messages: &[MessageItem]) -> Result<ModelResponse>`；`AgentCoordinator.current_model: ArcSwap<ProviderModel>`、`current_model() -> ProviderModel`、`set_current_model(pm: ProviderModel) -> Result<()>`；`AdminCommand::Model(ProviderModel)`；`/model <provider> <model>`

**说明：** 本任务为原子切换（旧结构删除与新消费方必须同步，否则编译失败）。旧 `ModelConfig` 及其 `Default`、`NexusRepo.models`、`model_config_by_name`、`add_model`、`ModelClient::update_config` / `call_openai` / `call_anthropic` 全部删除。

- [ ] **Step 1: 写失败测试（config_manager resolve 覆盖语义）**

在 `config_manager.rs` 测试段，把 Task 1 的 `resolve_effective_config_merges_provider_and_model` 改为 Option 覆盖语义（ModelConfig 字段全为 Option），并新增缺省继承测试：

```rust
    fn sample_provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: Arc::new(name.into()),
            provider_type: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "sk-test".into(),
            default_context_length: 65536,
            default_max_tokens: 4096,
            default_temperature: 0.7,
            default_timeout_secs: 60,
            default_retry_count: 3,
            models: Arc::new(ArcSwapHashMap::new()),
        }
    }

    #[tokio::test]
    async fn resolve_effective_config_merges_provider_and_model() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        let mut provider = sample_provider("deepseek");
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("deepseek-4-flash".to_string(), ArcSwap::new(Arc::new(ModelConfig {
                model: "deepseek-4-flash".into(),
                max_tokens: Some(2048),
                temperature: Some(0.3),
                timeout_secs: Some(30),
                retry_count: Some(2),
                context_length: None,  // 未配 → 继承 provider 默认
            })));
        }
        {
            let mut repo = manager.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            map.insert("deepseek".to_string(), ArcSwap::new(Arc::new(provider)));
        }
        let pm = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let eff = manager.resolve_effective_config(&pm).await.expect("应能合成");
        assert_eq!(eff.provider_type, "openai");
        assert_eq!(eff.base_url, "https://api.deepseek.com");
        assert_eq!(eff.api_key, "sk-test");
        assert_eq!(eff.model, "deepseek-4-flash");
        assert_eq!(eff.max_tokens, 2048, "model 覆盖应生效");
        assert_eq!(eff.temperature, 0.3);
        assert_eq!(eff.timeout_secs, 30);
        assert_eq!(eff.retry_count, 2);
        assert_eq!(eff.context_length, 65536, "context_length 未配应继承 provider 默认");
    }

    #[tokio::test]
    async fn resolve_effective_config_inherits_provider_defaults() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        let mut provider = sample_provider("deepseek");
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("deepseek-4-flash".to_string(), ArcSwap::new(Arc::new(ModelConfig {
                model: "deepseek-4-flash".into(),
                max_tokens: None,
                temperature: None,
                timeout_secs: None,
                retry_count: None,
                context_length: Some(131072),  // 覆盖 context_length
            })));
        }
        {
            let mut repo = manager.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            map.insert("deepseek".to_string(), ArcSwap::new(Arc::new(provider)));
        }
        let pm = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let eff = manager.resolve_effective_config(&pm).await.expect("应能合成");
        assert_eq!(eff.max_tokens, 4096, "缺省继承 provider 默认");
        assert_eq!(eff.temperature, 0.7);
        assert_eq!(eff.timeout_secs, 60);
        assert_eq!(eff.retry_count, 3);
        assert_eq!(eff.context_length, 131072, "model 覆盖 context_length 应生效");
    }
```

同时更新现有测试：

- `agent_config()` helper：`init_model: Arc::new(ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() })`
- `bootstrap_creates_nexus_with_seeds`：`assert_eq!(*repo.default_model, ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() })`（`ProviderModel` 需 `PartialEq`——见 Step 2 实现中为其派生 `PartialEq`）
- `nexus_repo_serde_roundtrip`：`models:` 字段改为 `providers: Arc::new(ArcSwapHashMap::new())`，`default_model: Arc::new(ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() })`，断言改为比较 `ProviderModel` 字段
- `nexus_repo_default_empty`：`repo.models.is_empty()` 改为 `repo.providers.is_empty()`

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent 2>&1 | tail -20`
Expected: 编译失败（旧 ModelConfig 字段仍在、新结构未实现）。本任务实现步骤较多，可先完成 Step 3 全部实现后再跑本步验证失败转成功。

- [ ] **Step 3: 实现（config_manager.rs）**

(a) `ProviderModel` 派生 `PartialEq`（测试断言需要）：

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
}
```

(b) 旧 `ModelConfig` 整体替换为新结构（删除旧字段与 `impl Default`；保留"// ModelConfig 定义在本文件..."注释并更新）：

```rust
// ModelConfig 定义在本文件，供 model_client 与本文件的 NexusRepo.providers[].models 共用
// 字段均为可继承参数（Option），不配时使用所属 ProviderConfig 的 default_* 值
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model: String,                   // 与所属 provider 的 models map key 相同
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}
```

(c) `NexusRepo`：删除 `models` 字段与 Default 中对应行；`default_model: Arc<String>` → `Arc<ProviderModel>`：

```rust
pub struct NexusRepo {
    pub channels: Arc<ArcSwapHashMap<String, ChannelConfig>>,   // key = channel_id
    pub providers: Arc<ArcSwapHashMap<String, ProviderConfig>>, // key = provider 名
    pub memory_structs: Arc<ArcSwapHashMap<String, MemoryStructConfig>>,
    // nexus 可对接的 station 列表
    pub stations: Arc<ArcSwapHashMap<String, StationConfig>>,
    pub default_agent_id: Arc<String>,
    pub default_role: Arc<String>,
    pub default_model: Arc<ProviderModel>,   // (provider, model) 打包
}
```

（Default 中 `default_model: Arc::new(ProviderModel { provider: String::new(), model: String::new() })`；`models` 行删除。）

(d) `AgentConfig`：`init_model: Arc<String>` → `Arc<ProviderModel>`：

```rust
    pub init_model: Arc<ProviderModel>,   // 种子 NexusRepo.default_model（(provider, model) 打包）
```

(e) 删除 `model_config_by_name`（models 段）与 `add_model`；`resolve_effective_config` 更新为 Option 覆盖语义：

```rust
    // ---------- providers ----------
    /// 合成 provider 默认 + model 覆盖的有效参数（每次调用现场合成，配置永远最新）
    pub async fn resolve_effective_config(&self, pm: &ProviderModel) -> Option<EffectiveModelConfig> {
        let repo = self.nexus_repo.read().await;
        let provider = repo.providers.get(&pm.provider)?.load_full();
        let model_cfg = provider.models.get(&pm.model)?.load_full();
        Some(EffectiveModelConfig {
            provider_type: provider.provider_type.clone(),
            base_url: provider.base_url.clone(),
            api_key: provider.api_key.clone(),
            model: model_cfg.model.clone(),
            max_tokens: model_cfg.max_tokens.unwrap_or(provider.default_max_tokens),
            temperature: model_cfg.temperature.unwrap_or(provider.default_temperature),
            timeout_secs: model_cfg.timeout_secs.unwrap_or(provider.default_timeout_secs),
            retry_count: model_cfg.retry_count.unwrap_or(provider.default_retry_count),
            context_length: model_cfg.context_length.unwrap_or(provider.default_context_length),
        })
    }
```

（`load_or_create_nexus` 中 `default_model: cfg.init_model.clone()` 不变——类型已是 `Arc<ProviderModel>`。）

- [ ] **Step 4: 实现（types.rs + command_router.rs）**

(a) `types.rs`：`AdminCommand::Model(String)` → `AdminCommand::Model(ProviderModel)`，文件顶部 import `crate::config_manager::ProviderModel`：

```rust
use crate::config_manager::ProviderModel;
```

```rust
    Model(ProviderModel),   // /model <provider> <model>
```

(b) `command_router.rs`：`"model"` 分支解析双参数：

```rust
            "model" => {
                if parts.len() != 3 {
                    return Err(Error::InvalidCommand("格式: /model <provider> <model>".to_string()));
                }
                Ok(AdminCommand::Model(ProviderModel {
                    provider: parts[1].to_string(),
                    model: parts[2].to_string(),
                }))
            }
```

`execute` 的 Model 分支（channel_id 参数已在 Task 3 加入签名）：

```rust
            AdminCommand::Model(pm) => {
                coordinator.set_current_model(pm.clone()).await?;
                Ok((format!("✅ 已切换模型为: {}/{}", pm.provider, pm.model), false))
            }
```

文件顶部 import 加 `use crate::config_manager::ProviderModel;`（`ChannelUser` 已有）。

- [ ] **Step 5: 实现（model_client.rs 全量重构）**

`kissbot-agent/src/model_client.rs` 整体替换为：

```rust
use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use crate::config_manager::{ConfigManager, EffectiveModelConfig, ProviderModel};
use crate::provider::{AnthropicProvider, OpenAiProvider, Provider};
use crate::types::{Error, MessageItem, ModelResponse, Result};

pub struct ModelClient {
    config_manager: Arc<ConfigManager>,
    client: Arc<reqwest::Client>,
}

impl ModelClient {
    pub fn new(config_manager: Arc<ConfigManager>) -> Self {
        let client = Arc::new(reqwest::Client::new());
        Self { config_manager, client }
    }

    /// 调用模型 API（非流式）
    /// 每次调用经 ConfigManager 现场合成最新 EffectiveModelConfig（配置永远最新，无需热更新），
    /// 并按 provider_type 构建对应 Provider 实现（未知类型报错）。
    pub async fn call(&self, pm: &ProviderModel, messages: &[MessageItem]) -> Result<ModelResponse> {
        let effective = self.config_manager.resolve_effective_config(pm).await
            .ok_or_else(|| Error::ModelProviderNotSupported(format!(
                "provider/model 不存在: {}/{}", pm.provider, pm.model)))?;
        let provider: Box<dyn Provider> = self.build_provider(&effective);
        self.call_with_retry(&effective, provider, messages).await
    }

    /// 按 provider_type 构建 Provider 实现（protocol 差异封装在 provider.rs）
    fn build_provider(&self, effective: &EffectiveModelConfig) -> Box<dyn Provider> {
        match effective.provider_type.as_str() {
            "openai" => Box::new(OpenAiProvider::new(self.client.clone(), &effective.base_url, &effective.api_key)),
            "anthropic" => Box::new(AnthropicProvider::new(self.client.clone(), &effective.base_url, &effective.api_key)),
            other => Box::new(UnsupportedProvider { provider_type: other.to_string() }),
        }
    }

    /// 指数退避重试（retry_count 来自有效配置）
    async fn call_with_retry(
        &self,
        effective: &EffectiveModelConfig,
        provider: Box<dyn Provider>,
        messages: &[MessageItem],
    ) -> Result<ModelResponse> {
        let max_retries = effective.retry_count;
        let mut last_error = None;

        for attempt in 0..=max_retries {
            match provider.send(effective, messages).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        sleep(Duration::from_secs(1u64 << attempt)).await; // 指数退避
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::ModelApiError("模型调用失败".to_string())))
    }
}

/// 未知 provider_type 时的占位实现（调用即报错）
struct UnsupportedProvider {
    provider_type: String,
}

#[async_trait::async_trait]
impl Provider for UnsupportedProvider {
    async fn send(&self, _effective: &EffectiveModelConfig, _messages: &[MessageItem]) -> Result<ModelResponse> {
        Err(Error::ModelProviderNotSupported(self.provider_type.clone()))
    }
}
```

（注：`UnsupportedProvider` 让"未知 provider_type"在 send 时报错而非构建时 panic，重试语义统一。原 `MessageItem` 定义已移至 types.rs（Task 2），本文件不再定义。）

- [ ] **Step 6: 实现（coordinator.rs 模型部分）**

(a) 字段与 import：`use crate::config_manager::{ConfigManager, ProviderModel};`（删除 `ModelConfig` import）：

```rust
    /// 运行状态：当前 agent / 角色 / 模型 / 模式（启动从 NexusRepo 默认值初始化，运行期不回写）
    current_agent_id: ArcSwap<String>,
    current_role: ArcSwap<String>,
    current_model: ArcSwap<ProviderModel>,   // (provider, model) 打包
    current_mode: ArcSwap<Mode>,
```

(b) `new()` 初始化（约 63-68 行）：

```rust
        // 运行状态从 NexusRepo 默认值初始化
        let default_agent_id = config.default_agent_id().await;
        let default_role = config.default_role().await;
        let default_model = config.default_model().await;

        // ModelClient 每次调用现场合成配置，无需预查 model 配置
        let model_client = ModelClient::new(config.clone());
```

（`default_model` 现在是 `ProviderModel`，直接用于 `current_model: ArcSwap::from_pointee(default_model)`。）

(c) getter 与 setter（约 128-150 行）：

```rust
    pub fn current_agent_id(&self) -> String { self.current_agent_id.load().to_string() }
    pub fn current_role(&self) -> String { self.current_role.load().to_string() }
    pub fn current_model(&self) -> ProviderModel { (*self.current_model.load_full()).clone() }
    pub fn current_mode(&self) -> Mode { (*self.current_mode.load_full()).clone() }
    /// 切换当前模式（仅存状态，上下文重建由调用方触发 reset_context）
    pub fn set_current_mode(&self, mode: Mode) { self.current_mode.store(Arc::new(mode)); }

    /// 切换当前角色（角色切换同时重建上下文）
    pub async fn set_current_role(&self, role: Option<String>) {
        self.current_role.store(Arc::new(role.unwrap_or_default()));
        self.reset_context().await;
    }

    /// 切换当前模型（校验 provider/model 存在；每次调用由 ConfigManager 现场合成，无需热更新）
    pub async fn set_current_model(&self, pm: ProviderModel) -> Result<()> {
        // 校验 provider 与 model 存在
        if self.config.resolve_effective_config(&pm).await.is_none() {
            return Err(Error::ModelProviderNotSupported(format!(
                "provider/model 不存在: {}/{}", pm.provider, pm.model)));
        }
        self.current_model.store(Arc::new(pm));
        Ok(())
    }
```

（注：`ConfigManager::default_model()` getter 返回类型改为 `ProviderModel`——见 Step 3 之后在 config_manager.rs 调整 `pub async fn default_model(&self) -> String` 为 `pub async fn default_model(&self) -> ProviderModel`。）

(d) `run_agentic_loop` 调用处（约 412-416 行）：

```rust
        // 2. 调用模型
        let response = {
            let ctx = self.context_builder.lock().await;
            let messages = ctx.build();
            let model = self.model_client.lock().await;
            model.call(&self.current_model(), &messages).await
        };
```

- [ ] **Step 7: config_manager.rs 收尾（default_model getter + Coordinator import 一致性）**

`default_model` getter 类型调整：

```rust
    // ---------- default 读写 ----------
    #[allow(dead_code)]
    pub async fn default_agent_id(&self) -> String { self.nexus_repo.read().await.default_agent_id.to_string() }
    pub async fn default_role(&self) -> String { self.nexus_repo.read().await.default_role.to_string() }
    pub async fn default_model(&self) -> ProviderModel { (*self.nexus_repo.read().await.default_model.load_full()).clone() }
```

- [ ] **Step 8: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent 2>&1 | tail -30`
Expected: 全部 PASS。若 coordinator 测试有 `current_model` 相关断言或 `set_current_model` 旧调用残留，按新签名修正。

- [ ] **Step 9: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/src/config_manager.rs kissbot-agent/src/model_client.rs kissbot-agent/src/coordinator.rs kissbot-agent/src/command_router.rs kissbot-agent/src/types.rs && git commit -m "refactor(agent): 模型链路切换到 ProviderConfig 抽象（删旧 ModelConfig、ModelClient 现场合成、/model 双参数）
- ModelConfig 改 Option 可继承参数（去掉 name/provider/endpoint/api_key），NexusRepo.models 删除、providers 转正
- default_model/init_model/current_model 统一为 ProviderModel (provider, model)
- ModelClient::call(pm, messages) 每次 resolve 合成 EffectiveModelConfig，按 provider_type 构建 Provider，删 update_config/call_openai/call_anthropic
- /model <provider> <model> 双参数，set_current_model 校验存在"
```

---

### Task 5: 配置文件迁移与全量验证

**Files:**
- Modify: `script/template/nexus.json`、`workspace/agent-data/nexus.json`、`config.json`（root）、`script/config.json`、`test/workspace/config.json`
- Test: 无新测试（验证用）

**Interfaces:**
- Consumes: Task 1-4 的新配置格式（providers 嵌套、base_url 前缀、default_model 对象、init_model 对象）

- [ ] **Step 1: 迁移 nexus.json（script/template/nexus.json）**

将文件内容改为（旧平铺 `models` → `providers` 嵌套，`endpoint` 全路径 → `base_url` 前缀，`default_model` 字符串 → 对象）：

```json
{
  "channels": {},
  "providers": {
    "deepseek": {
      "name": "deepseek",
      "provider_type": "openai",
      "base_url": "https://api.deepseek.com",
      "api_key": "",
      "default_context_length": 65536,
      "default_max_tokens": 4096,
      "default_temperature": 0.7,
      "default_timeout_secs": 60,
      "default_retry_count": 3,
      "models": {
        "deepseek-4-flash": {
          "model": "deepseek-4-flash"
        }
      }
    }
  },
  "memory_structs": {},
  "stations": {},
  "default_agent_id": "",
  "default_role": "",
  "default_model": {
    "provider": "deepseek",
    "model": "deepseek-4-flash"
  }
}
```

- [ ] **Step 2: 迁移 nexus.json（workspace/agent-data/nexus.json）**

同上结构，但 `"api_key": "sk-test-ok"`（保留原值），`default_model` 对象指向 deepseek/deepseek-4-flash。

- [ ] **Step 3: 迁移 config.json（root / script/config.json / test/workspace/config.json）**

三个文件的 `agent` 段 `"init_model": ""` 改为对象：

```json
    "init_model": {
      "provider": "deepseek",
      "model": "deepseek-4-flash"
    }
```

（三份文件的 agent 段当前均为 `"init_agent_id": ""`、`"init_role": ""`、`"init_model": ""`，只改 `init_model` 一处。）

- [ ] **Step 4: 全量构建与测试**

Run: `cd /home/admin/project/kissbot && cargo build -p kissbot-agent 2>&1 | tail -5 && cargo test -p kissbot-agent 2>&1 | tail -20`
Expected: 构建成功、全部测试 PASS。

- [ ] **Step 5: 验证新配置可被解析（临时冒烟）**

Run: `cd /home/admin/project/kissbot && cargo run -p kissbot-agent -- --help 2>&1 | head -5 || true`
（注：agent 启动依赖 KISSBOT_CONFIG 等环境，此步仅确认无 panic；若启动报配置相关错误，检查 nexus.json 与 config.json 格式。）

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot && git add script/template/nexus.json workspace/agent-data/nexus.json config.json script/config.json test/workspace/config.json && git commit -m "chore(agent): 配置文件迁移到 ProviderConfig 新格式
- nexus.json：平铺 models 迁移为 providers 嵌套，endpoint 全路径改 base_url 前缀，default_model 改 ProviderModel 对象
- config.json（root/script/test/workspace）：agent 段 init_model 改 (provider, model) 对象"
```

---

## Self-Review 记录

**1. Spec 覆盖：**
- 配置数据结构（ProviderModel/ProviderConfig/ModelConfig/EffectiveModelConfig）→ Task 1 + Task 4
- ChannelConfig.channel_id + map key → Task 3
- Provider trait + OpenAi/Anthropic → Task 2
- ModelClient 现场合成 + 删 update_config → Task 4
- ProviderModel 打包（函数调用/current/default）→ Task 4
- /model 双参数 → Task 4
- base_url 前缀语义 → Task 2（路径拼装）+ Task 5（配置迁移）
- default_context_length 只落位（不接截断）→ Task 1（ProviderConfig 字段）
- 测试覆盖（resolve 合并/继承、provider 纯函数、channel_id）→ 各任务 Step 1
- nexus.json / config.json 迁移 → Task 5

**2. 占位符扫描：** 无 TBD/TODO；所有代码步骤含完整代码。

**3. 类型一致性：**
- `resolve_effective_config(&ProviderModel) -> Option<EffectiveModelConfig>`：Task 1/4 一致
- `Provider::send(&EffectiveModelConfig, &[MessageItem])`：Task 2/4 一致
- `AdminCommand::Model(ProviderModel)`：Task 4 一致
- `execute(command, config, coordinator, channel_id)`：Task 3/4 一致
- `default_model`（NexusRepo/AgentConfig/ConfigManager getter）均为 `ProviderModel`：Task 4 一致
- `ModelClient::call(pm, messages)`：Task 4 一致
