# ConfigManager 全局单例化实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ConfigManager 改为进程级全局单例（`OnceLock` 存值，`new()` 完成时注册、`get()` 获取），各结构（AgentCoordinator / ChannelManager / ModelClient / HttpServer / command_router）去除 config 字段与参数；同时 `AgentCoordinator::instance()` 改名 `get()`，与 `ConfigManager::get()` / `ApiConfig::get()` 命名统一。

**Architecture:** 单例形态与 `AgentCoordinator` 完全一致：`static INSTANCE: OnceLock<ConfigManager>` + `new() -> Result<()>` 末尾 `INSTANCE.set` + `get() -> &'static Self`（expect panic）。各模块不再持 `Arc<ConfigManager>` 字段/参数，需要时 `ConfigManager::get()`。内部可变状态（`RwLock<NexusRepo>`、写方法、listeners）不动。任务按依赖序切分，前几个任务保留 `new()` 返回 `Result<Self>` 保证中间态可编译，最后一个任务收敛返回类型。

**Tech Stack:** Rust（std `OnceLock`，tokio），kissbot-agent crate。

## Global Constraints

- 改造范围仅 `kissbot-agent` crate（`kissbot-agent/src/` 下文件）
- 不要删除代码中的注释（项目 CLAUDE.md 规则）；被删代码自带注释随删除，但保留注释的说明价值
- 单例命名：两个 struct 的访问函数都叫 `get()`（`AgentCoordinator::get()` / `ConfigManager::get()`）
- `ConfigManager::new()` 最终签名 `pub async fn new() -> Result<()>`；`ConfigManager::get() -> &'static ConfigManager`（未初始化 panic）
- 每个任务结束必须 `cargo check` 通过、`cargo test` 全绿、git commit（中文 comment，包含该任务全部改动）
- 工作目录：`kissbot-agent`（cargo 命令在 `/home/admin/project/kissbot/kissbot-agent` 下执行）

---

### Task 1: ConfigManager 加单例入口（new 内部注册，仍返回 Self）

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`

**Interfaces:**
- Consumes: 无
- Produces: `ConfigManager::get() -> &'static ConfigManager`；`ConfigManager::new()` 仍为 `pub async fn new() -> Result<Self>`（内部注册单例，过渡期返回实例保持现有调用方编译）

- [ ] **Step 1: 确认 import 并加单例骨架**

`config_manager.rs` 顶部 import 确认含 `std::sync::OnceLock`（当前为 `use std::sync::{Arc, RwLock};` 之类，缺则补）。在 `pub struct ConfigManager { ... }` 定义之前加：

```rust
/// ConfigManager 全局单例（进程内唯一；new() 完成时注册，此后 get() 可用）。
/// 与 AgentCoordinator 同模式：任何模块读配置直接 ConfigManager::get()，不传参、不持引用。
static INSTANCE: OnceLock<ConfigManager> = OnceLock::new();
```

- [ ] **Step 2: new() 尾部注册单例**

`new()`（约 338 行起）末尾 `Ok(Self { ... })` 改为构造 manager 后注册再返回（仍返回实例）：

```rust
        let manager = Self {
            agent_config,
            nexus_repo: Arc::new(RwLock::new(nexus_repo)),
            station_repo: Arc::new(RwLock::new(station_repo)),
            nexus_path,
            station_path,
            listeners: DashMap::new(),
        };
        // 注册全局单例（此后 get() 可用；重复调用幂等，第二次 set 被忽略，与 AgentCoordinator 一致）
        let _ = INSTANCE.set(manager);
        Ok(manager)
```

- [ ] **Step 3: 新增 get()**

在 `impl ConfigManager` 内、`new()` 之前加：

```rust
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn get() -> &'static ConfigManager {
        INSTANCE.get().expect("ConfigManager 未初始化")
    }
```

- [ ] **Step 4: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: check 通过；全部测试通过（无新增测试，行为不变）

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/config_manager.rs
git commit -m "refactor(agent): ConfigManager 加全局单例入口（INSTANCE + get()，new() 末尾注册；与 AgentCoordinator 同模式，过渡期仍返回实例保证调用方编译）"
```

---

### Task 2: ModelClient 去 config_manager 字段

**Files:**
- Modify: `kissbot-agent/src/model_client.rs`
- Modify: `kissbot-agent/src/coordinator.rs`（仅 `ModelClient::new(...)` 调用处）

**Interfaces:**
- Consumes: Task 1 的 `ConfigManager::get()`
- Produces: `ModelClient::new() -> Self`（无参）；`ModelClient` 不再有 `config_manager` 字段

- [ ] **Step 1: 改 ModelClient**

`model_client.rs`：

```rust
pub struct ModelClient {
    client: Arc<reqwest::Client>,
}

impl ModelClient {
    pub fn new() -> Self {
        let client = Arc::new(reqwest::Client::new());
        Self { client }
    }
```

`call` 内 `let effective = self.config_manager.resolve_effective_config(pm).await` → `let effective = ConfigManager::get().resolve_effective_config(pm).await`（`use crate::config_manager::ConfigManager` 保留）。

`list_models` 内 `self.config_manager.provider_config_by_name(&pm.provider).await` → `ConfigManager::get().provider_config_by_name(&pm.provider).await`。

- [ ] **Step 2: 改 coordinator 调用处**

`coordinator.rs` `new()` 内 `let model_client = ModelClient::new(config.clone());` → `let model_client = ModelClient::new();`

- [ ] **Step 3: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: 通过（ModelClient 无独立测试，编译 + 回归即可）

- [ ] **Step 4: Commit**

```bash
git add kissbot-agent/src/model_client.rs kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): ModelClient 去 config_manager 字段，内部改 ConfigManager::get()，new() 无参"
```

---

### Task 3: ChannelManager 去 config 字段

**Files:**
- Modify: `kissbot-agent/src/channel_manager.rs`
- Modify: `kissbot-agent/src/coordinator.rs`（仅 `ChannelManager::new(...)` 调用处）

**Interfaces:**
- Consumes: Task 1 的 `ConfigManager::get()`
- Produces: `ChannelManager::new() -> Self`（无参）；`ChannelManager` 不再有 `config` 字段

- [ ] **Step 1: 删字段与改 new()**

`channel_manager.rs` 结构体删 `config: Arc<ConfigManager>` 字段（含其注释）；`pub fn new(config: Arc<ConfigManager>) -> Self` 改为：

```rust
    pub fn new() -> Self {
        Self {
            channels: DashMap::new(),
            disconnect_notify: DashMap::new(),
        }
    }
```

- [ ] **Step 2: connect_all 内三处 config 使用改单例**

`connect_all(self: Arc<Self>)` 内（约 162-194 行）：

- `let reconnect_secs = self.config.ws_reconnect_interval_secs();` → `let reconnect_secs = ConfigManager::get().ws_reconnect_interval_secs();`
- `for (_, ch) in self.config.channels().await {` → `for (_, ch) in ConfigManager::get().channels().await {`
- 删 `// 重连循环内实时读取绑定身份（/bind 回写后重连即生效），需持有 config 引用` 与 `let config = self.config.clone();` 两行；任务内 `let bind_users = config.channel(&channel_id).await` → `let bind_users = ConfigManager::get().channel(&channel_id).await`

- [ ] **Step 3: 改 coordinator 调用处**

`coordinator.rs` `new()` 内 `channel_manager: Arc::new(ChannelManager::new(config.clone())),` → `channel_manager: Arc::new(ChannelManager::new()),`

- [ ] **Step 4: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: 通过（channel_manager 测试只测 Channel 内部，不依赖 config）

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/channel_manager.rs kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): ChannelManager 去 config 字段，new() 无参，connect_all 与重连循环改 ConfigManager::get()"
```

---

### Task 4: Coordinator 删 config 字段 + new() 无参 + instance() 全局改名 get()

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`
- Modify: `kissbot-agent/src/command_router.rs`（仅 instance()→get() 改名）
- Modify: `kissbot-agent/src/channel_manager.rs`（仅 243 行 instance()→get() 改名）
- Modify: `kissbot-agent/src/session_manager.rs`（仅 279 行 instance()→get() 改名）
- Modify: `kissbot-agent/src/main.rs`（仅 `AgentCoordinator::new(config.clone())` 调用）

**Interfaces:**
- Consumes: Task 2/3 的 `ModelClient::new()` / `ChannelManager::new()`（无参）；Task 1 的 `ConfigManager::get()`
- Produces: `AgentCoordinator::new() -> Result<()>`（无参）；`AgentCoordinator::get() -> &'static AgentCoordinator`（替代 instance()）；`AgentCoordinator` 不再有 `config` 字段

- [ ] **Step 1: instance() → get() 全局改名**

`coordinator.rs`：
- `pub fn instance() -> &'static AgentCoordinator {` → `pub fn get() -> &'static AgentCoordinator {`（注释同步：`/// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）`）
- 单例注释（37 行 `此后 instance() 可用`）与注册注释（`此后 instance() 可用；...`）、spawn 注释（`任务内 instance() 必然就绪`）中的 `instance()` 改 `get()`
- spawn 消费者任务内 `let coordinator = AgentCoordinator::instance();` → `AgentCoordinator::get();`

其他文件同名替换：
- `channel_manager.rs:243` `AgentCoordinator::instance().incoming_message(...)` → `AgentCoordinator::get().incoming_message(...)`
- `command_router.rs:11` `AgentCoordinator::instance().channel_session_key(...)` → `AgentCoordinator::get().channel_session_key(...)`
- `command_router.rs:176` `let coordinator = AgentCoordinator::instance();` → `let coordinator = AgentCoordinator::get();`
- `session_manager.rs:279` `let coordinator = AgentCoordinator::instance();` → `let coordinator = AgentCoordinator::get();`

- [ ] **Step 2: 删 config 字段，new() 无参**

`coordinator.rs` 结构体删 `pub config: Arc<ConfigManager>,`（含注释行）；`new()` 签名与开头改为：

```rust
    pub async fn new() -> Result<()> {
        let config = ConfigManager::get();
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let data_dir = config.data_dir().to_string();
        let session_manager = SessionManager::new(&data_dir);
        let model_client = ModelClient::new();
        // agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<ConfigChange>();

        let coordinator = Self {
            memory_store_client,
            session_manager,
            model_client: Arc::new(model_client),
            channel_manager: Arc::new(ChannelManager::new()),
            valid_default: ArcSwap::from_pointee(None),
            command_tx,
            station_runtimes: Arc::new(DashMap::new()),
        };
```

（`config` 局部变量为 `&'static ConfigManager`，后续 `config.default_model()` / `config.stations()` 不变。）

- [ ] **Step 3: 全部 `self.config` → `ConfigManager::get()`**

`coordinator.rs` 中剩余所有 `self.config` 出现处（约 18 处：`ensure_session`/`build_context_from_memory_store`/`prune_sessions`/`load_ego_info`/`channel_session_key`/`apply_channel_key`/`set_session_model`/`handle_incoming`/`enqueue_batch`/`send_admin_reply`/`resolve_out_channel`/`tools_for_session`/`execute_tool_call`/`send_outgoing` 等）逐一替换为 `ConfigManager::get()`。例：

```rust
        let prompt = self.config.default_system_prompt().await;
```
→
```rust
        let prompt = ConfigManager::get().default_system_prompt().await;
```

注意两处 command_router 调用当前传 `&self.config`，改为传 `ConfigManager::get()`（参数删除在 Task 5）：
- `CommandRouter::check_admin(&self.config, channel_id, &messenger_id, &user_id)` → `CommandRouter::check_admin(ConfigManager::get(), channel_id, &messenger_id, &user_id)`
- `CommandRouter::execute(&cmd, &self.config, channel_id)` → `CommandRouter::execute(&cmd, ConfigManager::get(), channel_id)`

`main.rs`：`coordinator::AgentCoordinator::new(config.clone())` → `coordinator::AgentCoordinator::new()`（config 变量仍被后续 `HttpServer::new(mgr_config)` 使用，保留）。

- [ ] **Step 4: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: 通过；`rg "instance\(\)" kissbot-agent/src` 无结果

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/coordinator.rs kissbot-agent/src/command_router.rs kissbot-agent/src/channel_manager.rs kissbot-agent/src/session_manager.rs kissbot-agent/src/main.rs
git commit -m "refactor(agent): AgentCoordinator::instance 改名 get（与 ConfigManager/ApiConfig 统一）；Coordinator 删 config 字段、new() 无参，内部改 ConfigManager::get()"
```

---

### Task 5: command_router 删 config 参数 + HttpServer 无参化

**Files:**
- Modify: `kissbot-agent/src/command_router.rs`
- Modify: `kissbot-agent/src/coordinator.rs`（两处 CommandRouter 调用）
- Modify: `kissbot-agent/src/http_server.rs`
- Modify: `kissbot-agent/src/main.rs`（HttpServer::new 调用）

**Interfaces:**
- Consumes: Task 4 的 `AgentCoordinator::get()`；Task 1 的 `ConfigManager::get()`
- Produces: `CommandRouter::check_admin(channel_id: &str, messenger_id: &str, user_id: &str) -> bool`（无 config 参数）；`CommandRouter::execute(command: &AdminCommand, channel_id: &str) -> Result<String>`（无 config 参数）；`HttpServer::new() -> Self`（无参）；`AppState { config: &'static ConfigManager }`

- [ ] **Step 1: command_router 删 config 参数**

`check_admin`：

```rust
    /// 检查发送者是否为该来源 channel 的管理权限用户（per-channel，避免跨 channel 提权）
    pub async fn check_admin(
        channel_id: &str,
        messenger_id: &str,
        user_id: &str,
    ) -> bool {
        let admins = ConfigManager::get().channel_admins(channel_id).await;
        admins.iter().any(|a| a.messenger_id == messenger_id && a.user_id == user_id)
    }
```

`execute` 签名 `pub async fn execute(command: &AdminCommand, config: &ConfigManager, channel_id: &str)` → `pub async fn execute(command: &AdminCommand, channel_id: &str)`，函数体内所有 `config.xxx` → `ConfigManager::get().xxx`（含 `update_channel`/`add_admin`/`remove_admin`/`channels`/`set_default_model`/`channel` 等；`let coordinator = AgentCoordinator::get();` 保留）。

- [ ] **Step 2: coordinator 调用点同步**

`coordinator.rs`：
- `CommandRouter::check_admin(ConfigManager::get(), channel_id, &messenger_id, &user_id)` → `CommandRouter::check_admin(channel_id, &messenger_id, &user_id)`
- `CommandRouter::execute(&cmd, ConfigManager::get(), channel_id)` → `CommandRouter::execute(&cmd, channel_id)`

- [ ] **Step 3: HttpServer 无参化**

`http_server.rs`：

```rust
impl HttpServer {
    /// 从 kissbot_security 全局配置读取 admin_api_key（与 channel-web / memory-ego 一致）；
    /// 监听地址取 ConfigManager 单例的 mgmt_host / mgmt_port
    pub fn new() -> Self {
        let admin_api_key = kissbot_security::SecurityConfig::get().admin_api_key.to_string();
        Self { admin_api_key }
    }
```

结构体删 `config: Arc<ConfigManager>,` 字段；`build_router` 内 `let config = self.config.clone();` → `let config = ConfigManager::get();`；`AppState` 改：

```rust
#[derive(Clone)]
struct AppState {
    config: &'static ConfigManager,
}
```

`start()` 内 `self.config.mgmt_host(), self.config.mgmt_port()` → `ConfigManager::get().mgmt_host(), ConfigManager::get().mgmt_port()`。

- [ ] **Step 4: http_server 测试调整**

`http_server.rs` 测试（约 188-230 行）：

```rust
    // ConfigManager 字段私有，无法在模块外直接构造；
    // 通过 ConfigManager::new() 加载临时 KISSBOT_CONFIG 构造（data_dir 指向 tempdir，完成即注册单例）。
    // 注意：kissbot-config 是进程级 OnceLock 单例，本测试会触发它；ConfigManager 单例在测试进程内注册一次，
    // 之后 get() 取同一实例。
    async fn test_manager(dir: &tempfile::TempDir) {
        let data_dir = dir.path().join("data");
        let cfg_path = dir.path().join("config.json");
        let cfg_json = format!(
            r#"{{"security":{{"api_key":"user-key-456","admin_api_key":"admin-key-123"}},"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9090,"ws_reconnect_interval_secs":5}}}}"#,
            data_dir.to_str().unwrap()
        );
        std::fs::write(&cfg_path, cfg_json).unwrap();
        // 2024 edition：设置环境变量需要 unsafe
        unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };
        ConfigManager::new().await.unwrap();
    }
```

测试主体 `let manager = test_manager(&dir).await;` 后的 `manager` 用法改为 `ConfigManager::get()`（测试内构造 HttpServer 的调用改 `HttpServer::new()`；落盘验证取 data_dir 改 `ConfigManager::get().data_dir()`）。

- [ ] **Step 5: main.rs 收尾**

`main.rs`：删 `let mgr_config = config.clone();` 行；`HttpServer::new(mgr_config)` → `HttpServer::new()`。`let config = Arc::new(ConfigManager::new().await.expect("初始化配置失败"));` 改为 `ConfigManager::new().await.expect("初始化配置失败");`（丢弃返回值；Task 6 改签名后此行不变）。

- [ ] **Step 6: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: 通过（http_server 测试配置端点到 tempdir 落盘验证）

- [ ] **Step 7: Commit**

```bash
git add kissbot-agent/src/command_router.rs kissbot-agent/src/coordinator.rs kissbot-agent/src/http_server.rs kissbot-agent/src/main.rs
git commit -m "refactor(agent): command_router 删 config 参数改 ConfigManager::get()；HttpServer 去 config 字段、new() 无参、AppState 存 &'static 单例，测试改走单例"
```

---

### Task 6: ConfigManager::new() 收敛为 Result<()>

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`

**Interfaces:**
- Consumes: Task 1-5 全部
- Produces: `ConfigManager::new() -> Result<()>`（最终签名，不返回实例）

- [ ] **Step 1: 改签名**

`config_manager.rs` `new()`：签名 `pub async fn new() -> Result<Self>` → `pub async fn new() -> Result<()>`；尾部 `Ok(manager)` → `Ok(())`。注释（`/// 加载配置并注册全局单例（进程内唯一；new() 完成时注册，此后 get() 可用）` 之类）同步。

- [ ] **Step 2: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: 通过（全项目无 `ConfigManager::new()` 返回值使用；main.rs 与 http_server 测试已在 Task 5 适配）

- [ ] **Step 3: Commit**

```bash
git add kissbot-agent/src/config_manager.rs
git commit -m "refactor(agent): ConfigManager::new() 收敛为 Result<()>（不返回实例，取数一律 get()）"
```

---

### Task 7: 全局扫尾验证

**Files:**
- Verify only（发现问题才改）

- [ ] **Step 1: 残留检查**

Run: `cd /home/admin/project/kissbot && rg "instance\(\)|Arc<ConfigManager>|config_manager" kissbot-agent/src --type rust`
Expected: 无 `instance()`（coordinator 相关）；`Arc<ConfigManager>` 仅出现在 config_manager.rs 内部字段（nexus_repo 等，非外部持有）；`config_manager` 仅模块路径引用（`crate::config_manager`）

- [ ] **Step 2: 全量验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test 2>&1 | tail -3`
Expected: check 通过；`test result: ok. 116 passed`（数量不变或为 116 个测试全绿）

- [ ] **Step 3: 提交（如有残留修复）**

```bash
git add -A
git commit -m "refactor(agent): ConfigManager 单例化扫尾（残留引用清理）"
```

---

## 自审

**1. Spec 覆盖：**
- 决策 1（单例模式 new 注册）→ Task 1/6 ✓
- 决策 2（改名 get）→ Task 4 ✓
- 决策 3（各结构去引用）→ Task 2/3/4/5 ✓
- 决策 4（装配顺序）→ Task 5 的 main.rs 收尾 + Task 6 ✓
- 决策 5（测试走单例）→ Task 5 Step 4 ✓
- 决策 6（内部状态不变）→ 各任务不动 RwLock/写方法/listeners ✓

**2. 占位符扫描：** 无 TBD/TODO；每个 Step 含具体代码或精确指令。

**3. 类型一致性：**
- `ConfigManager::get()` 返回 `&'static ConfigManager`，`coordinator::new()` 内局部 `config` 变量即 `&'static`，其方法调用不变 ✓
- Task 4 中 command_router 调用先传 `ConfigManager::get()`（参数尚在），Task 5 删参数——跨任务类型一致 ✓
- `AppState { config: &'static ConfigManager }`（Task 5）与 handler 内 `state.config.xxx()` 用法一致 ✓
- `HttpServer::new()` 无参（Task 5）与 main.rs 调用（Task 5 Step 5）同步 ✓
- main.rs 的 `ConfigManager::new().await.expect(...)` 在 Task 5（丢弃 Result<Self>）与 Task 6（Result<()>）均合法 ✓
