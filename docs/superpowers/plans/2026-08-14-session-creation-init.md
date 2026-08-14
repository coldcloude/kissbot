# 会话创建初始化归位 + MemoryEgoClient 提取实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ① 新建 `MemoryEgoClient`（与 MemoryStoreClient 同模式）作为 `AgentCoordinator` 成员，解决 ego REST client 每次 `reqwest::Client::new()` 连接池不复用问题；② 会话新建初始化（if created 块）搬入 `SessionManager::create_session`，`get_or_create` 直接返回 `Arc<Session>`（去"是否新建"bool）；③ model 参数 Arc 化实现全链路零深拷贝；④ Session 字段可见性收窄 + 顺序调整。

**Architecture:** `MemoryEgoClient` 只做 REST 封装（共享 client + 单例配置读取），markdown 拼装留 coordinator；`load_ego_info` 内联进 `system_prompt_for_agent`（全仓唯一调用方）；`ensure_session` 保留薄封装（4 调用点不唯一，不内联）；初始化依赖 `AgentCoordinator::get()` 单例（session_manager → coordinator 依赖已有先例，run_agentic_loop 334 行）。

**Tech Stack:** Rust（reqwest / arc-swap / dashmap / tokio），kissbot-agent crate。

## Global Constraints

- 改造范围：新建 `kissbot-agent/src/memory_ego_client.rs`（main.rs 加 mod）、`kissbot-agent/src/coordinator.rs`、`kissbot-agent/src/session_manager.rs`
- 不要删除代码中的注释（项目 CLAUDE.md 规则）；被改注释同步更新保持准确
- **零深拷贝链路**：`valid_default.load_full()`（Arc，O(1)）→ 参数 `Arc<Option<ProviderModel>>` → `ArcSwap::from(model)`（arc-swap 1.9 `impl From<Arc<T>> for ArcSwapAny<T>`，O(1) Arc 转移）；禁止回退 `from_pointee((**model).clone())`
- `Session.model` 字段类型保持 `ArcSwap<Option<ProviderModel>>` 不变
- **Session 字段新顺序**：`agent_id` / `role_name` / `mode` / `model` / `context` / `batch_producer` / `notify`；`model` 为最后一个 pub 字段，其后 `context` / `batch_producer` / `notify` 全部**去 pub**（收窄为模块私有；改后 coordinator 对三者零直接访问，tests mod 子模块仍可访问）
- `load_ego_info` 内联进 `system_prompt_for_agent`（无独立函数）；`ensure_session` 保留薄封装（4 调用点不唯一）
- `verify_agent_exists` 保持关联函数（内部经 `AgentCoordinator::get().memory_ego_client`），空 id/"0" 直通逻辑保留（不触单例），ego_url 空检查挪进 `agent_exists`
- 每任务结束：`cargo check` 无警告、`cargo test` 全绿（121 + 适配）、git commit（中文 comment）
- 工作目录：cargo 命令在 `/home/admin/project/kissbot/kissbot-agent`

---

### Task 1: MemoryEgoClient 新模块 + coordinator 改造

**Files:**
- Create: `kissbot-agent/src/memory_ego_client.rs`
- Modify: `kissbot-agent/src/main.rs`（mod 声明）
- Modify: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Produces: `MemoryEgoClient { client, base_url, api_key }` + 4 个 REST 方法；`AgentCoordinator.memory_ego_client: Arc<MemoryEgoClient>`；`AgentCoordinator::system_prompt_for_agent(&self, agent_id, role_name) -> Result<String>`（含内联的 load_ego_info 逻辑）；`ensure_session(&self, key) -> Arc<Session>`（薄封装）
- Consumes: `kissbot_api::ApiConfig::get().memory_ego_url`、`kissbot_security::SecurityConfig::get().api_key`、`kissbot_api::{AgentMetadata, IndividualRecognition, RolePlay, ApiResponse}`、`crate::ego_md::*`

- [ ] **Step 1: 新建 memory_ego_client.rs**

按 spec D1 实现（字段/方法签名、注释、错误类型 `crate::types::Result` / `Error::MemoryEgoError`）：

```rust
// kissbot-agent/src/memory_ego_client.rs
//! ego 服务 REST 客户端（与 memory_store_client 同模式）：共享 reqwest client + ego 配置单例读取

use kissbot_api::{AgentMetadata, IndividualRecognition, RolePlay};

/// ego 服务 REST 客户端：共享 reqwest client（连接池复用，替代原每次 Client::new()）+
/// ego base_url + api_key；方法封装 /agent、/individual、/role 接口
pub struct MemoryEgoClient {
    client: reqwest::Client,
    base_url: String,   // 构造时从 ApiConfig::get().memory_ego_url 读取
    api_key: String,    // 构造时从 SecurityConfig::get().api_key 读取
}

impl MemoryEgoClient {
    /// 从进程级单例读取 ego 配置构造（ApiConfig.memory_ego_url / SecurityConfig.api_key）
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: kissbot_api::ApiConfig::get().memory_ego_url.clone(),
            api_key: kissbot_security::SecurityConfig::get().api_key.clone(),
        }
    }

    /// POST /agent/get 查询 agent 元数据；data null（agent 不存在）→ Ok(None)；网络/解析失败 → Err
    pub async fn get_agent(&self, agent_id: &str) -> crate::types::Result<Option<AgentMetadata>> { ... }

    /// POST /individual/get-all 查询个体识别；网络/解析失败 → Err
    pub async fn get_individuals(&self, agent_id: &str) -> crate::types::Result<Option<IndividualRecognition>> { ... }

    /// POST /role/get 查询角色设定（role_name 空由调用方决定是否调用）；网络/解析失败 → Err
    pub async fn get_role(&self, agent_id: &str, role_name: &str) -> crate::types::Result<Option<RolePlay>> { ... }

    /// agent 是否存在（data 非 null）；base_url 空 → Err("ego 未配置（memory_ego_url 为空）")
    pub async fn agent_exists(&self, agent_id: &str) -> crate::types::Result<bool> { ... }
}
```

实现细节：
- `get_agent`：`client.post(format!("{}/agent/get", base_url)).header(HEADER_API_KEY, api_key).json({agent_id}).send().await` → 解析 `ApiResponse<AgentMetadata>` → `Ok(envelope.data)`（envelope.data 即 Option<AgentMetadata>）
- `get_individuals`：`POST /individual/get-all`，同理
- `get_role`：`POST /role/get`，body 含 agent_id + role_name
- `agent_exists`：base_url 空 → Err（保留原 verify_agent_exists 的空检查语义）；`POST /agent/get` → `data["data"].is_null()` 则 Ok(false)，否则 Ok(true)

（迁移自 coordinator.rs 的 load_ego_info / verify_agent_exists 中对应 REST 段；HEADER_API_KEY 用 `kissbot_security::HEADER_API_KEY`）

- [ ] **Step 2: main.rs 加 mod 声明**

`kissbot-agent/src/main.rs` mod 列表（memory_store_client 附近）加 `mod memory_ego_client;`

- [ ] **Step 3: coordinator 加成员 + new() 构造**

`AgentCoordinator` 结构体加字段（memory_store_client 附近）：
```rust
    /// ego 服务 REST 客户端（共享连接池；load_ego_info / verify_agent_exists 经它发请求）
    memory_ego_client: Arc<MemoryEgoClient>,
```
`new()` 中与 memory_store_client 并列构造：`let memory_ego_client = Arc::new(MemoryEgoClient::new());`，`Self { ... }` 加字段。

- [ ] **Step 4: load_ego_info 内联进 system_prompt_for_agent**

删除 `async fn load_ego_info(&self, ...)` 方法；新增（放 ensure_session 原位置附近）：

```rust
    /// 根据 agent_id 获取系统提示词（新建会话系统消息，create_session 内调用）：
    /// 保留 agent（agent_id="0"）用 NexusRepo 默认系统提示词；其余走 ego REST（agent 元数据 + 个体识别 + 角色设定，
    /// 失败静默跳过，全部失败回退默认提示词"你是 kissbot 智能助手"）
    pub async fn system_prompt_for_agent(&self, agent_id: &str, role_name: &str) -> Result<String> {
        if agent_id == RESERVED_AGENT_ID {
            return Ok(ConfigManager::get().default_system_prompt().await);
        }
        let mut system_parts = vec![];
        // agent 自身活跃标识集合：来自各 channel 绑定身份（messenger_id, user_id；群组不限定）
        let mut ids = std::collections::HashSet::new();
        for (_, ch) in ConfigManager::get().channels().await {
            for bu in &ch.bind_users {
                ids.insert(kissbot_api::ChannelUser {
                    messenger_id: bu.messenger_id.clone(),
                    user_id: bu.user_id.clone(),
                });
            }
        }
        // 匹配的个体名，用于角色设定的 other_roles 过滤
        let mut individual_names = std::collections::HashSet::new();

        // 1. agent 元数据（按 agent_id 查询）-> 身份 markdown
        if let Ok(Some(metadata)) = self.memory_ego_client.get_agent(agent_id).await {
            system_parts.push(crate::ego_md::build_ego_identity_md(&metadata));
        }
        // 2. 个体识别（按 agent_id 查询）-> 个体识别 markdown，并收集匹配个体名
        if let Ok(Some(individuals)) = self.memory_ego_client.get_individuals(agent_id).await {
            for (name, entry) in individuals.individual_map.iter() {
                let individual = entry.load();
                if individual.identifiers.iter().any(|id| ids.contains(id)) {
                    individual_names.insert(name.clone());
                }
            }
            system_parts.push(crate::ego_md::build_ego_individual_recognition_md(&individuals, &ids));
        }
        // 3. 角色设定（按 agent_id + role_name 查询）-> 角色 markdown
        if !role_name.is_empty() {
            if let Ok(Some(role)) = self.memory_ego_client.get_role(agent_id, role_name).await {
                system_parts.push(crate::ego_md::build_role_play_md(&role, &individual_names));
            }
        }

        if system_parts.is_empty() {
            system_parts.push("你是 kissbot 智能助手".to_string());
        }

        Ok(system_parts.join("\n"))
    }
```

注意：原实现用 `let ego_url = ApiConfig::get().memory_ego_url.clone(); let client = reqwest::Client::new(); let api_key = ...` 全部消失（归 MemoryEgoClient）；其余逻辑（IDs/individual_names 收集、ego_md 调用、回退）原样保留。

- [ ] **Step 5: verify_agent_exists 改用 memory_ego_client**

替换原实现（139-167 行）：
```rust
    /// 校验 agent_id 存在（/agent 切换前调用）：空或保留 id "0" 直接通过；
    /// ego 未配置/HTTP 失败/agent 不存在返回 Err（调用方保持原 agent 不变）
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
（原 `let client = reqwest::Client::new();` 与 ego_url 空检查消失——空检查语义由 agent_exists 承担）

- [ ] **Step 6: ensure_session 改薄封装 + 4 调用点适配**

替换 ensure_session（166-193 行）为：
```rust
    /// 定位会话（不存在则创建；创建时上下文恢复/重建 + 系统消息在 get_or_create 内部完成）；返回会话（无"是否新建"标记）
    async fn ensure_session(&self, key: &SessionKey) -> Arc<Session> {
        // load_full() 直接返回 Arc<Option<ProviderModel>>（O(1)），零深拷贝传给 get_or_create
        let model = self.valid_default.load_full();
        self.session_manager.get_or_create(key, model).await
    }
```

4 个调用点（仅去 channel_id 参数与 tuple 解构；model 获取在 ensure_session 内保留，调用点不重复）：
- apply_channel_key（约 337）：`self.ensure_session(&key, channel_id).await;` → `self.ensure_session(&key).await;`
- set_session_model（约 362）：`let (session, _) = self.ensure_session(&key, channel_id).await;` → `let session = self.ensure_session(&key).await;`
- run()（约 373）：`self.ensure_session(&key, &ch.channel_id).await;` → `self.ensure_session(&key).await;`
- handle_incoming_message（约 448）：`let (session, _) = self.ensure_session(&key, channel_id).await;` → `let session = self.ensure_session(&key).await;`

- [ ] **Step 7: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: check 无警告；`test result: ok. 121 passed`；`rg "reqwest::Client::new" kissbot-agent/src/coordinator.rs` 零残留；`rg "load_ego_info" kissbot-agent/src/` 零残留
（注意：Task 1 完成后 session_manager.rs 仍引用 `session.context`/`AgentCoordinator::get().build_context_from_memory_store` 等——**Task 1 只改 coordinator/memory_ego_client，不动 session_manager**，因此编译可通过（ensure_session 薄封装调 get_or_create 的现有签名 tuple？**不**——get_or_create 仍是旧签名 `-> (Arc<Session>, bool)` 且同步！薄封装 `self.session_manager.get_or_create(key, model).await` 返回 tuple，`Arc<Session>` 类型不匹配 → 编译失败！）

**关键依赖顺序**：Task 1 的 ensure_session 薄封装依赖 Task 2 的 get_or_create 新签名（async + Arc + 返回 Arc<Session>）。因此**Task 1 与 Task 2 必须一起完成**（不可分任务提交后编译断）——或者 Task 1 只做 memory_ego_client + coordinator 其余部分（verify/system_prompt_for_agent），ensure_session 适配放到 Task 2。**修正：Task 1 不含 ensure_session/调用点适配（Step 6 移至 Task 2）**，Task 1 结束时 get_or_create 旧签名兼容（ensure_session 原样保留或临时保留旧实现），编译可过。见 Task 2 Step 1 统一处理。

- [ ] **Step 8: Commit**

```bash
git add kissbot-agent/src/memory_ego_client.rs kissbot-agent/src/main.rs kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): 新建 MemoryEgoClient（共享连接池替代每次 Client::new），load_ego_info 内联进 system_prompt_for_agent，verify_agent_exists 经 client 校验"
```

---

### Task 2: session_manager 改造（get_or_create 承担初始化）+ coordinator 调用适配

**Files:**
- Modify: `kissbot-agent/src/session_manager.rs`
- Modify: `kissbot-agent/src/coordinator.rs`（ensure_session 薄封装 + 4 调用点，Task 1 遗留的 Step 6）

**Interfaces:**
- Produces: `get_or_create(&self, key, model: Arc<Option<ProviderModel>>) -> Arc<Session>`（async）；`create_session(key, model: Arc<Option<ProviderModel>>, data_dir) -> Arc<Session>`（async，含初始化块）；Session 字段重排 + pub 收窄；`ensure_test_globals` 测试 helper
- Consumes: `AgentCoordinator::get().build_context_from_memory_store` / `system_prompt_for_agent`、`ConfigManager` 单例

- [ ] **Step 1: ensure_session 薄封装 + 4 调用点（Task 1 遗留）**

与 Task 1 Step 6 内容相同——本任务先做（因依赖 get_or_create 新签名，且 ensure_session/调用点在 coordinator.rs，改后需与 session_manager 同步编译）。

- [ ] **Step 2: get_or_create / create_session async + Arc 参数 + 初始化搬入**

替换 get_or_create（599-616 行）与 create_session（618-668 行）：

```rust
    /// 定位会话，不存在则创建（model 为初始模型 Arc，None = 无模型；agent_id 为会话状态保存的解析结果）；
    /// 返回会话（无"是否新建"标记；创建时的初始化——上下文恢复/重建 + 系统消息——在 create_session 内部完成）
    /// 创建时依赖序组装（内联 new_producer/BatchConsumer::new）：notify → 2 mpsc → producer → session → consumer → spawn
    /// （channel 均从 session.batch_producer 取 clone；任务持 consumer，consumer 持 session 弱引用与 notify，
    ///  anchor/deadline/notify 均为独立 Arc——producer 与 consumer 共享同一份）
    /// 双重锁定：先 get 快速路径（命中直接返回），未命中再走 entry API 原子创建（并发下仅一个创建成功）
    pub async fn get_or_create(&self, key: &SessionKey, model: Arc<Option<ProviderModel>>) -> Arc<Session> {
        if let Some(s) = self.sessions.get(key) {
            return s.clone();
        }
        match self.sessions.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(e) => e.get().clone(),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                // 创建部分抽出（create_session）：依赖序组装 + 新建会话初始化 + spawn 触发任务
                let session = Self::create_session(key, model, &self.data_dir).await;
                e.insert(session.clone());
                session
            }
        }
    }

    /// 创建会话（get_or_create 的创建分支抽出）：依赖序组装（内联 new_producer/BatchConsumer::new）+
    /// 新建会话初始化（上下文恢复/重建 + 系统消息，原 Coordinator::ensure_session 的 created 分支搬入）+
    /// spawn 触发任务（内联 spawn_trigger：tokio::spawn(consumer.run())）；返回新建会话
    /// （channel 均从 session.batch_producer 取 clone；任务持 consumer，consumer 持 session 弱引用与 notify，
    ///  anchor/deadline/notify 均为独立 Arc——producer 与 consumer 共享同一份）
    async fn create_session(
        key: &SessionKey,
        model: Arc<Option<ProviderModel>>,
        data_dir: &str,
    ) -> Arc<Session> {
        // 1. notify + anchor + deadline + 2 mpsc（无依赖；各 Arc 单独建立，复制给 producer/consumer）
        let notify = Arc::new(Notify::new());
        let anchor = Arc::new(Instant::now());
        let deadline = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::unbounded_channel();
        let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        // 2. 用 tx 构造 producer（anchor/deadline 复制自独立 Arc）
        let producer = BatchProducer {
            tx,
            trigger_tx,
            anchor: anchor.clone(),
            deadline: deadline.clone(),
        };
        // 3. 用 producer 构造 session（字面量，无 new 函数；Session 全字段在同文件内可见）
        // model 经 ArcSwap::from(Arc) 直接转移（零深拷贝，替代旧 from_pointee 值克隆）
        let session = Arc::new(Session {
            agent_id: Arc::new(key.agent_id.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            model: ArcSwap::from(model),
            context: tokio::sync::Mutex::new(SessionContext::new(data_dir, key)),
            batch_producer: producer,
            notify: notify.clone(),
        });
        // 4. 用 rx 和 session 构造 consumer（anchor/deadline/notify 均与 producer 共享同一 Arc）
        let consumer = BatchConsumer {
            rx,
            trigger_rx,
            delay: DelayQueue::new(),
            session: Arc::downgrade(&session),
            notify,
            anchor,
            deadline,
        };
        // 5. 新建会话初始化（原 Coordinator::ensure_session 的 created 分支；spawn 前执行，任务启动时上下文已就绪）：
        //    Event 从缓存恢复（全量回读；文件不存在为空，不清理）；Role 查询记忆重建（归档+清空在 archive_... 内部）
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
        // 系统消息：保留 agent（agent_id="0"）用 NexusRepo 默认系统提示词；其余走 ego REST（失败跳过设置）
        if let Ok(prompt) = AgentCoordinator::get()
            .system_prompt_for_agent(session.agent_id.as_str(), &session.role_name).await
        {
            session.context.lock().await.set_system_message(prompt);
        }
        // 6. consumer 去 spawn（内联 spawn_trigger）
        tokio::spawn(consumer.run());
        session
    }
```

- [ ] **Step 3: Session 字段重排 + pub 收窄**

Session struct（约 249-262 行）改为：
```rust
/// 单个会话：独立上下文、模型与模式状态
pub struct Session {
    pub agent_id: Arc<String>,      // 运行态：从 key 复制
    pub role_name: Arc<String>,     // 运行态：从 key 复制
    pub mode: Arc<Mode>,            // 运行态：从 key 复制
    /// 会话级模型（创建时取 default_model，/model 调整）；None = 无模型（普通消息静默忽略）
    pub model: ArcSwap<Option<ProviderModel>>,
    /// 会话上下文（内存消息 + 本地缓存 + 历史归档；coordinator 不直接访问，统一经 Session 方法/内部逻辑）
    context: tokio::sync::Mutex<SessionContext>,
    /// 合批生产侧（依赖序构造时经 create_session 传入；channel 均从本字段取 clone 绑定）
    batch_producer: BatchProducer,
    /// 会话销毁通知（Drop 时 notify_one → trigger 任务退出；与 consumer.notify 同一 Arc）
    notify: Arc<Notify>,
}
```

- [ ] **Step 4: 测试适配**

session_manager tests：

a) tests mod 顶部加 `use std::sync::atomic::AtomicBool;`（如无）与 helper：

```rust
    /// 测试进程级装配（幂等）：ConfigManager/AgentCoordinator 单例各注册一次。
    /// create_session 的初始化逻辑依赖这两个单例（build_context_from_memory_store / system_prompt_for_agent），
    /// get_or_create 相关测试前需先装配；data_dir 目录经 OnceLock 保活，避免 tempdir drop 后单例路径失效
    static TEST_GLOBAL_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static TEST_INIT_DONE: AtomicBool = AtomicBool::new(false);
    async fn ensure_test_globals() {
        if !TEST_INIT_DONE.load(Ordering::Relaxed) {
            let dir = TEST_GLOBAL_DIR.get_or_init(|| tempfile::tempdir().unwrap());
            let cfg_path = dir.path().join("config.json");
            let cfg_json = format!(
                r#"{{"security":{{"api_key":"user-key-456","admin_api_key":"admin-key-123"}},"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9091,"ws_reconnect_interval_secs":5}}}}"#,
                dir.path().join("data").to_str().unwrap()
            );
            std::fs::write(&cfg_path, cfg_json).unwrap();
            // 2024 edition：设置环境变量需要 unsafe
            unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };
            // 幂等：ConfigManager::new() 注册一次（第二实例丢弃）；AgentCoordinator 同理（SINGLETON.set 失败被忽略）
            let _ = ConfigManager::new().await;
            let _ = AgentCoordinator::new().await;
            TEST_INIT_DONE.store(true, Ordering::Relaxed);
        }
    }
```

（参考 http_server.rs 测试 test_manager 的配置 JSON 结构；如 tests mod 已 import OnceLock/AtomicBool/Ordering 则复用）

b) `get_or_create_dedupes`（约 782-796）：
```rust
    #[tokio::test]
    async fn get_or_create_dedupes() {
        ensure_test_globals().await;
        let mgr = mgr();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k = key("a1", "r1");
        let s1 = mgr.get_or_create(&k, Arc::new(Some(model.clone()))).await;
        let s2 = mgr.get_or_create(&k, Arc::new(Some(model.clone()))).await;
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同 mode 是不同会话
        let k_event = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let _s3 = mgr.get_or_create(&k_event, Arc::new(Some(model))).await;
    }
```

c) `get_or_create_with_none_model`（约 798-803）：
```rust
    #[tokio::test]
    async fn get_or_create_with_none_model() {
        ensure_test_globals().await;
        let mgr = mgr();
        let key = SessionKey { agent_id: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let s = mgr.get_or_create(&key, Arc::new(None)).await;
        assert!(s.model.load().is_none());
    }
```

d) tests 内其他 get_or_create 调用点（如有）同步 `.await` + Arc 参数。

e) `test_pair`（约 717-722）Session 字面量字段顺序与 struct 新顺序一致（字段名顺序在 Rust 字面量中不强制，但按新顺序书写保持风格一致；context/batch_producer/notify 为私有字段，tests mod 子模块可访问，无需改 pub）。

- [ ] **Step 5: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: check 无警告；`test result: ok. 121 passed`；`rg "ensure_session\(" kissbot-agent/src/coordinator.rs` 仅剩薄封装定义 + 4 调用点（无旧 tuple 解构）
Run: `rg "get_or_create" kissbot-agent/src/session_manager.rs` 确认签名统一（async + Arc）
Run 3 次 `cargo test` 观察无 flaky（session_manager 新 set_var 与 http_server 既有 set_var 的并行竞态观察）

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/session_manager.rs kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): 会话创建初始化归位——get_or_create 返回 Arc<Session>（去 created bool）并承担初始化（上下文恢复/重建+系统消息），model 参数 Arc 化零深拷贝（ArcSwap::from），ensure_session 薄封装，Session 字段重排（model 移 context 前）且 context/batch_producer/notify 去 pub"
```

---

### Task 3: 全量扫尾验证

**Files:**
- Verify only（发现问题才改）

- [ ] **Step 1: 残留检查**

Run: `cd /home/admin/project/kissbot && rg "reqwest::Client::new|load_ego_info" kissbot-agent/src/`（coordinator.rs 零残留；memory_ego_client.rs 允许 Client::new 一处）
Expected: 仅 memory_ego_client.rs 一处 `reqwest::Client::new`
Run: `rg "\.context\.lock\(\)|\.batch_producer\b|\.notify\b" kissbot-agent/src/coordinator.rs`
Expected: 零残留（coordinator 不再直接访问这三个字段）

- [ ] **Step 2: 全量验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test 2>&1 | tail -3`
Expected: check 无警告；`test result: ok. 121 passed`

- [ ] **Step 3: 提交（如有残留修复）**

```bash
git add -A
git commit -m "refactor(agent): 会话创建初始化归位扫尾（残留清理）"
```

---

## 自审

**1. Spec 覆盖：**
- D1（MemoryEgoClient）→ Task 1 Step 1-3 ✓
- D2（load_ego_info 内联）→ Task 1 Step 4 ✓
- D3（verify_agent_exists）→ Task 1 Step 5 ✓
- D4（ensure_session 薄封装）→ Task 2 Step 1 ✓
- D5（get_or_create/create_session）→ Task 2 Step 2 ✓
- D6（字段可见性 + 顺序）→ Task 2 Step 3 ✓
- D7（测试）→ Task 2 Step 4 ✓
- 验证 → Task 3 ✓

**2. 占位符扫描：** 无 TBD/TODO；每步含完整代码或精确指令。

**3. 类型一致性：**
- `MemoryEgoClient::get_agent` 返回 `Result<Option<AgentMetadata>>` 与 `system_prompt_for_agent` 的 `if let Ok(Some(metadata))` 匹配 ✓
- `get_or_create(key, Arc<Option<ProviderModel>>) -> Arc<Session>` 与 ensure_session 薄封装 `self.valid_default.load_full()`（`Arc<Option<ProviderModel>>`）匹配 ✓
- `ArcSwap::from(Arc<Option<ProviderModel>>)`：arc-swap 1.9 `impl From<T: RefCnt> for ArcSwapAny<T>`，`Arc<T>: RefCnt` ✓
- Session 新字段顺序与 create_session/test_pair 字面量一致；model 为最后 pub 字段 ✓
- `verify_agent_exists` 关联函数内 `AgentCoordinator::get()`：运行期单例已注册 ✓（测试仅覆盖保留 id 直通，不触单例）
- ensure_session 返回 `Arc<Session>` 与 4 调用点（337/362/373/448）`let session = ...` / 丢弃 匹配 ✓
- **任务间依赖**：Task 1 不含 ensure_session 适配（避免编译断），Task 2 Step 1 统一处理 ✓
