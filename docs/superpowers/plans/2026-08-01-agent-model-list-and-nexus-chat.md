# Agent 模型列表获取 + 保留agent + nexus-chat 测试 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provider 从 API 获取模型列表并校验模型切换与启动默认模型；保留 agent "0" 建会话但不调 memory-ego（用 AgentConfig 默认系统提示词）；新增 nexus-chat 集成测试（channel-client-cli + channel-web + nexus + deepseek 真实文本通信，key 经脚本注入）。

**Architecture:** `Provider` trait 加 `list_models()`（OpenAI 兼容 `GET /models`、Anthropic `GET /v1/models`），`provider_for` 工厂共用；`resolve_effective_config` 对未配置模型用 provider 默认值合成；`Session.model` 改 `ArcSwap<Option<ProviderModel>>`（None=无模型，普通消息静默忽略）；`/model` 每次切换与启动 `default_model` 都经 API 校验；`session_key_for` 仅空 agent_id 脱离，"0" 建会话且 `build_initial_context` 用 `default_system_prompt` 替代 ego。测试：拆分 `inject-key.mjs`（可被 test import）+ `agent-reset.sh`；Playwright 编排。

**Tech Stack:** Rust, tokio, reqwest, arc-swap, Playwright(node ESM), shell

## Global Constraints

- 所有文件 UTF-8、`\n` 换行符
- 不删除代码注释
- 读写文件用 Read/Write/Edit 工具，禁止 sed/python 修改文件
- git commit 中文
- 字符串统一 `Arc<String>`；运行状态 Map 用 `Arc<DashMap<K,Arc<V>>>`；确需变更的运行状态标量用 `ArcSwap`
- 模型列表每次切换现调 API；失败拒绝切换（保持原模型）；启动校验失败进入无模型状态（普通消息静默忽略）
- 保留 agent "0"：不调 memory-ego、用 `default_system_prompt`；照常调 memory-store / history
- `/model` 命令格式不变：`/model <provider> <model>`

## File Structure

| 文件 | 职责 |
|------|------|
| `kissbot-agent/src/provider.rs` | Provider trait + `list_models()` + `provider_for` 工厂 |
| `kissbot-agent/src/model_client.rs` | `build_provider` 改用 `provider_for`；新增 `list_models(pm)`（从 ProviderConfig 构造后调 API） |
| `kissbot-agent/src/config_manager.rs` | `resolve_effective_config` 未配置模型合成；AgentConfig + `default_system_prompt`；`provider_config_by_name` |
| `kissbot-agent/src/session_manager.rs` | `Session.model: ArcSwap<Option<ProviderModel>>`；`get_or_create` 收 Option |
| `kissbot-agent/src/coordinator.rs` | 启动校验 default_model 缓存；`set_session_model` API 校验；`session_key_for` 仅空脱离；`build_initial_context` "0" 用默认提示词；`run_agentic_loop` 无模型静默忽略 |
| `config.json` / `script/config.json` / `test/workspace-template/config.json` | agent 段补 `default_system_prompt` |
| `script/inject-key.mjs` | 新建：导出 `injectApiKeys` + CLI 入口 |
| `script/agent-reset.sh` | mkdir + cp 模板 + 调 inject-key.mjs |
| `script/agent-reset.mjs` | 删除 |
| `test/workspace-template/agent-data/nexus.json` | models 加 `deepseek-v4-flash`；channel agent_id 置空（初始脱离） |
| `test/tests/helpers/server.ts` | 加 `injectAgentApiKeys()` |
| `test/tests/agent-commands.spec.ts` | 删 /model 用例 |
| `test/tests/nexus-chat.spec.ts` | 新建 |

---

### Task 1: Provider.list_models + provider_for 工厂

**Files:**
- Modify: `kissbot-agent/src/provider.rs`
- Modify: `kissbot-agent/src/model_client.rs`
- Test: `kissbot-agent/src/provider.rs`（内联 `#[cfg(test)]`）

**Interfaces:**
- Consumes: 无
- Produces: `Provider::list_models() -> Result<Vec<String>>`、`provider_for(client, provider_type, base_url, api_key) -> Box<dyn Provider>`、`parse_openai_models(data)`/`parse_anthropic_models(data)`（测试用）

- [ ] **Step 1: Provider trait 加 list_models + provider_for**

`src/provider.rs`：

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse>;
    /// 从服务商 API 获取全部可用模型名（GET /models）
    async fn list_models(&self) -> Result<Vec<String>>;
}

/// 按 provider_type 构造 Provider 实现（"openai" | "anthropic"）
pub fn provider_for(client: Arc<reqwest::Client>, provider_type: &str, base_url: &str, api_key: &str) -> Box<dyn Provider> {
    match provider_type {
        "openai" => Box::new(OpenAiProvider::new(client, base_url, api_key)),
        "anthropic" => Box::new(AnthropicProvider::new(client, base_url, api_key)),
        _ => panic!("provider_for: 未知 provider_type: {}", provider_type),
    }
}
```

> `provider_for` 对未知类型 `panic`（与调用方先校验 provider_type 配合；`ModelClient::call` 当前对未知类型返回 `Error::ModelProviderNotSupported`，见 Step 3 调整——若调用方总能保证类型合法则 panic 可接受，否则改为返回 `Result<Box<dyn Provider>>`，实现时按调用链选择并保持一致）。

- [ ] **Step 2: OpenAi/Anthropic 实现 list_models（解析函数抽出来便于测试）**

`src/provider.rs` 追加：

```rust
fn parse_openai_models(data: &serde_json::Value) -> Vec<String> {
    data["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn parse_anthropic_models(data: &serde_json::Value) -> Vec<String> {
    data["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default()
}
```

`OpenAiProvider` 的 `impl Provider` 增加：

```rust
    async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let resp = self.client.get(&url)
            .timeout(Duration::from_secs(30))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ModelApiError(format!("OpenAI models API {}: {}", status, text)));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(parse_openai_models(&data))
    }
```

`AnthropicProvider` 的 `impl Provider` 增加（`GET {base_url}/v1/models`，`x-api-key` + `anthropic-version` 头，`parse_anthropic_models`）。

- [ ] **Step 3: model_client.rs 的 build_provider 改用 provider_for**

`src/model_client.rs`：`build_provider` 内改用 `crate::provider::provider_for(self.client.clone(), &effective.provider_type, &effective.base_url, &effective.api_key)`（删除原 match 或改为委托）。

- [ ] **Step 4: 测试（解析 + provider_for 分发）**

`src/provider.rs` 的 `tests` 模块追加：

```rust
    #[test]
    fn parse_openai_models_extracts_ids() {
        let data = serde_json::json!({ "data": [ { "id": "deepseek-chat" }, { "id": "deepseek-reasoner" } ] });
        assert_eq!(parse_openai_models(&data), vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()]);
    }

    #[test]
    fn parse_anthropic_models_extracts_ids() {
        let data = serde_json::json!({ "data": [ { "id": "claude-3-5" } ] });
        assert_eq!(parse_anthropic_models(&data), vec!["claude-3-5".to_string()]);
    }

    #[test]
    fn provider_for_dispatches_by_type() {
        let client = Arc::new(reqwest::Client::new());
        assert!(matches!(provider_for(client.clone(), "openai", "u", "k").provider_type_of(), ...)); // 见下注
    }
```

> 注：`provider_for` 返回 `Box<dyn Provider>`，无法直接断言具体类型。改为断言 `provider_for(...)` 的 `provider_type`——给 `Provider` trait 加一个默认方法 `fn provider_type(&self) -> &str`（OpenAi 返回 "openai"、Anthropic 返回 "anthropic"），测试断言之；若不想加该方法，测试改为"未知类型 panic"（`#[should_panic]`）并跳过分发断言。

- [ ] **Step 5: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过；provider 测试（原 4 个 + 新增）PASS。

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/provider.rs kissbot-agent/src/model_client.rs
git commit -m "feat(agent): Provider.list_models 从 API 获取模型列表 + provider_for 工厂"
```

---

### Task 2: resolve_effective_config 支持未配置模型

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`
- Test: `kissbot-agent/src/config_manager.rs`

**Interfaces:**
- Consumes: `ProviderModel`（已存在）
- Produces: `resolve_effective_config(pm) -> Option<EffectiveModelConfig>`：provider 不存在 → None；model 未配置 → 用 provider 默认值合成

- [ ] **Step 1: 修改 resolve_effective_config**

`src/config_manager.rs`：

```rust
    /// 合成 provider 默认 + model 覆盖的有效参数（每次调用现场合成，配置永远最新）
    /// model 未在 provider.models 配置时用 provider 默认值合成（极端 models={} 也可用）
    pub async fn resolve_effective_config(&self, pm: &ProviderModel) -> Option<EffectiveModelConfig> {
        let repo = self.nexus_repo.read().await;
        let provider = repo.providers.get(&pm.provider)?.load_full();
        let model_cfg = provider.models.get(&pm.model).map(|s| s.load_full());
        Some(EffectiveModelConfig {
            provider_type: provider.provider_type.clone(),
            base_url: provider.base_url.clone(),
            api_key: provider.api_key.clone(),
            model: pm.model.clone(),   // 用切换指令的模型名（未配置也有效）
            max_tokens: model_cfg.as_ref().and_then(|m| m.max_tokens).unwrap_or(provider.default_max_tokens),
            temperature: model_cfg.as_ref().and_then(|m| m.temperature).unwrap_or(provider.default_temperature),
            timeout_secs: model_cfg.as_ref().and_then(|m| m.timeout_secs).unwrap_or(provider.default_timeout_secs),
            retry_count: model_cfg.as_ref().and_then(|m| m.retry_count).unwrap_or(provider.default_retry_count),
            context_length: model_cfg.as_ref().and_then(|m| m.context_length).unwrap_or(provider.default_context_length),
        })
    }
```

- [ ] **Step 2: 更新既有测试**

`resolve_effective_config_missing_returns_none`（现断言 model "nope" → None）改为：provider "nope" → None（不变）；**model "nope" → Some（合成，参数取 provider 默认值）**。检查其它引用该语义的测试并同步。测试里用 `assert_eq!(eff.max_tokens, provider 默认值)` 之类断言合成结果。

- [ ] **Step 3: 新增测试：models={} 也能合成**

```rust
    #[tokio::test]
    async fn resolve_effective_config_synthesizes_unconfigured_model() {
        // manager 构造见既有测试 helper；provider.deepseek 的 models 为空（或不含目标 model）
        let eff = manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "deepseek-v4-flash".into() }).await;
        let eff = eff.expect("model 未配置也应合成");
        assert_eq!(eff.model, "deepseek-v4-flash");
        assert_eq!(eff.max_tokens, 4096);          // provider 默认值
        assert_eq!(eff.temperature, 0.7);
        assert_eq!(eff.timeout_secs, 60);
    }
```

- [ ] **Step 4: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过；config_manager 全部测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/config_manager.rs
git commit -m "feat(agent): resolve_effective_config 对未配置模型用 provider 默认值合成"
```

---

### Task 3: AgentConfig.default_system_prompt + config.json 更新

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`
- Modify: `config.json`、`script/config.json`、`test/workspace-template/config.json`
- Test: `kissbot-agent/src/config_manager.rs`

**Interfaces:**
- Produces: `AgentConfig.default_system_prompt: Arc<String>`、getter `default_system_prompt() -> &str`

- [ ] **Step 1: AgentConfig 加字段 + getter**

`src/config_manager.rs`：

```rust
pub struct AgentConfig {
    pub data_dir: Arc<String>,
    pub mgmt_host: Arc<String>,
    pub mgmt_port: u16,
    pub ws_reconnect_interval_secs: u64,
    pub default_system_prompt: Arc<String>,   // 保留 agent "0" 的默认系统提示词（不调 memory-ego 时用）
    pub init_agent_id: Arc<String>,
    pub init_role: Arc<String>,
    pub init_model: Arc<String>,
}
```

`impl AgentConfig` 加：`pub fn default_system_prompt(&self) -> &str { &self.agent_config... }`——注意 getter 在 `ConfigManager` 上（`self.agent_config.default_system_prompt`）：

```rust
    pub fn default_system_prompt(&self) -> &str { &self.agent_config.default_system_prompt }
```

- [ ] **Step 2: 测试 helper 与 AgentConfig 构造补字段**

`config_manager.rs` 测试里 `fn agent_config(data_dir)` 构造补 `default_system_prompt: Arc::new("你是 kissbot 智能助手".into())`。

- [ ] **Step 3: 三个 config.json 补 agent 段字段**

`config.json`、`script/config.json`、`test/workspace-template/config.json` 的 `agent` 段加：

```json
"default_system_prompt": "你是 kissbot 智能助手",
```

用 Read 先看各文件当前 agent 段，用 Edit 加（不删既有内容）。

- [ ] **Step 4: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过；config_manager 测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/config_manager.rs config.json script/config.json test/workspace-template/config.json
git commit -m "feat(agent): AgentConfig 增加 default_system_prompt（保留 agent 0 默认系统提示词）"
```

---

### Task 4: Session.model 改 Option + 无模型静默忽略

**Files:**
- Modify: `kissbot-agent/src/session_manager.rs`
- Modify: `kissbot-agent/src/coordinator.rs`
- Test: `kissbot-agent/src/session_manager.rs`

**Interfaces:**
- Consumes: Task 3（default_system_prompt，Task 6 用）
- Produces: `Session.model: ArcSwap<Option<ProviderModel>>`、`get_or_create(key, model: Option<ProviderModel>)`

- [ ] **Step 1: Session.model 与 get_or_create 改 Option**

`src/session_manager.rs`：
- `pub model: ArcSwap<ProviderModel>` → `pub model: ArcSwap<Option<ProviderModel>>`
- `Session::new(key, model: ProviderModel)` → `model: Option<ProviderModel>`，`model: ArcSwap::from_pointee(model)`
- `get_or_create(&self, key, model: ProviderModel)` → `model: Option<ProviderModel>`，`Session::new(key.clone(), model)`

`src/coordinator.rs` `ensure_session`：`get_or_create(key, <验证后的默认模型>)`（Task 5 提供缓存值，本轮先传 `Some(self.config.default_model().await)` 保持可编译，Task 5 改）。

- [ ] **Step 2: run_agentic_loop 无模型静默忽略**

`src/coordinator.rs` `run_agentic_loop` 顶部：

```rust
    async fn run_agentic_loop(&self, channel_id: &str, session: &Arc<Session>, incoming: Arc<IncomingMessage>) {
        // 无可用模型：静默忽略普通消息（仅管理指令可用）
        if session.model.load().is_none() {
            return;
        }
        let content_text = extract_text(&incoming.content);
        // ...（其余不变；原 `let model = session.model.load_full();` 改为 `let Some(model) = session.model.load_full().as_ref() else { return; };` 后传给 mc.call）
```

> `mc.call(&model, &messages)` 处：`session.model.load_full()` 返回 `Arc<Option<ProviderModel>>`，取 `as_ref().as_ref()` 得 `&ProviderModel`（或 `let Some(pm) = ...as_deref() else { return; }`），调用 `mc.call(pm, &messages)`。

- [ ] **Step 3: 测试**

`session_manager.rs` 测试追加：

```rust
    #[test]
    fn get_or_create_with_none_model() {
        let mgr = SessionManager::new();
        let key = SessionKey { agent_id: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let (s, created) = mgr.get_or_create(&key, None);
        assert!(created);
        assert!(s.model.load().is_none());
    }
```

> `SessionManager::new()` 与 `SessionKey` 的构造参照既有测试（`new` 存在、`SessionKey` 字段见 types.rs）。

- [ ] **Step 4: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过；session_manager + 既有测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/session_manager.rs kissbot-agent/src/coordinator.rs
git commit -m "feat(agent): Session.model 改 Option，无模型时普通消息静默忽略"
```

---

### Task 5: 模型校验（启动 default_model + /model 切换）

**Files:**
- Modify: `kissbot-agent/src/model_client.rs`
- Modify: `kissbot-agent/src/config_manager.rs`（`provider_config_by_name`）
- Modify: `kissbot-agent/src/coordinator.rs`
- Test: `kissbot-agent/src/config_manager.rs`（provider 查询 getter）

**Interfaces:**
- Consumes: Task 1（`provider_for`）、Task 4（`Session.model: Option`）
- Produces: `ModelClient::list_models(&ProviderModel) -> Result<Vec<String>>`、coordinator 字段 `valid_default: ArcSwap<Option<ProviderModel>>`

- [ ] **Step 1: config_manager 加 provider 查询 getter**

`src/config_manager.rs`：

```rust
    pub async fn provider_config_by_name(&self, name: &str) -> Option<Arc<ProviderConfig>> {
        self.nexus_repo.read().await.providers.get(name).map(|s| s.load_full())
    }
```

测试：构造 manager，`provider_config_by_name("deepseek")` 返回 Some、`("nope")` 返回 None。

- [ ] **Step 2: model_client 加 list_models**

`src/model_client.rs`：

```rust
    /// 从服务商 API 获取全部模型名（按 pm.provider 的 ProviderConfig 构造 Provider）
    /// 返回 Err 表示 API 调用失败（网络/鉴权）
    pub async fn list_models(&self, pm: &ProviderModel) -> Result<Vec<String>> {
        let pc = self.config_manager.provider_config_by_name(&pm.provider).await
            .ok_or_else(|| Error::ModelProviderNotSupported(format!("provider 不存在: {}", pm.provider)))?;
        let provider = crate::provider::provider_for(
            self.client.clone(), &pc.provider_type, &pc.base_url, &pc.api_key);
        provider.list_models().await
    }
```

- [ ] **Step 3: coordinator 启动校验 default_model + 缓存字段**

`src/coordinator.rs`：

- 结构体加字段：`valid_default: ArcSwap<Option<ProviderModel>>`（启动校验结果，None=无模型）
- `new()` 中（构造 coordinator 后）：

```rust
        // 启动校验 default_model：从 API 拉模型列表，不在列表则无模型（告警）
        let default_model = config.default_model().await;
        let valid_default = match coordinator.model_client.lock().await.list_models(&default_model).await {
            Ok(list) if list.iter().any(|m| m == &default_model.model) => Some(default_model.clone()),
            Ok(_) => { tracing::warn!("default_model {}/{} 不在 API 模型列表", default_model.provider, default_model.model); None }
            Err(e) => { tracing::warn!("校验 default_model 失败（API 不可用?）: {:?}", e); None }
        };
        coordinator.valid_default.store(Arc::new(valid_default));
```

> `model_client` 是 `Arc<tokio::sync::Mutex<ModelClient>>`；注意 `new()` 里构造顺序（`valid_default` 字段初始化在 struct 构造时用 `ArcSwap::from_pointee(None)`，之后 store 校验结果）。

- `ensure_session` 改用 `self.valid_default.load_full().as_ref().clone()`（替代 Task 4 的临时 `Some(...)`）：

```rust
    async fn ensure_session(&self, key: &SessionKey) -> (Arc<Session>, bool) {
        let model = (*self.valid_default.load()).clone();
        let (session, created) = self.session_manager.get_or_create(key, model);
        if created {
            self.build_initial_context(&session).await;
        }
        (session, created)
    }
```

> `valid_default.load()` 返回 Guard deref 到 `Option<ProviderModel>`，`.clone()` 得 `Option<ProviderModel>`。

- [ ] **Step 4: set_session_model 改 API 校验**

`src/coordinator.rs` `set_session_model`（替换原 `resolve_effective_config(...).is_none()` 校验）：

```rust
    pub async fn set_session_model(&self, channel_id: &str, pm: ProviderModel) -> Result<()> {
        let Some(ch) = self.channel_config(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let Some(key) = self.session_key_for(&ch) else {
            return Err(Error::InvalidCommand("channel 未关联 agent，无法设置模型".to_string()));
        };
        // 每次切换都从 API 拉模型列表校验（失败拒绝，保持原模型）
        let models = self.model_client.lock().await.list_models(&pm).await
            .map_err(|e| Error::ModelApiError(format!("获取模型列表失败: {}", e)))?;
        if !models.iter().any(|m| m == &pm.model) {
            return Err(Error::ModelProviderNotSupported(format!(
                "模型 {} 不在 {} 的 API 模型列表", pm.model, pm.provider)));
        }
        let (session, _) = self.ensure_session(&key).await;
        session.model.store(Arc::new(Some(pm)));
        Ok(())
    }
```

- [ ] **Step 5: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过；全部测试 PASS（模型校验为集成路径，nexus-chat 覆盖；本任务单元测试覆盖 `provider_config_by_name`）。

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/model_client.rs kissbot-agent/src/config_manager.rs kissbot-agent/src/coordinator.rs
git commit -m "feat(agent): 模型校验——启动 default_model 与 /model 切换都经 API 列表校验，失败进入无模型状态"
```

---

### Task 6: 保留 agent "0" 行为

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`
- Test: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: Task 3（`default_system_prompt`）、Task 4（`Session.model: Option`）
- Produces: `session_key_for` 仅空脱离；`build_initial_context` "0" 用默认提示词

- [ ] **Step 1: session_key_for 仅空 agent_id 脱离**

`src/coordinator.rs`：

```rust
    /// 按来源 channel 的绑定配置 + 运行态 mode 计算会话 key；agent 为空（未设置）返回 None
    fn session_key_for(&self, ch: &crate::config_manager::ChannelConfig) -> Option<SessionKey> {
        let agent_id = ch.agent_id.to_string();
        if agent_id.is_empty() {
            return None; // 脱离 agent：只处理管理命令
        }
        // 保留 agent "0"（无参 /agent）同样建会话，初始上下文用默认系统提示词（见 build_initial_context）
        let mode = self.session_manager.channel_mode(&ch.channel_id);
        Some(SessionKey {
            agent_id,
            role_name: ch.role_name.to_string(),
            mode,
        })
    }
```

- [ ] **Step 2: build_initial_context 按 agent 分流**

`src/coordinator.rs`：

```rust
    /// 会话创建/重置时：加载 ego（"0" 用默认提示词）+ 历史记录 + 顶层记忆索引构建初始上下文
    async fn build_initial_context(&self, session: &Arc<Session>) {
        // 保留 agent "0" 不调 memory-ego，用 AgentConfig 默认系统提示词；其余 agent 走 load_ego_info
        if session.key.agent_id == RESERVED_AGENT_ID {
            session.context.lock().await.set_system_message(self.config.default_system_prompt().to_string());
        } else if let Ok(ego_info) = self.load_ego_info(&session.key.agent_id, &session.key.role_name).await {
            session.context.lock().await.set_system_message(ego_info);
        }
        // 历史记忆照常加载（"0" 也调 memory-store；URL 空则优雅跳过）
        if let Ok(history) = self.memory_reader
            .read_history(&self.config, &session.key.agent_id, &session.key.role_name, &session.key.mode)
            .await
        {
            session.context.lock().await.load_history(history);
        }
        // 顶层记忆索引（memory-struct 未实现时静默跳过）——保持不变
    }
```

- [ ] **Step 3: 测试 session_key_for**

`src/coordinator.rs` 测试（构造最小 `ChannelConfig` + `SessionManager`，参照既有 coordinator 测试）追加：

```rust
    #[tokio::test]
    async fn session_key_for_empty_detaches_but_zero_attaches() {
        // 构造 coordinator 或提取 session_key_for 为静态/独立函数测试：
        // agent_id="" → None（脱离）；agent_id="0" → Some（保留 agent 建会话）
    }
```

> 若直接构造 `AgentCoordinator` 依赖较多，可将 `session_key_for` 逻辑抽为纯函数 `fn session_key_of(agent_id: &str, role_name: &str, mode: Mode) -> Option<SessionKey>`，`session_key_for` 委托之，测试直接测纯函数（""→None，"0"→Some，其他→Some）。

- [ ] **Step 4: 编译 + 测试**

Run: `cd kissbot-agent && cargo test`
Expected: 编译通过；全部测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "feat(agent): 保留 agent 0 建会话但不调 memory-ego（用 default_system_prompt），照常调 history"
```

---

### Task 7: 脚本拆分 inject-key.mjs + agent-reset.sh

**Files:**
- Create: `script/inject-key.mjs`
- Rewrite: `script/agent-reset.sh`
- Delete: `script/agent-reset.mjs`

**Interfaces:**
- Produces: `injectApiKeys(keyFile, nexusPath) -> Promise<void>`（可被 test import）、CLI `node inject-key.mjs <keyFile> <nexusPath>`

- [ ] **Step 1: 新建 inject-key.mjs**

```js
// 注入 api key 到 nexus.json：
// 从 key 文件（{"provider名":"key"}）按 provider 名注入 nexus.providers[].api_key，就地写回。
// 可被 test import（injectApiKeys），也可 CLI 调用：node inject-key.mjs <key文件> <nexus.json路径>
import { readFile, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';

export async function injectApiKeys(keyFile, nexusPath) {
  const nexus = JSON.parse(await readFile(nexusPath, 'utf8'));
  const providers = nexus.providers || {};
  if (existsSync(keyFile)) {
    const keys = JSON.parse(await readFile(keyFile, 'utf8'));
    for (const [name, key] of Object.entries(keys)) {
      if (providers[name]) {
        providers[name].api_key = key;
        console.log(`  ✓ ${name}: api_key 已注入`);
      } else {
        console.warn(`  ⚠ ${name}: nexus.json 中没有名为 ${name} 的 provider，跳过`);
      }
    }
    for (const [name, provider] of Object.entries(providers)) {
      if (!provider.api_key) {
        console.warn(`  ⚠ provider ${name} 未配置 api_key（key 文件中无对应条目）`);
      }
    }
  } else {
    console.warn(`  ⚠ ${keyFile} 不存在，api_key 保持为空`);
  }
  await writeFile(nexusPath, JSON.stringify(nexus, null, 2) + '\n', 'utf8');
}

// CLI 入口（被 import 时不执行）
if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [keyFile, nexusPath] = process.argv.slice(2);
  if (!keyFile || !nexusPath) {
    console.error('用法: node inject-key.mjs <key.local.json路径> <nexus.json路径>');
    process.exit(1);
  }
  injectApiKeys(keyFile, nexusPath).catch((e) => { console.error('inject-key 失败:', e); process.exit(1); });
}
```

> 顶部需 `import { pathToFileURL } from 'node:url';`。

- [ ] **Step 2: 重写 agent-reset.sh（模板复制进 shell）**

```bash
#!/bin/bash
# 重置 agent 数据：从模板生成 nexus.json/station.json 到数据目录，并从 key 文件注入 api key
# 用法: ./reset-agent.sh [数据目录] [key.local.json路径]
#       默认数据目录 <项目根>/workspace/agent-data，默认 key 文件 <项目根>/key.local.json
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DATA_DIR="${1:-$PROJECT_DIR/workspace/agent-data}"
KEY_FILE="${2:-$PROJECT_DIR/key.local.json}"

echo "==> 重置 agent 数据..."
mkdir -p "$DATA_DIR"
cp "$SCRIPT_DIR/template/nexus.json" "$DATA_DIR/nexus.json"
cp "$SCRIPT_DIR/template/station.json" "$DATA_DIR/station.json"
node "$SCRIPT_DIR/inject-key.mjs" "$KEY_FILE" "$DATA_DIR/nexus.json"
echo "==> 完成"
```

- [ ] **Step 3: 删除 agent-reset.mjs**

```bash
git rm script/agent-reset.mjs
```

- [ ] **Step 4: 验证**

Run: `script/reset-agent.sh`
Expected: 生成 workspace/agent-data/nexus.json、station.json；用假 key 验证注入（`{"deepseek":"sk-test"}` 临时写入根 key.local.json 后恢复 `{}`）；`node script/inject-key.mjs <key> <nexus>` 直接调用也可注入。

- [ ] **Step 5: Commit**

```bash
git add -A script
git commit -m "refactor(agent): 拆分 inject-key.mjs（可被 test import）+ agent-reset.sh 承载模板复制"
```

---

### Task 8: 测试基建（模板 nexus + injectAgentApiKeys + agent-commands 删 /model）

**Files:**
- Modify: `test/workspace-template/agent-data/nexus.json`
- Modify: `test/tests/helpers/server.ts`
- Modify: `test/tests/agent-commands.spec.ts`

**Interfaces:**
- Consumes: Task 7（`injectApiKeys`）
- Produces: `injectAgentApiKeys()`（helpers/server.ts）

- [ ] **Step 1: test 模板 nexus.json 加 deepseek-v4-flash + channel 初始脱离**

`test/workspace-template/agent-data/nexus.json`：
- `providers.deepseek.models` 加 `"deepseek-v4-flash": { "model": "deepseek-v4-flash" }`
- channel `web-main` 的 `agent_id` 改为 `""`（初始脱离，nexus-chat 用无参 `/agent` 挂到 "0"）

用 Read 先看当前文件，用 Edit 改。

- [ ] **Step 2: helpers/server.ts 加 injectAgentApiKeys**

`test/tests/helpers/server.ts` 顶部 import 注入函数（若 TS 对 .mjs 静态 import 报模块解析错误，用动态 import）：

```ts
import { injectApiKeys } from '../../../script/inject-key.mjs';
// 或：const { injectApiKeys } = await import('../../../script/inject-key.mjs');

export async function injectAgentApiKeys(): Promise<void> {
  const keyFile = join(REPO_ROOT, 'key.local.json');
  const nexus = join(REPO_ROOT, 'test', 'workspace', 'agent-data', 'nexus.json');
  await injectApiKeys(keyFile, nexus);
}
```

- [ ] **Step 3: agent-commands.spec.ts 删 /model 用例**

删除 `TC-01: 非管理员（u3）发送 /model 被忽略` 与 `TC-04: u3 成为管理员后发送 /model 调整会话模型` 两个 `test(...)` 块（及仅被它们引用的临时变量/注释）。其余用例与编号顺序保持可运行（编号可顺延或保留，不强制重排）。

- [ ] **Step 4: 编译/语法验证**

Run: `cd kissbot-agent && cargo test`（agent 不受影响，确认仍绿）
Run: `npx tsc --noEmit -p test/tsconfig.json`（或 `cd test && npx tsc --noEmit`）
Expected: TS 编译通过（若 inject-key import 报类型错误，改用动态 import）。

- [ ] **Step 5: Commit**

```bash
git add test/workspace-template/agent-data/nexus.json test/tests/helpers/server.ts test/tests/agent-commands.spec.ts
git commit -m "test(agent): 测试模板加 deepseek-v4-flash 与初始脱离 channel，helpers 加 injectAgentApiKeys，agent-commands 删 /model 用例"
```

---

### Task 9: nexus-chat.spec.ts

**Files:**
- Create: `test/tests/nexus-chat.spec.ts`

**Interfaces:**
- Consumes: Task 8（`injectAgentApiKeys`）、`resetWorkspace/startBackend/startAgent/waitForPort`、`spawnCli`

- [ ] **Step 1: 新建 spec**

```ts
import { test, expect } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, startAgent, stopAgent, waitForPort, injectAgentApiKeys } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let backend: ChildProcess;
let agent: ChildProcess;
let cli: SpawnedCli;   // u2（管理员）经 channel-web 与 nexus 通信

function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

test.describe.serial('nexus-chat：真实 LLM 文本通信（deepseek）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    await injectAgentApiKeys();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
    await sleep(2000);   // 等待 agent 完成 channel 连接与绑定
    cli = spawnCli(['web', 'u2', 'g1', './downloads'], WORKSPACE);
    await cli.waitForOutput(/bound\./);
  });

  test.afterAll(() => {
    if (cli) cli.proc.kill();
    stopAgent(agent);
    stopBackend(backend);
  });

  test('TC-1: 无参 /agent 把 channel 挂到保留 agent 0', async () => {
    cli.stdin('/send /agent');
    await cli.waitForOutput(/✅ 已设置 agent: 0 \/ role: 0/, 10000);
  });

  test('TC-2: /model 切换到 deepseek-v4-flash（真实 API 校验）', async () => {
    cli.stdin('/send /model deepseek deepseek-v4-flash');
    await cli.waitForOutput(/✅ 已切换模型为: deepseek\/deepseek-v4-flash/, 20000);
  });

  test('TC-3: 普通文本消息得到真实 LLM 非空回复', async () => {
    const baseline = cli.getOutput().length;
    cli.stdin('/send 你好，请用一句话自我介绍');
    // 等待 agent 回复出现在 CLI 输出（真实 LLM 延迟较长，放宽超时）
    await cli.waitForOutput(/./, 60000);
    const tail = cli.getOutput().slice(baseline);
    // 回复应包含非空文本内容（去掉 /send 回显后仍有内容）
    expect(tail.trim().length).toBeGreaterThan(0);
  });
});
```

> `spawnCli` 返回 `{ proc, stdin, waitForOutput, hasOutput, getOutput }`（见 helpers/cli.ts）。TC-3 的 `waitForOutput(/./)` 会立即命中（CLI 有输出），故用"基线 + 等待 agent 回复关键词或延时轮询"更稳：实现时优先**等一个明确的回复特征**（如 agent 回复行以消息时间戳/发送者前缀开头，参照 channel-cli.spec 的断言方式），否则退化为 `sleep(等待) + tail 非空`。TC-3 依赖根目录 key.local.json 的 deepseek key 有效且 `deepseek-v4-flash` 是该账号真实模型名——若不匹配会校验失败（TC-2 红）或 LLM 报错，届时核对模型名与 key。

- [ ] **Step 2: 跑测试（需要真实 key）**

Run: `cd test && npx playwright test tests/nexus-chat.spec.ts`
Expected: 需根目录 `key.local.json` 含有效 deepseek key（`{"deepseek":"sk-..."}`）。TC-1/2/3 通过；若 TC-2/3 失败，检查模型名是否为账号真实模型 id、key 是否有效。

- [ ] **Step 3: 全量 agent 测试回归**

Run: `cd kissbot-agent && cargo test`（应全绿）
Run: `cd test && npx playwright test tests/agent-commands.spec.ts`（应通过——已删 /model，不依赖 key）

- [ ] **Step 4: Commit**

```bash
git add test/tests/nexus-chat.spec.ts
git commit -m "test(agent): 新增 nexus-chat 集成测试（channel-cli + channel-web + nexus + deepseek 真实文本通信）"
```

---

## Self-Review 笔记

- **Spec 覆盖**：Part1 模型列表（Task1/2/5）、Part2 保留 agent "0"（Task3/6）、Part3 nexus-chat（Task7/8/9）均覆盖。
- **注意**：`resolve_effective_config` 语义变化（model 未配置不再 None）会使既有测试 `resolve_effective_config_missing_returns_none` 需同步改（Task2 Step2）。`ModelClient::call` 内部对未配置模型现在会合成——`/model` 切换已用 list_models 把关（Task5），`run_agentic_loop` 用会话模型（经校验），故合成路径只服务已校验模型。
- **风险**：nexus-chat 依赖真实 deepseek key + `deepseek-v4-flash` 为账号真实模型 id；key.local.json 为空或模型名不符时 TC-2/3 红（预期，属环境问题非代码缺陷）。startup 校验 default_model（模板为 deepseek-4-flash）在 key 有效且该模型在列表时成功；若不在列表则无模型，TC-3 静默忽略——测试前应确认模板 default_model 也在列表，或 nexus-chat 先 `/model` 切到 deepseek-v4-flash 后再发文本（TC-3 依赖 TC-2 已切换）。
- **类型一致性**：`Session.model` Option 化后，`set_session_model`/`ensure_session`/`run_agentic_loop` 三处消费点同步；`get_or_create` 签名统一 `Option<ProviderModel>`。
