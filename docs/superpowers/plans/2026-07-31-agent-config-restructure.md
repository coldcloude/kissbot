# Agent 配置三分重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 kissbot-agent 配置拆为 AgentConfig（静态）/ NexusRepo+StationRepo（可改落盘）/ 运行状态（coordinator 内不落盘），废弃 CONFIG_PATH 统一 KISSBOT_CONFIG，并程序范围 llm->model 改名。

**Architecture:** AgentConfig 从 KISSBOT_CONFIG `agent` 段加载（kissbot-config 单例）；NexusRepo/StationRepo 落 `<data_dir>/*.json`，`Arc<RwLock<Repo>>` + 内部 `Arc<ArcSwapHashMap>`/`Arc<HashSet>` + COW 写回；运行状态作为 `AgentCoordinator` 字段（`ArcSwap` 标量 + `Arc<DashMap>` 集合），启动从 NexusRepo 默认值初始化，不落盘。各任务用临时 shim 委托保证中间状态可编译，最后一个任务移除 shim。

**Tech Stack:** Rust, tokio, arc-swap, dashmap, serde, kissbot-config, kissbot-api（`ArcSwapHashMap`）

## Global Constraints

- 所有文件 UTF-8 编码、`\n` 换行符
- 不删除代码中的注释
- 读写文件必须用 Read/Write/Edit 工具，禁止 sed/python 修改文件
- git commit 用中文，包含全部改动
- 字符串统一 `Arc<String>`；Repo Map 用 `Arc<ArcSwapHashMap<K,V>>`，运行状态 Map 用 `Arc<DashMap<K,Arc<V>>>`，确需变更的运行状态标量用 `ArcSwap<T>`
- 不使用 `ArcSwapHashMap` 于运行状态

## File Structure

| 文件 | 职责 |
|------|------|
| `kissbot-agent/Cargo.toml` | 新增 arc-swap / kissbot-config / serde-rc / tempfile(dev) |
| `kissbot-agent/src/repo.rs` | 新建：NexusRepo / StationRepo / ChannelConfig / ChannelUser / MemoryStructConfig / StationConfig 结构体 |
| `kissbot-agent/src/config_manager.rs` | 重写：AgentConfig（静态）+ ConfigManager（加载/引导/save/CRUD），最终无运行状态 |
| `kissbot-agent/src/model_client.rs` | 由 `llm_client.rs` 改名：ModelClient（原 LlmClient） |
| `kissbot-agent/src/types.rs` | ModelResponse（原 LlmResponse）、Error 变体改名 |
| `kissbot-agent/src/coordinator.rs` | 新增运行状态字段 + per-channel 连接 + 模型按 current_model 查 |
| `kissbot-agent/src/command_router.rs` | 运行状态命令走 coordinator、admin 走 ConfigManager |
| `kissbot-agent/src/main.rs` | AgentConfig 经 kissbot-config 加载、mgmt_host/port、删 CONFIG_PATH |
| `kissbot-agent/src/http_server.rs` | bind mgmt_host:mgmt_port |
| `kissbot-agent/src/memory_reader.rs` | memory_struct 从 NexusRepo 读 |
| `config.json`（root/script/test/workspace） | 新增 `agent` 段 |

---

### Task 1: 依赖变更

**Files:**
- Modify: `kissbot-agent/Cargo.toml`

**Interfaces:** 无（仅依赖）

- [ ] **Step 1: 修改 Cargo.toml**

在 `[dependencies]` 中：
- `serde` 行改为 `serde = { version = "1.0", features = ["derive", "rc"] }`（加 `"rc"`，支持 `Arc` 序列化）
- 新增 `arc-swap = { version = "1.9", features = ["serde"] }`
- 新增 `kissbot-config = { path = "../kissbot-config" }`

新增 dev-dependencies 段：

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 编译验证**

Run: `cd kissbot-agent && cargo build`
Expected: 编译通过（仅加了未用依赖，可能有 unused warning，无妨）

- [ ] **Step 3: Commit**

```bash
git add kissbot-agent/Cargo.toml
git commit -m "feat(agent): 新增 arc-swap/kissbot-config/serde-rc/tempfile 依赖"
```

---

### Task 2: llm->model 改名 + ModelConfig 加 name + 新结构体 repo.rs

本任务做两件可独立编译的事：(a) 程序范围 `Llm*` 改名 `Model*`，`ModelConfig` 增加 `name` 字段（`#[serde(default)]` 兼容旧 JSON）；(b) 新建 `repo.rs` 定义 NexusRepo 等结构体（暂不接入 ConfigManager，与旧结构并存）。

**Files:**
- Rename: `kissbot-agent/src/llm_client.rs` -> `kissbot-agent/src/model_client.rs`
- Modify: `kissbot-agent/src/model_client.rs`
- Modify: `kissbot-agent/src/types.rs`
- Modify: `kissbot-agent/src/config_manager.rs`
- Modify: `kissbot-agent/src/coordinator.rs`
- Modify: `kissbot-agent/src/main.rs`
- Create: `kissbot-agent/src/repo.rs`
- Test: `kissbot-agent/src/repo.rs`（内联 `#[cfg(test)]`）

**Interfaces:**
- Produces: `ModelConfig`（`name: Arc<String>` + 原 LlmConfig 字段，`#[serde(default)]` name）、`ModelClient`、`ModelResponse`、`repo.rs` 中全部新结构体

- [ ] **Step 1: git mv 改名文件**

```bash
cd kissbot-agent && git mv src/llm_client.rs src/model_client.rs
```

- [ ] **Step 2: model_client.rs 内 Llm->Model 改名**

`src/model_client.rs` 中：
- `use crate::config_manager::LlmConfig;` -> `use crate::config_manager::ModelConfig;`
- `use crate::types::{LlmResponse, Result, Error};` -> `use crate::types::{ModelResponse, Result, Error};`
- `pub struct LlmClient` -> `pub struct ModelClient`，字段 `config: LlmConfig` -> `config: ModelConfig`
- `impl LlmClient` -> `impl ModelClient`
- `pub fn new(config: LlmConfig) -> Self` -> `pub fn new(config: ModelConfig) -> Self`
- `pub fn update_config(&mut self, config: LlmConfig)` -> `pub fn update_config(&mut self, config: ModelConfig)`
- `pub async fn call(&self, messages: &[MessageItem]) -> Result<LlmResponse>` -> `Result<ModelResponse>`
- `async fn call_inner(&self, ...) -> Result<LlmResponse>` -> `Result<ModelResponse>`
- `async fn call_openai(&self, ...) -> Result<LlmResponse>` -> `Result<ModelResponse>`
- `async fn call_anthropic(&self, ...) -> Result<LlmResponse>` -> `Result<ModelResponse>`
- 函数体内所有 `LlmResponse` -> `ModelResponse`、`Error::LlmApiError` -> `Error::ModelApiError`、`Error::LlmProviderNotSupported` -> `Error::ModelProviderNotSupported`
- 注释中 "LLM API" -> "模型 API"、"LLM 调用失败" -> "模型调用失败"（保留注释）

- [ ] **Step 3: types.rs 改名**

`src/types.rs` 中：
- `pub struct LlmResponse` -> `pub struct ModelResponse`
- `#[error("LLM API error: {0}")]` 对应变体 `LlmApiError` -> `ModelApiError`，文案 `LLM API error` -> `Model API error`
- `#[error("LLM provider not supported: {0}")]` 对应变体 `LlmProviderNotSupported` -> `ModelProviderNotSupported`，文案改 `Model provider not supported`

- [ ] **Step 4: config_manager.rs LlmConfig->ModelConfig + name 字段**

`src/config_manager.rs` 中：
- `pub struct LlmConfig` -> `pub struct ModelConfig`，并在字段最前加 `#[serde(default)] pub name: Arc<String>`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub name: Arc<String>,
    pub provider: String,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub timeout_secs: u64,
    pub retry_count: u32,
}
```

- `impl Default for LlmConfig` -> `impl Default for ModelConfig`，`Self {` 内补 `name: Arc::new(String::new()),`
- `AgentConfigFile.llm: LlmConfig` -> `model: ModelConfig`
- `AgentConfig.llm: LlmConfig` -> `model: ModelConfig`
- `load()` 中 `let agent_config = AgentConfig { agent_id: file.agent_id, llm: file.llm, ... }` -> `model: file.model, ...`
- `save()` 中 `llm: self.agent_config.llm.clone()` -> `model: self.agent_config.model.clone()`
- `pub fn llm_config(&self) -> &LlmConfig` -> `pub fn model_config(&self) -> &ModelConfig`，函数体 `&self.agent_config.llm` -> `&self.agent_config.model`
- 顶部 `use std::sync::Arc;` 已有，无需改

- [ ] **Step 5: coordinator.rs 跟随改名**

`src/coordinator.rs` 中：
- `mod llm_client;` -> 不在此文件（在 main.rs）
- `use crate::llm_client::LlmClient;` -> `use crate::model_client::ModelClient;`
- `use crate::types::{Mode, WriteTask, ContextMessage, AdminCommand, Result, Error};` 不变（无 LlmResponse 直接引用）
- 字段 `llm_client: Arc<tokio::sync::Mutex<LlmClient>>` -> `model_client: Arc<tokio::sync::Mutex<ModelClient>>`
- `let llm_config = config.llm_config().clone();` -> `let model_config = config.model_config().clone();`
- `LlmClient::new(llm_config)` -> `ModelClient::new(model_config)`
- 结构体初始化 `llm_client,` -> `model_client: Arc::new(tokio::sync::Mutex::new(model_client)),`（注意字段名与值都对齐）
- `let llm = self.llm_client.lock().await;` -> `let model = self.model_client.lock().await;`，后续 `llm.call(...)` -> `model.call(...)`

- [ ] **Step 6: main.rs mod 改名**

`src/main.rs` 中：
- `mod llm_client;` -> `mod model_client;`
- 其它不变

- [ ] **Step 7: 新建 repo.rs 结构体**

`src/repo.rs`：

```rust
use std::collections::HashSet;
use std::sync::Arc;

use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};

use crate::config_manager::ModelConfig;

/// nexus 可改配置，持久化到 <data_dir>/nexus.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusRepo {
    #[serde(default)]
    pub channels: Arc<ArcSwapHashMap<String, ChannelConfig>>,
    #[serde(default)]
    pub models: Arc<ArcSwapHashMap<String, ModelConfig>>,
    #[serde(default)]
    pub memory_structs: Arc<ArcSwapHashMap<String, MemoryStructConfig>>,
    #[serde(default)]
    pub default_agent_id: Arc<String>,
    #[serde(default)]
    pub default_role: Arc<String>,
    #[serde(default)]
    pub default_model: Arc<String>,
}

impl Default for NexusRepo {
    fn default() -> Self {
        Self {
            channels: Arc::new(ArcSwapHashMap::new()),
            models: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            default_agent_id: Arc::new(String::new()),
            default_role: Arc::new(String::new()),
            default_model: Arc::new(String::new()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub messenger_id: Arc<String>,
    pub ws_url: Arc<String>,
    #[serde(default)]
    pub admins: Arc<HashSet<ChannelUser>>,
    #[serde(default)]
    pub default_bind_user: Option<ChannelUser>,
    #[serde(default)]
    pub enabled_by_default: bool,
}

/// 机器人绑定身份 / 管理员身份统一结构
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct ChannelUser {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStructConfig {
    pub name: Arc<String>,
    pub url: Arc<String>,
}

/// station 可改配置，持久化到 <data_dir>/station.json（本轮占位）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StationRepo {
    #[serde(default)]
    pub stations: Arc<ArcSwapHashMap<String, StationConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationConfig {
    pub station_id: Arc<String>,
    pub base_url: Arc<String>,
    pub timeout_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_user_hash_eq_by_value() {
        let a = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
        let b = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 ChannelUser 应命中 HashSet");
    }

    #[test]
    fn nexus_repo_serde_roundtrip() {
        let repo = NexusRepo {
            channels: Arc::new(ArcSwapHashMap::new()),
            models: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            default_agent_id: Arc::new("agent-1".into()),
            default_role: Arc::new("dev".into()),
            default_model: Arc::new("gpt-4o".into()),
        };
        let json = serde_json::to_string(&repo).unwrap();
        let back: NexusRepo = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.default_agent_id, "agent-1");
        assert_eq!(*back.default_role, "dev");
        assert_eq!(*back.default_model, "gpt-4o");
    }

    #[test]
    fn nexus_repo_default_empty() {
        let repo = NexusRepo::default();
        assert!(repo.channels.is_empty());
        assert!(repo.models.is_empty());
        assert!(repo.memory_structs.is_empty());
        assert!(repo.default_agent_id.is_empty());
    }
}
```

> 说明：`default_bind_user` 用 `Option<ChannelUser>`（空表示不默认绑定），与 spec 中"default_bind_user 非空才绑定"一致；`ChannelUser` derive `Hash`/`Eq`/`PartialEq`。

- [ ] **Step 8: main.rs 注册 repo 模块**

`src/main.rs` 加 `mod repo;`（与其它 `mod` 并列）。

- [ ] **Step 9: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过；repo.rs 三个测试 PASS。

- [ ] **Step 10: Commit**

```bash
git add -A kissbot-agent
git commit -m "refactor(agent): llm->model 改名（ModelConfig 加 name）+ 新建 repo.rs 结构体"
```

---

### Task 3: ConfigManager 接入新结构（AgentConfig 静态 + NexusRepo/StationRepo）+ 旧运行状态 getter 改 shim

本任务用新 `AgentConfig`（静态，从 KISSBOT_CONFIG `agent` 段）替换旧 `AgentConfig`/`AgentConfigFile`；ConfigManager 加载/引导 `NexusRepo`/`StationRepo` 并提供 CRUD。旧运行状态 getter（`current_role`/`channel_bindings`/`admin_users`/`set_current_role`/`add_binding`/`add_admin`/`remove_admin`/`agent_id`/`model_config` 等）改为**临时 shim**，委托到 NexusRepo 默认值/集合，保证 coordinator/command_router 仍可编译。shim 的语义是过渡态（如 `set_current_role` 暂时写 `default_role` 并回写），Task 4 用 coordinator 运行状态替代后移除。

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`
- Modify: `kissbot-agent/src/coordinator.rs`（per-channel ws_url、agent_id/model/shim 读取）
- Modify: `kissbot-agent/src/main.rs`（AgentConfig 经 kissbot-config 加载、删 CONFIG_PATH、mgmt_host/port）
- Modify: `kissbot-agent/src/http_server.rs`（bind mgmt_host:mgmt_port）
- Test: `kissbot-agent/src/config_manager.rs`（内联 `#[cfg(test)]`）

**Interfaces:**
- Consumes: `repo.rs`（Task 2）、`kissbot-config`
- Produces: 新 `AgentConfig`、`ConfigManager::new_from_config()`、`save_nexus()`/`save_station()`、NexusRepo/StationRepo CRUD、临时 shim getter

- [ ] **Step 1: 重写 config_manager.rs 顶部结构体与 AgentConfig**

删除旧 `AgentConfigFile`、旧 `AgentConfig`、`AgentRuntimeConfig`、`LlmConfig`（已改名 ModelConfig，保留）。新增静态 `AgentConfig`：

```rust
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::repo::{NexusRepo, StationRepo, ChannelConfig, ChannelUser, MemoryStructConfig, StationConfig};
use crate::types::{Mode, Result, Error};

// ModelConfig 仍定义在本文件（Task 2 已改名），供 model_client 与 repo.rs 共用
// （ModelConfig 定义保留在原处，含 #[serde(default)] name 字段）

/// 静态配置：来自 KISSBOT_CONFIG 的 agent 段，启动后不变
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub data_dir: Arc<String>,
    pub mgmt_host: Arc<String>,
    pub mgmt_port: u16,
    pub ws_reconnect_interval_secs: u64,
    #[serde(default)]
    pub init_agent_id: Arc<String>,
    #[serde(default)]
    pub init_role: Arc<String>,
    #[serde(default)]
    pub init_model: Arc<String>,
}

impl AgentConfig {
    /// 从 kissbot-config 全局单例的 agent 段加载
    pub fn from_public_config() -> Self {
        kissbot_config::Config::get().get_section("agent")
    }
}
```

> `ChannelBinding`/`AdminUser` 类型删除，统一用 `repo::ChannelUser`。`ConfigChangeListener` trait 保留。

- [ ] **Step 2: 重写 ConfigManager 结构与加载/引导**

```rust
pub struct ConfigManager {
    agent_config: AgentConfig,
    nexus_repo: Arc<RwLock<NexusRepo>>,
    station_repo: Arc<RwLock<StationRepo>>,
    nexus_path: String,
    station_path: String,
    listeners: DashMap<String, Arc<dyn ConfigChangeListener>>,
}

impl ConfigManager {
    /// 从公共配置加载 AgentConfig，按 data_dir 加载/引导 NexusRepo/StationRepo
    pub async fn new() -> Result<Self> {
        let agent_config = AgentConfig::from_public_config();
        let data_dir = agent_config.data_dir.to_string();
        tokio::fs::create_dir_all(&data_dir).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        // 派生子目录（仅创建，功能本轮不实现）
        for sub in ["sessions", "attachments", "station"] {
            let _ = tokio::fs::create_dir_all(format!("{}/{}", data_dir, sub)).await;
        }
        let nexus_path = format!("{}/nexus.json", data_dir);
        let station_path = format!("{}/station.json", data_dir);

        let nexus_repo = Self::load_or_create_nexus(&nexus_path, &agent_config).await?;
        let station_repo = Self::load_or_create_station(&station_path).await?;

        Ok(Self {
            agent_config,
            nexus_repo: Arc::new(RwLock::new(nexus_repo)),
            station_repo: Arc::new(RwLock::new(station_repo)),
            nexus_path,
            station_path,
            listeners: DashMap::new(),
        })
    }

    async fn load_or_create_nexus(path: &str, cfg: &AgentConfig) -> Result<NexusRepo> {
        if std::path::Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await
                .map_err(|e| Error::ConfigNotFound(format!("{}: {}", path, e)))?;
            let repo: NexusRepo = serde_json::from_str(&content)
                .map_err(|e| Error::ConfigParseError(e.to_string()))?;
            Ok(repo)
        } else {
            // 首次创建：用 init_* 种子 3 个 default，集合为空
            let repo = NexusRepo {
                default_agent_id: cfg.init_agent_id.clone(),
                default_role: cfg.init_role.clone(),
                default_model: cfg.init_model.clone(),
                ..NexusRepo::default()
            };
            let json = serde_json::to_string_pretty(&repo)?;
            tokio::fs::write(path, json).await.map_err(|e| Error::IoError(e.to_string()))?;
            Ok(repo)
        }
    }

    async fn load_or_create_station(path: &str) -> Result<StationRepo> {
        if std::path::Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await
                .map_err(|e| Error::ConfigNotFound(format!("{}: {}", path, e)))?;
            let repo: StationRepo = serde_json::from_str(&content)
                .map_err(|e| Error::ConfigParseError(e.to_string()))?;
            Ok(repo)
        } else {
            let repo = StationRepo::default();
            let json = serde_json::to_string_pretty(&repo)?;
            tokio::fs::write(path, json).await.map_err(|e| Error::IoError(e.to_string()))?;
            Ok(repo)
        }
    }

    pub async fn save_nexus(&self) -> Result<()> {
        let repo = self.nexus_repo.read().await;
        let json = serde_json::to_string_pretty(&*repo)?;
        tokio::fs::write(&self.nexus_path, json).await.map_err(|e| Error::IoError(e.to_string()))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn save_station(&self) -> Result<()> {
        let repo = self.station_repo.read().await;
        let json = serde_json::to_string_pretty(&*repo)?;
        tokio::fs::write(&self.station_path, json).await.map_err(|e| Error::IoError(e.to_string()))?;
        Ok(())
    }
}
```

- [ ] **Step 3: 静态配置 getter**

```rust
    pub fn ws_reconnect_interval_secs(&self) -> u64 { self.agent_config.ws_reconnect_interval_secs }
    pub fn mgmt_host(&self) -> &str { &self.agent_config.mgmt_host }
    pub fn mgmt_port(&self) -> u16 { self.agent_config.mgmt_port }
    pub fn data_dir(&self) -> &str { &self.agent_config.data_dir }
```

删除旧 `channel_ws_url()`、`memory_struct_url()`、`stations()`（ws_url 下沉 per-channel、memory_struct 进 NexusRepo、stations 进 StationRepo）。

- [ ] **Step 4: NexusRepo CRUD getter/setter（写时 COW + save_nexus）**

```rust
    // ---------- channels ----------
    /// 返回所有 channel 配置快照（messenger_id -> Arc<ChannelConfig>）
    pub async fn channels(&self) -> Vec<(String, Arc<ChannelConfig>)> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter().map(|(k, v)| (k.clone(), v.load())).collect()
    }
    pub async fn channel_ws_url(&self, messenger_id: &str) -> Option<String> {
        let repo = self.nexus_repo.read().await;
        repo.channels.get(messenger_id).map(|s| s.load().ws_url.to_string())
    }
    pub async fn add_channel(&self, ch: ChannelConfig) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.channels);
            map.insert(ch.messenger_id.to_string(), ArcSwap::new(Arc::new(ch)));
        }
        self.save_nexus().await
    }
    pub async fn remove_channel(&self, messenger_id: &str) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.channels);
            map.remove(messenger_id);
        }
        self.save_nexus().await
    }

    // ---------- models ----------
    pub async fn model_config_by_name(&self, name: &str) -> Option<ModelConfig> {
        let repo = self.nexus_repo.read().await;
        repo.models.get(name).map(|s| (*s.load()).clone())
    }
    pub async fn add_model(&self, cfg: ModelConfig) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.models);
            map.insert(cfg.name.to_string(), ArcSwap::new(Arc::new(cfg)));
        }
        self.save_nexus().await
    }

    // ---------- memory_structs ----------
    pub async fn memory_structs(&self) -> Vec<MemoryStructConfig> {
        let repo = self.nexus_repo.read().await;
        repo.memory_structs.iter().map(|(_, v)| (*v.load()).clone()).collect()
    }

    // ---------- default 读写 ----------
    pub async fn default_agent_id(&self) -> String { self.nexus_repo.read().await.default_agent_id.to_string() }
    pub async fn default_role(&self) -> String { self.nexus_repo.read().await.default_role.to_string() }
    pub async fn default_model(&self) -> String { self.nexus_repo.read().await.default_model.to_string() }
```

> 注：`ArcSwapHashMap` 的 `.iter()` 来自其 `Deref<Target=HashMap<K, ArcSwap<V>>>`；`v.load()` 返回 `Arc<V>`。`Arc::make_mut(&mut repo.channels)` 对 `Arc<ArcSwapHashMap>` 做写时复制（参考 channel-web `write_config`）。

- [ ] **Step 5: 临时 shim -- 旧运行状态 getter 委托 NexusRepo**

```rust
    // ===== 临时 shim（Task 4 移除，改由 coordinator 运行状态承担）=====
    /// shim: agent_id 暂读 default_agent_id
    pub fn agent_id(&self) -> String {
        // 同步读不到 RwLock async，这里用 try_read 兜底空串；Task 4 后此方法删除
        self.nexus_repo.try_read().map(|r| r.default_agent_id.to_string()).unwrap_or_default()
    }
    /// shim: model_config 暂按 default_model 名查
    pub async fn model_config(&self) -> ModelConfig {
        let name = self.default_model().await;
        self.model_config_by_name(&name).await
            .unwrap_or_else(|| ModelConfig::default())
    }
    /// shim: current_role 暂读 default_role
    pub async fn current_role(&self) -> String { self.default_role().await }
    /// shim: current_mode 暂返回 Role
    pub async fn current_mode(&self) -> Mode { Mode::Role }
    /// shim: channel_bindings 暂读 enabled + default_bind_user 非空的 channel
    pub async fn channel_bindings(&self) -> Vec<ChannelUser> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter()
            .filter_map(|(_, v)| {
                let c = v.load();
                if c.enabled_by_default {
                    c.default_bind_user.clone()
                } else { None }
            })
            .collect()
    }
    /// shim: admin_users 暂聚合所有 channel 的 admins
    pub async fn admin_users(&self) -> Vec<ChannelUser> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter()
            .flat_map(|(_, v)| v.load().admins.iter().cloned())
            .collect()
    }
    /// shim: set_current_role 暂写 default_role 并回写（过渡态，Task 4 改为 coordinator 运行状态不回写）
    pub async fn set_current_role(&self, role: Option<String>) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            repo.default_role = Arc::new(role.unwrap_or_default());
        }
        self.save_nexus().await?;
        self.notify_listeners().await;
        Ok(())
    }
    /// shim: set_current_mode 暂不落盘（过渡态）
    pub async fn set_current_mode(&self, _mode: Mode) -> Result<()> { Ok(()) }
    /// shim: add_binding/remove_binding/add_admin/remove_admin 暂改 NexusRepo 并回写
    pub async fn add_binding(&self, binding: ChannelUser) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            if let Some(swap) = repo.channels.get(&binding.messenger_id.to_string()) {
                let mut ch = (*swap.load()).clone();
                ch.default_bind_user = Some(binding);
                swap.store(Arc::new(ch));
            }
        }
        self.save_nexus().await
    }
    pub async fn remove_binding(&self, messenger_id: &str) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            if let Some(swap) = repo.channels.get(messenger_id) {
                let mut ch = (*swap.load()).clone();
                ch.default_bind_user = None;
                swap.store(Arc::new(ch));
            }
        }
        self.save_nexus().await
    }
    pub async fn add_admin(&self, admin: ChannelUser) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            if let Some(swap) = repo.channels.get(&admin.messenger_id.to_string()) {
                let mut ch = (*swap.load()).clone();
                Arc::make_mut(&mut ch.admins).insert(admin);
                swap.store(Arc::new(ch));
            }
        }
        self.save_nexus().await
    }
    pub async fn remove_admin(&self, messenger_id: &str, user_id: &str) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            if let Some(swap) = repo.channels.get(messenger_id) {
                let mut ch = (*swap.load()).clone();
                let target = ChannelUser { messenger_id: Arc::new(messenger_id.into()), user_id: Arc::new(user_id.into()) };
                Arc::make_mut(&mut ch.admins).remove(&target);
                swap.store(Arc::new(ch));
            }
        }
        self.save_nexus().await
    }
```

> `notify_listeners` 保留原实现。`add_listener` 保留。

- [ ] **Step 6: coordinator.rs per-channel ws_url + shim 读取**

`src/coordinator.rs` 的 `connect_channels` 改为每 channel 从 `NexusRepo` 读 ws_url：

```rust
    async fn connect_channels(self: &Arc<Self>) {
        let bindings = self.config.channel_bindings().await;   // shim: Vec<ChannelUser>
        let reconnect_secs = self.config.ws_reconnect_interval_secs();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        let coordinator = self.clone();

        for binding in &bindings {
            let messenger_id = binding.messenger_id.to_string();
            let user_id = binding.user_id.to_string();
            let ws_url = match self.config.channel_ws_url(&messenger_id).await {
                Some(u) => u,
                None => { warn!("channel {} 无 ws_url，跳过", messenger_id); continue; }
            };
            // ...（其余 ChannelClient::new / connect / bind / 重连循环与原逻辑一致，ws_url 用上面的 per-channel 值）
        }
    }
```

`new()` 中 `let model_config = config.model_config().clone;` 改为 `let model_config = config.model_config().await;`（shim async），`ModelClient::new(model_config)` 不变。`incoming_message` / `send_reply` / `run_agentic_loop` 中 `self.config.agent_id()` 保持同步调用（shim 同步）。`load_ego_info` 中 `config.agent_id()` 同步（shim）。

- [ ] **Step 7: main.rs 改用 ConfigManager::new + mgmt_host/port**

```rust
    let config = Arc::new(
        config_manager::ConfigManager::new().await
            .expect("初始化配置失败")
    );
    info!("Agent ID: {}", config.agent_id());
    // ...
    let mgr_config = config.clone();
    let host = mgr_config.mgmt_host().to_string();
    let port = mgr_config.mgmt_port();
    tokio::spawn(async move {
        let server = http_server::HttpServer::new(mgr_config, host, port);
        if let Err(e) = server.start().await {
            tracing::error!("管理 API 服务器退出: {:?}", e);
        }
    });
```

删除 `CONFIG_PATH` 读取。`mod repo;` 已在 Task 2 加。

- [ ] **Step 8: http_server.rs 接收 host + port**

```rust
pub struct HttpServer {
    config: Arc<ConfigManager>,
    host: String,
    port: u16,
}
impl HttpServer {
    pub fn new(config: Arc<ConfigManager>, host: String, port: u16) -> Self {
        Self { config, host, port }
    }
    pub async fn start(&self) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| crate::types::Error::IoError(e.to_string()))?;
        info!("管理 API 服务器启动: {}", addr);
        // ...（循环与原一致）
    }
}
```

- [ ] **Step 9: memory_reader.rs memory_struct 改读 NexusRepo**

`read_memory_struct_index` 改用 `config.memory_structs().await`（返回 `Vec<MemoryStructConfig>`），删除原 `config.memory_struct_url()` 调用（Task 3 Step 3 已删该方法）。`config.agent_id()`/`config.current_role().await` 暂保留（Task 3 仍为 shim，Task 4 改为参数传入）：

```rust
    pub async fn read_memory_struct_index(&self, config: &ConfigManager, _mode: &Mode) -> Result<Vec<String>> {
        let structs = config.memory_structs().await;
        if structs.is_empty() {
            return Ok(Vec::new());
        }
        // memory-struct 功能未实现，暂占位
        Ok(Vec::new())
    }
```

- [ ] **Step 10: config_manager.rs 内联测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn agent_config(data_dir: &str) -> AgentConfig {
        AgentConfig {
            data_dir: Arc::new(data_dir.into()),
            mgmt_host: Arc::new("127.0.0.1".into()),
            mgmt_port: 9090,
            ws_reconnect_interval_secs: 5,
            init_agent_id: Arc::new("agent-1".into()),
            init_role: Arc::new("dev".into()),
            init_model: Arc::new("gpt-4o".into()),
        }
    }

    #[tokio::test]
    async fn bootstrap_creates_nexus_with_seeds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let cfg = agent_config(dir.path().to_str().unwrap());
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap(), &cfg).await.unwrap();
        assert_eq!(*repo.default_agent_id, "agent-1");
        assert_eq!(*repo.default_role, "dev");
        assert_eq!(*repo.default_model, "gpt-4o");
        assert!(repo.channels.is_empty());
        assert!(path.exists(), "首次创建应写文件");
    }

    #[tokio::test]
    async fn bootstrap_loads_existing_nexus() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let cfg = agent_config(dir.path().to_str().unwrap());
        // 第一次创建
        let _ = ConfigManager::load_or_create_nexus(path.to_str().unwrap(), &cfg).await.unwrap();
        // 改 init 不影响第二次（文件已存在为权威）
        let cfg2 = AgentConfig { init_agent_id: Arc::new("changed".into()), ..cfg };
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap(), &cfg2).await.unwrap();
        assert_eq!(*repo.default_agent_id, "agent-1", "文件存在时 init_* 应被忽略");
    }

    #[tokio::test]
    async fn save_nexus_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let cfg = agent_config(dir.path().to_str().unwrap());
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap(), &cfg).await.unwrap();
        // 模拟写回再读
        let json = serde_json::to_string_pretty(&repo).unwrap();
        std::fs::write(&path, json).unwrap();
        let back: NexusRepo = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(*back.default_agent_id, "agent-1");
    }
}
```

> `load_or_create_nexus` 需改为 `pub(crate)` 或测试可见（`#[cfg(test)]` 下同模块可访问私有，无需改可见性，因测试在 `config_manager.rs` 内）。若 `load_or_create_nexus` 是关联函数，测试直接调用即可。

- [ ] **Step 11: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过（shim 使 coordinator/command_router 仍可编译）；config_manager 三个测试 + repo.rs 三个测试 PASS。

- [ ] **Step 12: Commit**

```bash
git add -A kissbot-agent
git commit -m "refactor(agent): ConfigManager 接入 AgentConfig静态+NexusRepo/StationRepo，旧运行状态 getter 改 shim"
```

---

### Task 4: coordinator 运行状态迁移 + command_router 语义 + 移除 shim

把运行状态（current_agent_id/role/model、bound_channels、selected_memory_structs）落到 `AgentCoordinator`（`ArcSwap` 标量 + `Arc<DashMap>` 集合），启动从 NexusRepo 默认值初始化；`/bind` `/unbind` `/role` `/mode` `/model` `/agent` 走 coordinator 不回写，`/admin` `/unadmin` 走 ConfigManager NexusRepo 回写；移除 Task 3 的临时 shim。

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`
- Modify: `kissbot-agent/src/command_router.rs`
- Modify: `kissbot-agent/src/config_manager.rs`（删 shim）
- Modify: `kissbot-agent/src/types.rs`（AdminCommand 加 Model/Agent 变体）
- Test: `kissbot-agent/src/coordinator.rs`（内联 `#[cfg(test)]`）

**Interfaces:**
- Consumes: ConfigManager NexusRepo 默认值/CRUD（Task 3）
- Produces: coordinator 运行状态 getter/setter（`current_agent_id()`/`current_role()`/`current_model()`/`bound_channels()`/`set_current_role()`/`set_current_model()`/`set_current_agent_id()`/`bind`/`unbind`）、`AdminCommand::Model`/`Agent`

- [ ] **Step 1: types.rs AdminCommand 加变体**

```rust
pub enum AdminCommand {
    Bind { messenger_id: String, user_id: String },
    Unbind { messenger_id: String },
    Admin { messenger_id: String, user_id: String },
    Unadmin { messenger_id: String, user_id: String },
    SetRole(Option<String>),
    ModeEvent(Option<String>),
    ModeRole,
    Reenter(String),
    Events,
    Reset,
    Model(String),   // 新增：/model <name>
    Agent(String),   // 新增：/agent <id>
}
```

- [ ] **Step 2: command_router.rs 解析新命令**

`parse` 的 `match parts[0]` 增加：

```rust
            "model" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand("格式: /model <name>".to_string()));
                }
                Ok(AdminCommand::Model(parts[1].to_string()))
            }
            "agent" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand("格式: /agent <id>".to_string()));
                }
                Ok(AdminCommand::Agent(parts[1].to_string()))
            }
```

- [ ] **Step 3: coordinator.rs 新增运行状态字段**

`AgentCoordinator` 结构体增加：

```rust
use arc_swap::ArcSwap;
use crate::repo::ChannelUser;

pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    // ...原有字段...
    current_agent_id: Arc<ArcSwap<String>>,
    current_role: Arc<ArcSwap<String>>,
    current_model: Arc<ArcSwap<String>>,
    bound_channels: Arc<DashMap<String, Arc<ChannelUser>>>,
    selected_memory_structs: Arc<DashMap<String, ()>>,
}
```

- [ ] **Step 4: coordinator.rs new() 初始化运行状态**

`new()` 中（`connect_channels` 之前）：

```rust
        let default_agent_id = config.default_agent_id().await;
        let default_role = config.default_role().await;
        let default_model = config.default_model().await;

        let coordinator = Arc::new(Self {
            config: config.clone(),
            // ...原有...
            current_agent_id: Arc::new(ArcSwap::from_pointee(default_agent_id)),
            current_role: Arc::new(ArcSwap::from_pointee(default_role)),
            current_model: Arc::new(ArcSwap::from_pointee(default_model)),
            bound_channels: Arc::new(DashMap::new()),
            selected_memory_structs: Arc::new(DashMap::new()),
            channel_clients: Arc::new(DashMap::new()),
            disconnect_notify: Arc::new(DashMap::new()),
        });

        // 初始化 bound_channels：enabled_by_default 且 default_bind_user 非空
        for (_, ch) in config.channels().await {
            if ch.enabled_by_default {
                if let Some(bu) = &ch.default_bind_user {
                    coordinator.bound_channels.insert(ch.messenger_id.to_string(), Arc::new(bu.clone()));
                }
            }
        }
        coordinator.connect_channels().await;
```

- [ ] **Step 5: coordinator.rs 运行状态 getter/setter**

```rust
    pub fn current_agent_id(&self) -> String { self.current_agent_id.load().to_string() }
    pub fn current_role(&self) -> String { self.current_role.load().to_string() }
    pub fn current_model(&self) -> String { self.current_model.load().to_string() }
    pub async fn set_current_role(&self, role: Option<String>) {
        self.current_role.store(Arc::new(role.unwrap_or_default()));
        self.reset_context().await;
    }
    pub async fn set_current_model(&self, name: String) -> Result<()> {
        // 校验 model 存在
        if self.config.model_config_by_name(&name).await.is_none() {
            return Err(Error::LlmProviderNotSupported(format!("model 不存在: {}", name)));
        }
        self.current_model.store(Arc::new(name.clone()));
        // 热更新 ModelClient
        let cfg = self.config.model_config_by_name(&name).await.unwrap();
        let mut client = self.model_client.lock().await;
        client.update_config(cfg);
        Ok(())
    }
    pub async fn set_current_agent_id(&self, id: String) {
        self.current_agent_id.store(Arc::new(id));
        self.reset_context().await;
    }
    pub async fn bind_channel(&self, binding: ChannelUser) {
        self.bound_channels.insert(binding.messenger_id.to_string(), Arc::new(binding));
    }
    pub async fn unbind_channel(&self, messenger_id: &str) {
        self.bound_channels.remove(messenger_id);
    }
```

> 注：`Error::LlmProviderNotSupported` 在 Task 2 已改名 `ModelProviderNotSupported`，此处用 `Error::ModelProviderNotSupported`。

- [ ] **Step 6: coordinator.rs 消费点改用运行状态**

- `incoming_message` / `send_reply` / `run_agentic_loop` / `load_ego_info` 中 `self.config.agent_id()` -> `self.current_agent_id()`（同步）；`self.config.current_role().await` -> `self.current_role()`（同步）。
- `connect_channels` 中 `let bindings = self.config.channel_bindings().await;` 改为从 `self.bound_channels` 读：

```rust
        let bindings: Vec<Arc<ChannelUser>> = self.bound_channels.iter().map(|e| e.value().clone()).collect();
```

- `new()` 中 `let model_config = config.model_config().await;` 改为 `let model_config = config.model_config_by_name(&coordinator.current_model.load()).await.unwrap_or_else(ModelConfig::default);`（在 coordinator 构造后取，或先取 default_model 再查）。注意构造顺序：先建 coordinator 再查 model；或先查 default_model 的 config。实现时把 `ModelClient` 初始化移到 coordinator 构造后用 `Arc::new(tokio::sync::Mutex::new(...))` 填入。简化：先 `let default_model = config.default_model().await; let model_config = config.model_config_by_name(&default_model).await.unwrap_or_else(ModelConfig::default);` 再构造。

- [ ] **Step 7: command_router.rs execute 改走 coordinator**

`CommandRouter::execute` 签名改为接收 coordinator（运行状态）+ config（admin/NexusRepo）：

```rust
    pub async fn execute(
        command: &AdminCommand,
        config: &ConfigManager,
        coordinator: &AgentCoordinator,
    ) -> Result<(String, bool)> {
        match command {
            AdminCommand::Bind { messenger_id, user_id } => {
                coordinator.bind_channel(ChannelUser { messenger_id: Arc::new(messenger_id.clone()), user_id: Arc::new(user_id.clone()) }).await;
                Ok((format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id), false))
            }
            AdminCommand::Unbind { messenger_id } => {
                coordinator.unbind_channel(messenger_id).await;
                Ok((format!("✅ 已解绑 messenger: {}", messenger_id), false))
            }
            AdminCommand::Admin { messenger_id, user_id } => {
                config.add_admin(ChannelUser { messenger_id: Arc::new(messenger_id.clone()), user_id: Arc::new(user_id.clone()) }).await?;
                Ok((format!("✅ 已添加管理权限: {} / {}", messenger_id, user_id), false))
            }
            AdminCommand::Unadmin { messenger_id, user_id } => {
                config.remove_admin(messenger_id, user_id).await?;
                Ok((format!("✅ 已移除管理权限: {} / {}", messenger_id, user_id), false))
            }
            AdminCommand::SetRole(role) => {
                coordinator.set_current_role(role.clone()).await;
                Ok((match role { Some(n) => format!("✅ 已切换角色为: {}", n), None => "✅ 已取消角色".into() }, true))
            }
            AdminCommand::Model(name) => {
                coordinator.set_current_model(name.clone()).await?;
                Ok((format!("✅ 已切换模型为: {}", name), false))
            }
            AdminCommand::Agent(id) => {
                coordinator.set_current_agent_id(id.clone()).await;
                Ok((format!("✅ 已切换 agent 为: {}", id), true))
            }
            AdminCommand::ModeEvent(event_id) => {
                let id = event_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                Ok((format!("✅ 新事件 ID: {}", id), true))
            }
            AdminCommand::ModeRole => Ok(("✅ 已切换为角色模式".into(), true)),
            AdminCommand::Reenter(event_id) => Ok((format!("✅ 将重进事件: {}", event_id), true)),
            AdminCommand::Events => Ok(("📋 查询事件列表中...".into(), false)),
            AdminCommand::Reset => Ok(("🔄 正在重置上下文...".into(), true)),
        }
    }
```

`check_admin` 改用 `config.admin_users().await`（仍走 ConfigManager 聚合 NexusRepo admins，签名不变）。`coordinator.rs` 的 `handle_admin_command` 调用改为 `CommandRouter::execute(&cmd, &self.config, self).await`，并处理 `AdminCommand::Model`/`Agent` 的 `cmd_needs_reset`（Model 不重建，Agent 重建已在 set_current_agent_id 内做，此处返回 true 时按需 reset）。

- [ ] **Step 8: memory_reader.rs 签名改为接收运行时快照**

运行状态（agent_id/role）移到 coordinator 后，memory_reader 不再从 ConfigManager 读。三个方法签名增加 `agent_id: &str`、`role_name: &str` 参数（coordinator 传入当前快照），删除方法体内 `config.agent_id()`/`config.current_role().await`：

```rust
    pub async fn read_memory_struct_index(&self, config: &ConfigManager, agent_id: &str, role_name: &str, _mode: &Mode) -> Result<Vec<String>>
    pub async fn read_history(&self, config: &ConfigManager, agent_id: &str, role_name: &str, mode: &Mode) -> Result<Vec<ContextMessage>>
    #[allow(dead_code)]
    pub async fn list_events(&self, config: &ConfigManager, agent_id: &str, role_name: &str) -> Result<Vec<String>>
```

方法体内原 `let agent_id = config.agent_id();` / `let role_name = config.current_role().await;` 删除，改用参数。

- [ ] **Step 9: coordinator.rs 调用点传快照 + handle_incoming/handle_admin_command 改用运行状态**

- `new()`/`reset_context()` 中 `memory_reader.read_history(&config, &mode)` -> `read_history(&self.config, &self.current_agent_id(), &self.current_role(), &mode)`；`read_memory_struct_index` 同理传快照。
- `handle_incoming` 中 `let bindings = self.config.channel_bindings().await; if !bindings.iter().any(...)` 改为：
```rust
        if !self.bound_channels.contains_key(&messenger_id) { return; }
```
- `handle_admin_command` 中 `ModeEvent`/`ModeRole`/`Reenter` 分支删除 `let _ = self.config.set_current_mode(...).await;`（mode 不再落盘，`mode_manager.set_mode` + `reset_context` 已足够）。
- `incoming_message`/`send_reply`/`run_agentic_loop`/`load_ego_info` 中 `self.config.agent_id()` -> `self.current_agent_id()`、`self.config.current_role().await` -> `self.current_role()`（与 Step 6 一致，确保无残留）。

- [ ] **Step 10: main.rs 日志改 default_agent_id**

`main.rs` 中 `info!("Agent ID: {}", config.agent_id());` 改为 `info!("Agent ID: {}", config.default_agent_id().await);`（main 无 coordinator，此时运行状态未建，记默认值）。

- [ ] **Step 11: config_manager.rs 移除 shim**

删除 Task 3 Step 5 的全部 shim 方法：`agent_id`/`model_config`/`current_role`/`current_mode`/`channel_bindings`/`admin_users`/`set_current_role`/`set_current_mode`/`add_binding`/`remove_binding`/`add_admin`/`remove_admin`。保留 `default_*` getter、`channels`/`channel_ws_url`/`add_channel`/`remove_channel`/`model_config_by_name`/`add_model`/`memory_structs`/`admin_users`（聚合，check_admin 用）。`admin_users` 聚合方法保留（check_admin 需要）。其余 shim 删除。

> 检查 `command_router.rs` `check_admin` 仍用 `config.admin_users()`（保留）。`coordinator.rs` 不再调 `config.agent_id()`/`config.current_role()`/`config.channel_bindings()`（已改运行状态）。

- [ ] **Step 12: coordinator.rs 内联测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_manager::ConfigManager;

    // 注：ConfigManager::new 依赖 KISSBOT_CONFIG 全局单例，单元测试难注入。
    // bound_channels 初始化逻辑用直接构造验证：
    #[test]
    fn bound_channels_init_logic() {
        // 模拟：enabled + default_bind_user 非空才入 bound_channels
        let enabled_with_bind = ChannelConfig {
            messenger_id: Arc::new("m1".into()), ws_url: Arc::new("ws://x".into()),
            admins: Arc::new(HashSet::new()),
            default_bind_user: Some(ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) }),
            enabled_by_default: true,
        };
        let enabled_no_bind = ChannelConfig {
            default_bind_user: None, ..enabled_with_bind.clone()
        };
        let disabled_with_bind = ChannelConfig {
            messenger_id: Arc::new("m2".into()), enabled_by_default: false, ..enabled_with_bind.clone()
        };
        let all = vec![enabled_with_bind.clone(), enabled_no_bind, disabled_with_bind];
        let bound: Vec<_> = all.iter()
            .filter(|c| c.enabled_by_default)
            .filter_map(|c| c.default_bind_user.clone())
            .collect();
        assert_eq!(bound.len(), 1);
        assert_eq!(*bound[0].messenger_id, "m1");
    }
}
```

> `ChannelConfig` 需 derive `Clone`（Task 2 已 derive）。`HashSet` import 在测试模块加 `use std::collections::HashSet;`。

- [ ] **Step 13: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过（shim 已移除，coordinator/command_router/memory_reader 用运行状态/快照）；全部测试 PASS。

- [ ] **Step 14: Commit**

```bash
git add -A kissbot-agent
git commit -m "refactor(agent): 运行状态迁移到 coordinator + command_router 语义调整 + 移除 shim"
```

---

### Task 5: memory_reader + config.json agent 段 + 文档 + 清理

`memory_reader` 的 memory_struct 改读 NexusRepo；repo 内 `config.json` 加 `agent` 段；更新组件设计文档；清理遗留。

**Files:**
- Modify: `config.json`
- Modify: `script/config.json`
- Modify: `test/workspace/config.json`
- Modify: `test/workspace-template/config.json`
- Modify: `docs/design/components-design/kissbot-agent.md`

**Interfaces:** 无新接口

- [ ] **Step 1: config.json 加 agent 段**

`config.json`（root）顶层加：

```json
  "agent": {
    "data_dir": "data",
    "mgmt_host": "127.0.0.1",
    "mgmt_port": 9090,
    "ws_reconnect_interval_secs": 5,
    "init_agent_id": "",
    "init_role": "",
    "init_model": ""
  },
```

`script/config.json`、`test/workspace/config.json`、`test/workspace-template/config.json` 同样加该段（`data_dir` 按各场景调整为相对路径，如 script 用 `../workspace/agent-data`，test 用 `agent-data`）。

- [ ] **Step 2: 更新组件设计文档**

`docs/design/components-design/kissbot-agent.md` 的"配置管理器"小节改为描述三分结构（AgentConfig 静态 / NexusRepo+StationRepo 可改 / 运行状态在 coordinator），与 spec 一致。不删除现有注释。

- [ ] **Step 3: 全量编译 + 测试**

Run: `cd kissbot-agent && cargo build && cargo test`
Expected: 编译通过、无 warning 遗留（除已知 dead_code 标注）、全部测试 PASS。

- [ ] **Step 4: 手动启动验证（可选）**

Run: `cd script && KISSBOT_CONFIG=config.json cargo run --manifest-path ../kissbot-agent/Cargo.toml`
Expected: agent 启动，日志打印 Agent ID（default_agent_id），`<data_dir>/nexus.json` 与 `station.json` 被创建，管理 API 监听 `127.0.0.1:9090`。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(agent): memory_reader 读 NexusRepo memory_struct + config.json 加 agent 段 + 文档更新"
```

---

## Self-Review 笔记

- **Spec 覆盖**：AgentConfig（Task 3）/ NexusRepo+StationRepo（Task 2 结构 + Task 3 接入）/ 运行状态（Task 4）/ 持久化语义（Task 4 command_router）/ Bootstrap（Task 3）/ llm->model 改名（Task 2）/ main+http_server+memory_reader（Task 3+5）/ config.json（Task 5）均已覆盖。station 占位（Task 2 StationRepo + Task 3 save_station）。memory-struct 选中集合（Task 4 selected_memory_structs 字段，功能未实现符合"推迟"）。
- **shim 过渡态**：Task 3 的 shim 在 Task 4 移除；中间态 `set_current_role` 等会回写 default（过渡），最终态（Task 4 后）运行状态不回写，符合 spec。
- **类型一致性**：`ChannelUser`（repo.rs）/ `ModelConfig`（config_manager.rs）/ `ChannelConfig.default_bind_user: Option<ChannelUser>` 各任务间一致；`AdminCommand::Model/Agent`（Task 4 types.rs）与 command_router 解析、coordinator setter 一致。
- **已知简化**：`selected_memory_structs` 仅建字段未接查询逻辑（memory-struct 推迟）；`sessions`/`attachments`/`station` 子目录仅创建未用（功能推迟）；管理 API 完整路由未实现（http_server 仍骨架，仅接 mgmt 监听）。
