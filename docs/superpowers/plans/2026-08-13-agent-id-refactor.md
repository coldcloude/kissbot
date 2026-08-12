# Agent 标识统一改造（agent_name → agent_id）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** agent 侧消灭 `agent_name`，会话标识全链路改用 `agent_id`（UUID 或保留值 `"0"`），删除 agent_name → agent_id 的运行时解析/缓存链。

**Architecture:** 配置直接存 `agent_id`（`ChannelConfig.agent_name` 改名 + serde 归一化 `""`→`"0"`）；删除 coordinator 的运行时绑定链（channel 不再缓存 agent_id，`Session.agent_id` 直接来自 SessionKey）；`/agent` 命令直接接受 UUID 并经 ego `/agent/get` 校验存在。常量 `RESERVED_AGENT_ID` 从 coordinator 迁至 types.rs（config_manager 的 serde 归一化也需要它）。

**Tech Stack:** Rust，serde（自定义反序列化），reqwest（ego HTTP），axum（HTTP 服务），tokio。

## Global Constraints

- 仅改动 `kissbot-agent`；kissbot-memory-ego 的 `/agent/search-name` 接口保留，本次不动
- 不变式：`agent_id` 恒非空；保留 agent 显式用 `"0"`（`RESERVED_AGENT_ID`）
- 入口归一化规则：程序加载、API 修改、命令修改三处入口遇到空串自动变 `"0"`，不拒绝
- 中间层不归一化、不校验：`session_key_for` 直接构造，无纯函数构造器
- 不改 `role_name` 语义（空串 = 保留 role）
- 提交 comment 用中文，包含该提交所有改动

**任务切分说明**：本重构受 Rust 单 crate 编译单元约束——`ChannelConfig.agent_name` 字段改名会同步破坏 coordinator/command_router 的引用，且 `channel_manager` 的 `set_agent_id`/`agent_id` 方法已在上一提交删除（coordinator 当前调用已断），故**改名与删绑定链必须同一次编译内完成**，无法拆成各自独立编译通过的更小任务。Task 1 的 step 1-5 期间 crate 预期不编译，step 6 统一验证恢复。

---

### Task 1: 全量改名 agent_name → agent_id + 删除运行时绑定链（编译恢复）

**Files:**
- Modify: `kissbot-agent/src/types.rs`
- Modify: `kissbot-agent/src/config_manager.rs`
- Modify: `kissbot-agent/src/coordinator.rs`
- Modify: `kissbot-agent/src/command_router.rs`
- Modify: `kissbot-agent/src/http_server.rs`

**Interfaces:**
- Consumes: 无（本任务是重构起点；当前 crate 因上一提交的字段改名遗留而编译失败）
- Produces:
  - `types::RESERVED_AGENT_ID: &str = "0"`（从 coordinator 迁入）
  - `config_manager::ChannelConfig.agent_id: Arc<String>`（serde 缺省/空串 → `"0"`）
  - `config_manager::ConfigManager::context_config(&self, agent_id: &str, role_name: &str) -> EffectiveContextConfig`
  - `coordinator::AgentCoordinator::change_channel_key(&self, channel_id: &str, new_key: SessionKey) -> Result<()>`（无 agent_id 参数）
  - `coordinator::AgentCoordinator::verify_agent_exists(&self, agent_id: &str) -> Result<()>`
  - `coordinator::AgentCoordinator::channel_session_key(&self, channel_id: &str) -> Option<SessionKey>`（复用 session_key_for）
  - `SessionKey { agent_id: String, role_name: String, mode: Mode }`（字段已存在，本任务补齐所有引用点）

- [ ] **Step 1: types.rs 迁入 RESERVED_AGENT_ID 常量**

在 `// ========== 会话标识 ==========` 注释块、`SessionKey` 结构体定义之前插入：

```rust
// ========== 会话标识 ==========

/// 保留 agent 的 memory-store/ego agent_id（"0"）：配置缺省/空串归一化目标，会话构建判保留用
pub const RESERVED_AGENT_ID: &str = "0";

/// 会话唯一标识：agent_id + role_name + mode 三元组
/// 所有绑定 channel 的信息去重，每个三元组 = 一个会话
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub agent_id: String,
    pub role_name: String,
    pub mode: Mode,
}
```

**1b. `AdminCommand::SetAgent` 注释更新**（types.rs 管理命令区，字段名已改，注释同步保留语义）：

```rust
    /// 设置 channel 绑定的 agent 与 role（缺省用保留值：agent_id="0"、role_name=""）
    SetAgent { agent_id: Option<String>, role: Option<String> },
```

- [ ] **Step 2: config_manager.rs 字段改名 + serde 归一化 + context_config 签名**

**2a. import 更新**（文件头部 `use crate::types::{Result, Error};`）：

```rust
use crate::types::{Result, Error, RESERVED_AGENT_ID};
```

**2b. `NexusRepo.context` 字段注释**（`agent_name → AgentContextConfig` 改为 `agent_id → AgentContextConfig`）：

```rust
    /// agent_id → AgentContextConfig（上下文配置，三层继承见 merge_context_config）
    /// serde(default)：旧 nexus.json 无 context 段时反序列化为空 map（= 全局默认，兼容旧配置）
    #[serde(default)]
    pub context: Arc<ArcSwapHashMap<String, AgentContextConfig>>,
```

**2c. `ChannelConfig` 字段改名**：

```rust
    /// 绑定的 agent_id（UUID；缺省/空 = 保留 agent = "0"，建会话用默认系统提示词，不调 memory-ego）
    #[serde(default = "default_agent_id", deserialize_with = "deserialize_agent_id")]
    pub agent_id: Arc<String>,
```

**2d. 新增两个 serde helper 函数**（放在 `impl Default for NexusRepo` 之后、`ChannelConfig` 定义之前）：

```rust
/// ChannelConfig.agent_id 缺省值：保留 agent（"0"）
fn default_agent_id() -> Arc<String> {
    Arc::new(RESERVED_AGENT_ID.to_string())
}

/// ChannelConfig.agent_id 反序列化：空串自动归一化为保留 agent（"0"），非空原样
fn deserialize_agent_id<'de, D>(d: D) -> Result<Arc<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    Ok(Arc::new(if s.is_empty() { RESERVED_AGENT_ID.to_string() } else { s }))
}
```

**2e. context 配置结构注释**（`/// agent 级 context 配置（key = agent_name，覆盖全局默认；...`）：

```rust
/// agent 级 context 配置（key = agent_id，覆盖全局默认；类似 ProviderConfig 的 default_*）
```

**2f. `context_config` 签名与 doc**：

```rust
    /// 按 (agent_id, role_name) 合并 context 配置（三层继承：全局默认 ← agent ← role）
    pub async fn context_config(&self, agent_id: &str, role_name: &str) -> EffectiveContextConfig {
        let repo = self.nexus_repo.read().await;
        let agent = repo.context.get(agent_id).map(|s| s.load_full());
        let role = agent.as_ref().and_then(|a| a.roles.get(role_name).map(|s| s.load_full()));
        merge_context_config(agent.as_deref(), role.as_deref())
    }
```

**2g. config_manager 测试更新**：

`sample_channel` 中 `agent_name: Arc::new("".into())` → `agent_id: Arc::new("0".into())`；

`channel_config_bind_users_and_outgoing_roundtrip` 中 `agent_name: Arc::new("a1".into())` → `agent_id: Arc::new("a1".into())`（值非空即合法，保持最小改动）；

`channel_config_new_shape_serde_roundtrip` 中 `assert!(json.contains("\"agent_name\""));` → `assert!(json.contains("\"agent_id\""));`；

`update_channel_mutates_and_persists` 中三处：`c.agent_name = Arc::new("a1".into())` → `c.agent_id = Arc::new("a1".into())`、`assert_eq!(*ch.agent_name, "a1")` → `assert_eq!(*ch.agent_id, "a1")`、`assert_eq!(saved["channels"]["web-main"]["agent_name"], "a1")` → `assert_eq!(saved["channels"]["web-main"]["agent_id"], "a1")`；

注释 `// 修改 agent_name/role_name/bind_users` → `// 修改 agent_id/role_name/bind_users`。

另有两处独立测试调用 `manager.update_channel("web-main", |c| c.agent_name = Arc::new("a1".into()))`，一并改为 `c.agent_id`。

**2h. 新增 serde 归一化测试**（放在 `channel_config_old_shape_no_longer_parses` 测试之后）：

```rust
    #[test]
    fn channel_config_agent_id_empty_normalizes_to_reserved() {
        // 显式空串 → 归一化为 "0"
        let json = r#"{"channel_id":"c1","ws_url":"ws://127.0.0.1:8201","admins":[],"bind_users":[],"agent_id":"","role_name":"","enabled":true}"#;
        let ch: ChannelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(ch.agent_id.as_str(), "0", "空串应归一化为保留 id");
    }

    #[test]
    fn channel_config_agent_id_missing_defaults_to_reserved() {
        // 字段缺省 → "0"
        let json = r#"{"channel_id":"c1","ws_url":"ws://127.0.0.1:8201","admins":[],"bind_users":[],"role_name":"","enabled":true}"#;
        let ch: ChannelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(ch.agent_id.as_str(), "0", "缺省应回退保留 id");
    }
```

- [ ] **Step 3: coordinator.rs 改名 + 删绑定链 + 新增 verify_agent_exists**

**3a. import 更新**：

```rust
use crate::types::{
    Error, Message, Mode, ModelResponse, RESERVED_AGENT_ID, Result, SessionKey, ToolCall, memory_role,
};
```

**3b. 常量块**（删除 `RESERVED_AGENT_NAME` 与 `RESERVED_AGENT_ID`，仅保留 role 常量）：

```rust
/// 保留 role：空串 = 保留 role
pub const RESERVED_ROLE_NAME: &str = "";
```

**3c. `ConfigChange` 枚举**（删除 `agent_id` 字段）：

```rust
/// agent/role/event 变更任务（mpsc 队列串行处理，避免写-写竞态；读无需外部加锁）
/// 统一为「应用新的会话三元组」：写 config + 运行态 mode + 会话重定位
enum ConfigChange {
    /// 应用新会话三元组（agent/role/mode 任一变化）
    ApplyKey { channel_id: String, new_key: SessionKey, done: tokio::sync::oneshot::Sender<Result<()>> },
}
```

**3d. struct 字段注释**（`channel_manager` 说明去掉 agent_id）：

```rust
    /// 每 channel 运行时管理（ChannelManager：内部 DashMap 无锁并发，含 pending/mode/client）
    channel_manager: Arc<ChannelManager>,
```

**3e. `new()` 中变更消费者**：

```rust
                    ConfigChange::ApplyKey { channel_id, new_key, done } => {
                        let coordinator = AgentCoordinator::instance();
                        let rst = coordinator.apply_channel_key(&channel_id, &new_key).await;
                        let _ = done.send(rst);
                    }
```

**3f. `session_key_for` 直接构造**（删除 `session_key_of` 调用）：

```rust
    /// 按来源 channel 的绑定配置 + 运行态 mode 计算会话 key（agent/role 取绑定配置）
    fn session_key_for(&self, ch: &crate::config_manager::ChannelConfig) -> SessionKey {
        SessionKey {
            agent_id: ch.agent_id.to_string(),
            role_name: ch.role_name.to_string(),
            mode: self.channel_mode(&ch.channel_id),
        }
    }
```

**3g. 删除 5 个方法**：`channel_agent`、`bind_channel_runtime`、`set_channel_runtime`、`resolve_agent_id_for_bind`、`resolve_agent_id_http`（整段删除，含 doc 注释）。

**3h. `ensure_session` 删除 channel_agent 调用**：

```rust
    async fn ensure_session(&self, key: &SessionKey, channel_id: &str) -> (Arc<Session>, bool) {
        // valid_default.load_full() 返回 Arc<Option<ProviderModel>>，解引用克隆得 Option
        let model = (*self.valid_default.load_full()).clone();
        let (session, created) = self.session_manager.get_or_create(key, model);
```

**3i. `channel_session_key` 复用 session_key_for**：

```rust
    /// 取 channel 当前会话三元组（config 的 agent_id/role_name + 运行态 mode），命令构造新三元组用
    pub async fn channel_session_key(&self, channel_id: &str) -> Option<SessionKey> {
        let ch = self.config.channel(channel_id).await?;
        Some(self.session_key_for(&ch))
    }
```

**3j. `change_channel_key` 签名去掉 agent_id 参数**：

```rust
    /// agent/role/mode 变更统一入口：应用新会话三元组（写 config agent_id/role_name + 运行态 mode + 会话重定位），
    /// 走串行队列，返回时已生效
    pub async fn change_channel_key(&self, channel_id: &str, new_key: SessionKey) -> Result<()> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        self.command_tx.send(ConfigChange::ApplyKey {
            channel_id: channel_id.to_string(),
            new_key,
            done: done_tx,
        }).map_err(|_| Error::InternalError("变更队列已关闭".to_string()))?;
        done_rx.await.map_err(|_| Error::InternalError("变更处理中断".to_string()))?
    }
```

**3k. `apply_channel_key` 删除 agent_id 逻辑**：

```rust
    async fn apply_channel_key(&self, channel_id: &str, new_key: &SessionKey) -> Result<()> {
        self.config.update_channel(channel_id, |c| {
            c.agent_id = Arc::new(new_key.agent_id.clone());
            c.role_name = Arc::new(new_key.role_name.clone());
        }).await?;
        self.set_channel_mode(channel_id, new_key.mode.clone());
        self.relocate_channel(channel_id).await;
        Ok(())
    }
```

**3l. `run()` 删除 bind_channel_runtime 调用**：

```rust
        for (_, ch) in self.config.channels().await {
            let key = self.session_key_for(&ch);
            self.ensure_session(&key, &ch.channel_id).await;
        }
```

（同步删除其上注释中的「绑定运行态 agent（解析失败回退保留 agent），」字样。）

**3l2. `run()` 的 doc 注释更新**：

```rust
    /// 启动主循环（保持进程运行）：初始化会话 + 连接全部 channel
    pub async fn run(&self) {
        info!("AgentCoordinator 启动，等待外部输入...");
        // 按 channel 绑定三元组初始化会话集合（agent_id 取 config，保留 agent = "0"）
```

**3m. 三处记忆写入身份改取 key.agent_id**：

`incoming_message` 中：

```rust
        let key = self.session_key_for(&ch);
        let role_name = memory_role(&key.role_name, &key.mode);
        let agent_id = Arc::new(key.agent_id.clone());
```

`send_admin_reply` 中（`let key = self.session_key_for(&ch);` 之后）：

```rust
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = Arc::new(key.agent_id.clone());
```

`send_outgoing` 中（同样模式）：

```rust
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = Arc::new(key.agent_id.clone());
```

**3n. `resolve_out_channel` 匹配字段**：

```rust
            if c.agent_id == ch.agent_id && c.role_name == ch.role_name {
```

**3o. `resolve_out_channel_for_session` 匹配字段**：

```rust
            if c.agent_id.as_str() == session.agent_id.as_str()
                && c.role_name.as_str() == session.role_name.as_str()
```

**3p. `enqueue_batch` 与 `tools_for_session` 的 context_config 参数**：

`enqueue_batch` 中 `session.agent_name.as_str()` → `session.agent_id.as_str()`；`tools_for_session` 中同样替换（`execute_tool_call` 上一提交已改为 `session.agent_id`，无需再改）。

**3q. 新增 `verify_agent_exists`（公共方法）+ 自由函数**（放在 `channel_mode` 之后、`ensure_session` 之前）：

```rust
    /// 校验 agent_id 存在（/agent 切换前调用）：保留 id "0" 直接通过，其余委托 verify_agent_exists_http
    pub async fn verify_agent_exists(&self, agent_id: &str) -> Result<()> {
        let ego_url = kissbot_api::ApiConfig::get().memory_ego_url.clone();
        verify_agent_exists_http(agent_id, &ego_url).await
    }
```

自由函数放在文件底部 `resolve_agent_id_http` 原位置（删除后者后）：

```rust
/// verify_agent_exists 的纯函数实现（便于单测）：空 agent_id 或 "0"（保留）直接 Ok；
/// ego 未配置/HTTP 失败/data 为 null 返回 Err（调用方保持原 agent 不变）
async fn verify_agent_exists_http(agent_id: &str, ego_url: &str) -> Result<()> {
    if agent_id.is_empty() || agent_id == RESERVED_AGENT_ID {
        return Ok(());
    }
    if ego_url.is_empty() {
        return Err(Error::MemoryEgoError("ego 未配置（memory_ego_url 为空）".to_string()));
    }
    let client = reqwest::Client::new();
    let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
    let resp = client.post(format!("{}/agent/get", ego_url))
        .header(kissbot_security::HEADER_API_KEY, api_key.as_str())
        .json(&serde_json::json!({ "agent_id": agent_id }))
        .send()
        .await
        .map_err(|e| Error::MemoryEgoError(format!("agent/get 请求失败: {}", e)))?;
    let data: serde_json::Value = resp.json().await
        .map_err(|e| Error::MemoryEgoError(format!("agent/get 响应解析失败: {}", e)))?;
    if data["data"].is_null() {
        Err(Error::MemoryEgoError(format!("agent 不存在: {}", agent_id)))
    } else {
        Ok(())
    }
}
```

**3r. 删除 coordinator 测试**（5 个函数整段删除）：`session_key_of_always_builds_key`、`resolve_agent_id_http_empty_returns_reserved`、`resolve_agent_id_http_ego_unconfigured_errors`、`resolve_agent_id_http_unreachable_errors`、`think_write_condition_any_non_empty`（引用已删除的 `should_write_think`，当前编译错误）。

- [ ] **Step 4: 陈旧注释同步**（coordinator.rs 与 session_manager.rs）

coordinator.rs `new()` 中（启动动作已移入 `run()`，注释一并清理）：

```rust
        // 启动动作（绑定运行态 agent / 初始化会话 / 连接 channel）统一在 run() 中执行
```

`ensure_session` doc 注释改为：

```rust
    /// 定位会话，新建时构建初始上下文；返回 (会话, 是否新建)
    /// channel_id 为触发会话创建/重置的来源 channel；新建会话的 agent_id 取自 key（config 绑定）
```

`relocate_channel` 注释改为：

```rust
        // 2. 新三元组对应会话不存在则创建并构建初始上下文（agent 标识取会话 key）
```

session_manager.rs 的 `SessionManager` doc 注释（约 577 行）改为：

```rust
/// 会话管理器：汇总所有绑定 channel 的 (agent_id, role_name, mode) 去重维护会话集合
/// （session_key 仅用于去重；agent_id 直接取 key 的 agent_id 字段）
```

- [ ] **Step 5: command_router.rs 命令改造**

**4a. import 更新**：

```rust
use crate::types::{AdminCommand, Error, Mode, OutChannelParams, RESERVED_AGENT_ID, Result, SessionKey};
use crate::config_manager::{ConfigManager, OutChannelConfig, ProviderModel};
use kissbot_api::ChannelUser;
use crate::coordinator::{AgentCoordinator, RESERVED_ROLE_NAME};
```

（原 `use crate::coordinator::{AgentCoordinator, RESERVED_AGENT_NAME, RESERVED_ROLE_NAME};` 删除 `RESERVED_AGENT_NAME`。）

**4b. `channel_current_key` fallback**：

```rust
async fn channel_current_key(channel_id: &str) -> SessionKey {
    AgentCoordinator::instance().channel_session_key(channel_id).await
        .unwrap_or_else(|| SessionKey { agent_id: RESERVED_AGENT_ID.to_string(), role_name: String::new(), mode: Mode::Role })
}
```

**4c. `/agent` 解析**：

```rust
            "agent" => {
                // /agent [agent_id] [role]：缺省 agent_id 用保留 agent（"0"），缺省 role 用保留 role（空）
                let agent_id = parts.get(1).map(|s| s.to_string());
                let role = parts.get(2).map(|s| s.to_string());
                Ok(AdminCommand::SetAgent { agent_id, role })
            }
```

**4d. `SetAgent` 执行**：

```rust
            AdminCommand::SetAgent { agent_id, role } => {
                let new_agent_id = agent_id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| RESERVED_AGENT_ID.to_string());
                let new_role = role.clone().unwrap_or_else(|| RESERVED_ROLE_NAME.to_string());
                // 切换前先校验新 agent 存在：失败则保持原有 agent 不变（只读 API，队列外，避免阻塞变更队列）
                coordinator.verify_agent_exists(&new_agent_id).await?;
                // 构造新会话三元组（mode 保持当前运行态），统一走串行队列应用（防写-写竞态）
                let cur = channel_current_key(channel_id).await;
                let new_key = SessionKey { agent_id: new_agent_id.clone(), role_name: new_role.clone(), mode: cur.mode };
                coordinator.change_channel_key(channel_id, new_key).await?;
                Ok(format!("✅ 已设置 agent: {} / role: {}", new_agent_id, new_role))
            }
```

**4e. 其余命令构造 SessionKey 处**（`SetRole` / `ModeEvent` / `ModeRole` / `Reenter` 共 4 处）：`cur.agent_name` → `cur.agent_id`。

**4f. `BindOutgoing` 唯一性匹配**：

```rust
                            if cid != channel_id && c.agent_id == src.agent_id && c.role_name == src.role_name {
```

- [ ] **Step 6: http_server.rs 测试 JSON 字段**

测试用例中（`POST /config/channels 添加 + admins`）：

```rust
                "agent_id": "0", "role_name": "",
```

- [ ] **Step 7: 编译与全量测试恢复**

```bash
cd /home/admin/project/kissbot/kissbot-agent
cargo check 2>&1 | tail -20
```

预期：无 error（warning 可接受）。

```bash
cargo test 2>&1 | tail -30
```

预期：全部通过（`kissbot-agent` 的现有测试 + Task 1 内更新的测试）。

- [ ] **Step 8: Commit**

```bash
cd /home/admin/project/kissbot
git add -A
git commit -m "refactor(agent): agent 标识统一改用 agent_id，删除 agent_name→agent_id 运行时绑定链

- types：RESERVED_AGENT_ID（\"0\"）从 coordinator 迁入 types（config_manager serde 归一化共用）
- config：ChannelConfig.agent_name 改名 agent_id，serde 缺省/空串自动归一化为 \"0\"；context 配置 key 语义与 context_config 签名改按 agent_id
- coordinator：删除 session_key_of、channel_agent/bind_channel_runtime/set_channel_runtime/resolve_agent_id_for_bind/resolve_agent_id_http 绑定链及 RESERVED_AGENT_NAME；session_key_for 直接构造 SessionKey（config agent_id + 运行态 mode）；channel_session_key 复用；记忆写入身份改取 key.agent_id；ConfigChange::ApplyKey/change_channel_key 去掉 agent_id 参数；新增 verify_agent_exists（ego /agent/get 校验，保留 id 直过）
- command_router：/agent 直接接受 agent_id（缺省/空 → \"0\"），切换前 verify_agent_exists 校验；channel_current_key 回退与各命令构造用 agent_id
- http_server：测试 JSON 字段 agent_name → agent_id"
```

---

### Task 2: verify_agent_exists 测试补全（含本地 mock）与全量验证

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`（测试区）

**Interfaces:**
- Consumes: `verify_agent_exists_http(agent_id: &str, ego_url: &str) -> Result<()>`（Task 1 产出；自由函数带 ego_url 参数，便于测试注入地址）
- Produces: 无新接口

- [ ] **Step 1: 写失败测试（3 个用例，替换已删除的 resolve_agent_id_http 测试位置）**

在 `coordinator.rs` 测试模块内新增：

```rust
    #[tokio::test]
    async fn verify_agent_exists_http_reserved_or_empty_passes() {
        // 保留 id "0" 与空串直接 Ok，无需 ego
        assert!(verify_agent_exists_http("0", "http://127.0.0.1:1").await.is_ok());
        assert!(verify_agent_exists_http("", "http://127.0.0.1:1").await.is_ok());
    }

    #[tokio::test]
    async fn verify_agent_exists_http_ego_unconfigured_errors() {
        // ego_url 为空 -> Err
        assert!(verify_agent_exists_http("alice", "").await.is_err());
    }

    #[tokio::test]
    async fn verify_agent_exists_http_unreachable_errors() {
        // ego_url 指向不可达端口 -> 连接失败 Err
        assert!(verify_agent_exists_http("carol", "http://127.0.0.1:1").await.is_err());
    }
```

运行：`cargo test verify_agent_exists 2>&1 | tail -10`
预期：PASS（实现 `verify_agent_exists_http` 已在 Task 1 产出；此步为回归覆盖）。

- [ ] **Step 2: 写本地 mock 成功路径测试（先失败）**

在测试模块内新增（`kissbot-agent` 已依赖 axum / tokio，用于起本地 mock 服务）：

```rust
    /// 起本地 axum mock：/agent/get 返回 data 为给定值
    async fn mock_ego(data: Option<serde_json::Value>) -> String {
        use axum::{routing::post, Router};
        let app = Router::new().route("/agent/get", post(move |_| async move {
            axum::Json(serde_json::json!({ "success": true, "data": data }))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn verify_agent_exists_http_found_passes() {
        let url = mock_ego(Some(serde_json::json!({ "name": "alice" }))).await;
        assert!(verify_agent_exists_http("alice", &url).await.is_ok(), "data 非 null 应 Ok");
    }

    #[tokio::test]
    async fn verify_agent_exists_http_not_found_errors() {
        let url = mock_ego(None).await;
        assert!(verify_agent_exists_http("nobody", &url).await.is_err(), "data 为 null 应 Err");
    }
```

运行：`cargo test verify_agent_exists_http_found_passes 2>&1 | tail -10`
预期：FAIL（mock_ego 或断言行为不满足，视实现而定——若 Task 1 的 `data["data"].is_null()` 判断与 mock 形状不一致则失败）。

- [ ] **Step 3: 验证通过**

运行：`cargo test verify_agent_exists 2>&1 | tail -10`
预期：PASS（5 个用例：reserved/empty、ego 未配置、不可达、found、not_found）。

若 Step 2 失败，唯一可能的原因为 mock 的 `data` 形状与实现的 `data["data"].is_null()` 判定不一致（例如 reqwest 响应体解析路径与 axum Json 序列化形状差异）——按 mock 返回 `{ "success": true, "data": null|对象 }` 对齐后重跑，不要改动业务判定逻辑。

- [ ] **Step 4: 全量测试**

```bash
cargo test 2>&1 | tail -30
```

预期：`kissbot-agent` 全部测试通过。

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot
git add -A
git commit -m "test(agent): verify_agent_exists 测试补全（保留 id 直过/ego 未配置/不可达 + 本地 mock 成功与不存在路径）"
```
