# 会话创建初始化归位 + MemoryEgoClient 提取设计

> **日期:** 2026-08-14
> **状态:** 已确认（brainstorming 完成）

## 背景与动机

`Coordinator::ensure_session` 当前承担双重职责：① 取 `valid_default` 模型并调 `SessionManager::get_or_create`；② 新建会话（created 分支）时的初始化——Event/Role 上下文恢复重建 + 系统消息设置。本次重构：

- 新建 `MemoryEgoClient`（与 `MemoryStoreClient` 同模式），作为 `AgentCoordinator` 成员，**解决 ego REST client 每次调用 `reqwest::Client::new()` 导致的连接池不复用问题**，同时让 ego 调用有明确归属。
- 初始化逻辑（if created 块）整体搬入 `SessionManager::create_session`（`get_or_create` 的创建分支），`get_or_create` 直接返回 `Arc<Session>`（去掉"是否新建"bool）。
- `get_or_create` / `create_session` 的 model 参数改为 `Arc<Option<ProviderModel>>`，配合 `ArcSwap::from(arc)` 实现**全链路零深拷贝**。

## 决策记录

### D1: 新建 MemoryEgoClient（coordinator 成员）

`kissbot-agent/src/memory_ego_client.rs` 新文件（main.rs 加 `mod memory_ego_client;`）：

```rust
/// ego 服务 REST 客户端（与 MemoryStoreClient 同模式）：共享 reqwest client（连接池复用）
/// + ego base_url + api_key；方法封装 /agent、/individual、/role 接口
pub struct MemoryEgoClient {
    client: reqwest::Client,
    base_url: String,   // 构造时从 ApiConfig::get().memory_ego_url 读取
    api_key: String,    // 构造时从 SecurityConfig::get().api_key 读取
}

impl MemoryEgoClient {
    pub fn new() -> Self { ... }
    /// POST /agent/get：data null（agent 不存在）→ Ok(None)；网络/解析失败 → Err
    pub async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentMetadata>>
    /// POST /individual/get-all：失败 → Err
    pub async fn get_individuals(&self, agent_id: &str) -> Result<Option<IndividualRecognition>>
    /// POST /role/get：失败 → Err
    pub async fn get_role(&self, agent_id: &str, role_name: &str) -> Result<Option<RolePlay>>
    /// agent 是否存在（data 非 null；base_url 空 → Err("ego 未配置")）
    pub async fn agent_exists(&self, agent_id: &str) -> Result<bool>
}
```

- 职责边界：**只做 REST 封装**（client 复用 + 单例配置读取）；markdown 拼装（ego_md）留在 coordinator。
- `AgentCoordinator` 加字段 `memory_ego_client: Arc<MemoryEgoClient>`，`new()` 构造。
- 错误类型：crate::types::Result / Error::MemoryEgoError。

### D2: load_ego_info 内联进 system_prompt_for_agent

`load_ego_info` 全仓唯一调用方是系统消息段（ensure_session 189 行），改后无独立存在价值 → **内联**。新增 Coordinator 方法：

```rust
/// 根据 agent_id 获取系统提示词（新建会话系统消息，create_session 内调用）：
/// 保留 agent（agent_id="0"）用 NexusRepo 默认系统提示词；其余走 ego REST（agent 元数据 + 个体识别 + 角色设定，
/// 失败静默跳过，全部失败回退默认提示词"你是 kissbot 智能助手"）
pub async fn system_prompt_for_agent(&self, agent_id: &str, role_name: &str) -> Result<String>
```

内部用 `self.memory_ego_client.get_agent / get_individuals / get_role`；IDs 收集（channel 绑定身份）、individual_names 过滤、ego_md 拼装逻辑原样保留，容错语义不变。

### D3: verify_agent_exists 改用 memory_ego_client

`verify_agent_exists`（关联函数）改为经 `AgentCoordinator::get().memory_ego_client.agent_exists(...)`：

```rust
pub async fn verify_agent_exists(agent_id: &str) -> Result<()> {
    if agent_id.is_empty() || agent_id == RESERVED_AGENT_ID {
        return Ok(());
    }
    if AgentCoordinator::get().memory_ego_client.agent_exists(agent_id).await? {
        Ok(())
    } else {
        Err(Error::MemoryEgoError(format!("agent 不存在: {}", agent_id)))
    }
}
```

- ego_url 空检查（原 `if ego_url.is_empty() → Err("ego 未配置")`）挪进 `agent_exists` 内部。
- 空 id / 保留 id 直通逻辑保留（不触单例，coordinator 测试 `verify_agent_exists_reserved_or_empty_passes` 不受影响）。

### D4: ensure_session 保留薄封装（不内联）

4 个调用点（apply_channel_key / set_session_model / run / handle_incoming_message）都需要"取 model + get_or_create"，**不唯一 → 不内联**。保留封装，去掉"是否新建"bool：

```rust
/// 定位会话（不存在则创建；创建时上下文恢复/重建 + 系统消息在 get_or_create 内部完成）；返回会话（无"是否新建"标记）
async fn ensure_session(&self, key: &SessionKey) -> Arc<Session> {
    let model = self.valid_default.load_full();   // Arc<Option<ProviderModel>>，O(1)
    self.session_manager.get_or_create(key, model).await
}
```

4 个调用点仅去掉 channel_id 参数与 tuple 解构（`(session, _)` → `session`）。

### D5: get_or_create / create_session async + model 参数 Arc 化 + 初始化搬入

```rust
pub async fn get_or_create(&self, key: &SessionKey, model: Arc<Option<ProviderModel>>) -> Arc<Session> {
    if let Some(s) = self.sessions.get(key) { return s.clone(); }
    match self.sessions.entry(key.clone()) {
        dashmap::mapref::entry::Entry::Occupied(e) => e.get().clone(),
        dashmap::mapref::entry::Entry::Vacant(e) => {
            let session = Self::create_session(key, model, &self.data_dir).await;
            e.insert(session.clone());
            session
        }
    }
}

async fn create_session(key: &SessionKey, model: Arc<Option<ProviderModel>>, data_dir: &str) -> Arc<Session> {
    // 1-4. 依赖序组装（不变）；Session.model 改 ArcSwap::from(model)（Arc 转移，零深拷贝）
    // 5. 新建会话初始化（原 ensure_session 的 created 分支整体搬入，spawn 前执行）：
    match session.mode.as_ref() {
        Mode::Event(_) => {
            let _ = session.context.lock().await.recover_from_cache().await;
        }
        Mode::Role => {
            let messages = AgentCoordinator::get()
                .build_context_from_memory_store(session.agent_id.clone(), session.role_name.clone()).await;
            let _ = session.context.lock().await.archive_and_clear_cache_and_reset_messages(Some(messages)).await;
        }
    }
    if let Ok(prompt) = AgentCoordinator::get().system_prompt_for_agent(session.agent_id.as_str(), &session.role_name).await {
        session.context.lock().await.set_system_message(prompt);
    }
    // 6. spawn consumer（原第 5 步后移）
    tokio::spawn(consumer.run());
    session
}
```

- **零深拷贝链路**：`valid_default.load_full()`（Arc，O(1)）→ 参数 `Arc<Option<ProviderModel>>` → `ArcSwap::from(model)`（O(1)，arc-swap 1.9 `impl From<Arc<T>> for ArcSwapAny<T>`）。
- `Session.model` 字段类型保持 `ArcSwap<Option<ProviderModel>>` 不变（消费方零改动）。
- 初始化移 spawn 前：consumer 任务启动前上下文已就绪，比原顺序（spawn 后才初始化）更安全，无行为差异。
- session_manager → coordinator 单例依赖**已有先例**（run_agentic_loop 溢出重置 334 行已调 `coordinator.build_context_from_memory_store`）。

### D6: Session 字段可见性收窄

改后 coordinator 对 Session 字段的直接访问情况：

- **可去 pub（改后零跨模块访问，收窄为模块私有）**：
  - `context`：coordinator 唯一使用在 ensure_session（175-190 行），初始化搬入 create_session 后零访问
  - `batch_producer`：coordinator 经 `session.enqueue_batch(...)`（Session 的 pub(crate) 方法）调用，无字段直接访问
  - `notify`：全仓无 coordinator 使用
- **必须保留 pub（coordinator 仍直接访问）**：`agent_id` / `role_name` / `mode`（多处读取）、`model`（set_session_model 的 `session.model.store(...)`）

session_manager 内部 tests mod 是子模块（`use super::*`），私有字段仍可访问（test_pair 构造、spawn_trigger_exits_on_notify 用 notify），不受影响。

**字段顺序调整**：`model` 移到 `context` 之前（生命周期相近字段相邻：model 与 context 同为会话核心状态），新顺序：`agent_id` / `role_name` / `mode` / `model` / `context` / `batch_producer` / `notify`；`create_session` 与 tests 的 Session 字面量构造同步调整。

### D7: 测试适配

session_manager tests：
- `get_or_create_dedupes` / `get_or_create_with_none_model`：去 created 断言 + `.await` + model 参数 `Arc::new(Some(model.clone()))` / `Arc::new(None)`。
- 新增 `ensure_test_globals`（幂等）：保活 tempdir（`OnceLock<TempDir>`，防单例 data_dir 失效）+ set_var KISSBOT_CONFIG + `ConfigManager::new()` + `AgentCoordinator::new()`——因为 create_session 初始化触发 `AgentCoordinator::get()` / `ConfigManager::get()` 单例。AgentCoordinator::new() 的 verify_model HTTP 失败仅 warn，stations 空，安全。

coordinator / http_server tests 不受影响（不触发该路径；http_server 测试已 init ConfigManager 单例，其 set_var 竞态为既有债务）。

## 不做的事（YAGNI）

- 不改 `Session.model` 字段类型（保持 ArcSwap，消费方零改动）
- 不做 ego client 的独立单例模块（MemoryStoreClient 同模式：coordinator 成员即可）
- 不改 build_context_from_memory_store 位置（仍为 Coordinator 方法，session_manager 经单例调用）

## 验证标准

1. `cargo check` 无警告
2. `cargo test` 全绿（121 + 适配后不变）
3. `rg "reqwest::Client::new"` 在 coordinator.rs 零残留（ego client 归属 MemoryEgoClient 单实例）
4. 测试多次运行无 flaky（set_var 竞态观察）
