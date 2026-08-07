# Terminal 全局唯一 + Arc<Self> Receiver 重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Terminal trait receiver 改 `self: Arc<Self>`，AgentCoordinator 直接 impl Terminal（删 TerminalHandle），connect_channels 用全局唯一 Terminal，ChannelClient 归入 ChannelContext。

**Architecture:** 三个独立 crate（kissbot-channel-client / kissbot-channel-client-cli / kissbot-agent）。trait 签名先改（Task 1，连带 CLI impl 同步）；随后 agent 侧删 TerminalHandle、固有方法 `&Arc<Self>` 统一 `self: Arc<Self>`（Task 2）；最后 client 归入 ChannelContext、删 channel_clients（Task 3）。重构不改变行为，验证靠编译 + 现有 106 测试。

**Tech Stack:** Rust、async-trait 0.1（官方测试确认支持 `self: Arc<Self>`）、ArcSwapOption、tokio。

## Global Constraints

- 不要删除代码中的注释（更新措辞而非删除；本计划所有删改处均给出替换注释）
- 读写文件必须用 Read/Write/Edit 工具，禁止 sed/python 修改文件
- 提交 comment 用中文，覆盖本次改动全部内容
- 各 crate 独立（无根 workspace）：`cd kissbot-agent && cargo test` 等
- 非枚举/非 Map Key/非 Vec 字段用 `Arc<T>` 包裹（ArcSwapOption 无锁优先）
- 测试基线：kissbot-agent **106 passed / 0 warnings**（重构不改变行为，每任务后必须全绿）

---

### Task 1: Terminal trait 签名改 `self: Arc<Self>` + CLI 同步

**Files:**
- Modify: `kissbot-channel-client/src/terminal.rs`（trait 6 方法 receiver + 注释）
- Modify: `kissbot-channel-client-cli/src/main.rs:110-160`（impl Terminal for CliTerminal 6 方法签名）

**Interfaces:**
- Produces: `pub trait Terminal: Send + Sync + 'static`，6 方法 receiver 均为 `self: Arc<Self>`（参数与返回不变）。ChannelClient 调用点（`get_terminal()` 每次 upgrade 得 `Arc<dyn Terminal>`）天然匹配，零改动。

- [ ] **Step 1: 改 trait 签名（kissbot-channel-client/src/terminal.rs）**

```rust
/// 终端接口：ChannelClient 收到服务端推送后调用的回调函数。
/// id 是触发事件的 ChannelClient 的标识（由 ChannelClient::new 时传入）。
/// receiver 用 self: Arc<Self>（by-value Arc）：调用方（ChannelClient）持 Weak<dyn Terminal>，
/// upgrade 得 Arc<dyn Terminal> 后直接调用——与 &self 不同，方法内可直接持有/降级 Arc 自身
/// （&Arc<Self> receiver 不可对象化 E0038，trait 里无法声明）。
#[async_trait]
pub trait Terminal: Send + Sync + 'static {
    /// 收到上行消息（含接收方 recipient_user_id）
    async fn incoming_message(self: Arc<Self>, id: &str, message: Arc<IncomingMessageEvent>);
    /// 用户加入群组
    async fn join_group(self: Arc<Self>, id: &str, notification: Arc<GroupChangeNotification>);
    /// 用户离开群组
    async fn leave_group(self: Arc<Self>, id: &str, notification: Arc<GroupChangeNotification>);
    /// 用户被删除
    async fn user_removed(self: Arc<Self>, id: &str, notification: Arc<UserRemoveNotification>);
    /// 下载分块到达（Ok/Err 即该块的确认结果）
    async fn download_chunk(self: Arc<Self>, id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()>;
    /// 连接关闭（不做自动重连）
    async fn closed(self: Arc<Self>, id: &str);
}
```

（仅 `&self` → `self: Arc<Self>` 六处 + 顶部注释段落更新；参数/返回类型不动）

- [ ] **Step 2: 编译验证 kissbot-channel-client**

Run: `cd kissbot-channel-client && cargo check`
Expected: 通过（ChannelClient 内 6 处调用点 `terminal.xxx(...)` 中 terminal 是 `Arc<dyn Terminal>`，与 `self: Arc<Self>` 匹配）

- [ ] **Step 3: 同步 CLI impl 签名（kissbot-channel-client-cli/src/main.rs）**

`impl Terminal for CliTerminal` 的 6 个方法签名 `&self` → `self: Arc<Self>`，方法体不变（Arc deref 访问字段照常；closed 里 `std::process::exit(0)` 照常）：

```rust
#[async_trait]
impl Terminal for CliTerminal {
    async fn incoming_message(self: Arc<Self>, _id: &str, message: Arc<IncomingMessageEvent>) {
        // 打印 content 原始 JSON 串
        let json = serde_json::to_string(&message.incoming_message.content).unwrap();
        println!("<< [{}:{}] {}", message.incoming_message.user_id, message.incoming_message.group_id, json);
    }

    async fn join_group(self: Arc<Self>, _id: &str, notification: Arc<GroupChangeNotification>) {
        println!("<< join group: {} @ {}", notification.group_id, notification.messenger_id);
    }

    async fn leave_group(self: Arc<Self>, _id: &str, notification: Arc<GroupChangeNotification>) {
        println!("<< leave group: {} @ {}", notification.group_id, notification.messenger_id);
    }

    async fn user_removed(self: Arc<Self>, _id: &str, notification: Arc<UserRemoveNotification>) {
        println!("<< user removed: {} @ {}", notification.user_id, notification.messenger_id);
    }

    async fn download_chunk(self: Arc<Self>, _id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.download_dir)?;
        let path = format!("{}/{}", self.download_dir, info.info.file_name);
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(&data)?;
        if pos + data.len() as u64 >= info.info.size_bytes {
            println!(">> downloaded to {}", path);
        }
        Ok(())
    }

    async fn closed(self: Arc<Self>, _id: &str) {
        println!("!! connection closed");
        std::process::exit(0);
    }
}
```

（main.rs:170 `Arc::downgrade(&cli_terminal) as Weak<dyn Terminal>` 不动）

- [ ] **Step 4: 编译验证 CLI**

Run: `cd kissbot-channel-client-cli && cargo build`
Expected: 通过

- [ ] **Step 5: Commit**

```bash
git add kissbot-channel-client/src/terminal.rs kissbot-channel-client-cli/src/main.rs
git commit -m "refactor(channel): Terminal trait 方法 receiver 改 self: Arc<Self>（by-value Arc 可对象化，&Arc<Self> 报 E0038；调用方 Weak upgrade 得 Arc<dyn Terminal> 天然匹配，ChannelClient 零改动）；CLI impl 签名同步"
```

注：本任务后 `kissbot-agent` 编译暂失败（TerminalHandle 的 `&self` impl 与 trait 不匹配），Task 2 紧跟恢复，勿单独提交 agent。

---

### Task 2: 删 TerminalHandle，AgentCoordinator 直接 impl Terminal + 固有方法 `self: Arc<Self>` + connect_channels 全局 Terminal

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`（主体）
- Modify: `kissbot-agent/src/command_router.rs:296`（execute 内 set_session_model 调用点 1 处）

**Interfaces:**
- Consumes: Task 1 的 `Terminal` trait（`self: Arc<Self>` receiver）
- Produces: `impl Terminal for AgentCoordinator`（6 方法）；固有方法全部 `self: Arc<Self>`：`ensure_session(&SessionKey, &str) -> (Arc<Session>, bool)` / `relocate_channel(&str)` / `apply_channel_key(&str, &SessionKey, Option<Arc<String>>) -> Result<()>` / `set_session_model(&str, ProviderModel) -> Result<()>` / `connect_channels()` / `handle_incoming(&str, Arc<ChannelConfig>, Arc<IncomingMessageEvent>)` / `handle_admin_command(&str, &Arc<IncomingMessageEvent>, &str)`。`CommandRouter::execute` 保持 `&Arc<AgentCoordinator>`（Task 3 与 Task 2 均依赖）。

- [ ] **Step 1: 删除 TerminalHandle 结构与其 impl，新增 `impl Terminal for AgentCoordinator`（coordinator.rs:721-757 区域）**

将 `TerminalHandle` 结构体、`impl Terminal for TerminalHandle` 整块替换为：

```rust
// ==================== Terminal 回调（AgentCoordinator 直接实现；固有方法） ====================

/// Terminal 即 Coordinator 自身（全局唯一）：trait receiver 为 self: Arc<Self>（by-value Arc），
/// 方法内可直接持 Arc 调用 Arc 链方法（构造会话降级自身弱引用）——不再需要 TerminalHandle 适配器；
/// ChannelClient 持 Weak<dyn Terminal>，connect_channels 中全部 client 弱引用指向同一 coordinator
impl Terminal for AgentCoordinator {
    /// 收到上行消息（event 含接收方 recipient_user_id）
    async fn incoming_message(self: Arc<Self>, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        // 1. 来源 channel 必须在配置中
        let Some(ch) = self.config.channel(channel_id).await else { return; };

        // 2. msg_id 回显判定：命中（已发未回显）则跳过，不存 record、不进 agentic loop
        if self.is_self_echo_by_msg_id(channel_id, &event.incoming_message.msg_id).await {
            return;
        }

        // 3. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 取来源 channel 运行态绑定，事件模式编码）
        let key = self.session_key_for(&ch);
        let role_name = memory_role(&key.role_name, &key.mode);
        let agent_id = self.channel_agent(channel_id).await;
        self.memory_store_client.push_channel_record(ChannelRequest {
            agent_id,
            role_name: Arc::new(role_name),
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.incoming_message.user_id.clone(),
            // 接收方身份 = event.recipient_user_id（agent 视角的 self；与 is_self 不同，其他人用绑定用户发消息时 user_id == self_user_id 但 is_self == 0）
            self_user_id: event.recipient_user_id.clone(),
            group_id: event.incoming_message.group_id.clone(),
            is_self: 0,
            messenger_name: event.incoming_message.messenger_name.clone(),
            user_name: event.incoming_message.user_name.clone(),
            group_name: event.incoming_message.group_name.clone(),
            content: event.incoming_message.content.clone(),
            time: event.incoming_message.time.clone(),
        }).await;

        // 4. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
        self.clone().handle_incoming(channel_id, ch, event).await;
    }

    async fn join_group(self: Arc<Self>, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组加入事件，当前暂不处理
    }

    async fn leave_group(self: Arc<Self>, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组离开事件，当前暂不处理
    }

    async fn user_removed(self: Arc<Self>, _id: &str, _notification: Arc<UserRemoveNotification>) {
        // 用户删除事件，当前暂不处理
    }

    async fn download_chunk(self: Arc<Self>, _id: &str, _info: Arc<AttachmentInfoResponse>, _pos: u64, _data: Bytes) -> std::result::Result<(), kissbot_channel_client::Error> {
        // 当前未使用附件下载
        Ok(())
    }

    async fn closed(self: Arc<Self>, id: &str) {
        info!("channel 连接关闭: {}，准备重连", id);
        // 通知重连循环
        if let Some(notify) = self.disconnect_notify.get(id) {
            notify.notify_one();
        }
    }
}
```

注：原 `incoming_message`/`join_group`/`leave_group`/`user_removed`/`download_chunk`/`closed` 固有方法（758-815 区域，`impl AgentCoordinator` 内）随之删除——其逻辑全部迁入上述 impl Terminal（`incoming_message` 的 `&self` 固有版本删除后只剩 trait 版本）。

- [ ] **Step 2: 8 个固有方法签名 `&Arc<Self>` → `self: Arc<Self>`**

将以下方法签名逐一替换（仅 receiver 变化，参数/返回不变）：

```rust
    // 314
    async fn ensure_session(self: Arc<Self>, key: &SessionKey, channel_id: &str) -> (Arc<Session>, bool) {
    // 379
    async fn relocate_channel(self: Arc<Self>, channel_id: &str) {
    // 589
    async fn apply_channel_key(self: Arc<Self>, channel_id: &str, new_key: &SessionKey, agent_id: Option<Arc<String>>) -> Result<()> {
    // 610
    pub async fn set_session_model(self: Arc<Self>, channel_id: &str, pm: ProviderModel) -> Result<()> {
    // 650
    async fn connect_channels(self: Arc<Self>) {
    // 821
    async fn handle_incoming(
        self: Arc<Self>,
        channel_id: &str,
        ch: Arc<crate::config_manager::ChannelConfig>,
        event: Arc<IncomingMessageEvent>,
    ) {
    // 881
    async fn handle_admin_command(
        self: Arc<Self>,
        channel_id: &str,
        event: &Arc<IncomingMessageEvent>,
        content: &str,
    ) {
```

- [ ] **Step 3: 方法体内 Arc<Self> receiver 调用点改 `self.clone().xxx()`**

`self: Arc<Self>` 后 `self.ensure_session(...)` 等会 move self，以下 6 处改为 `self.clone().xxx()`（Arc clone 廉价；其他 `&self` 方法调用如 `self.config.xxx()`/`self.bind_batch(...)` 自动解引用，不动）：

```rust
    // relocate_channel 内（原 385）
    self.clone().ensure_session(&key, channel_id).await;

    // apply_channel_key 内（原 598）
    self.clone().relocate_channel(channel_id).await;

    // set_session_model 内（原 622）
    let (session, _) = self.clone().ensure_session(&key, channel_id).await;

    // handle_incoming 内（原 839）
    self.clone().handle_admin_command(channel_id, &event, &content_text).await;

    // handle_incoming 内（原 850）
    let (session, _) = self.clone().ensure_session(&key, channel_id).await;
```

（`ensure_session` 内部的 `self.session_manager.get_or_create(key, model, agent_id, Arc::downgrade(&self))`——`&self` 是 `&Arc<Self>`，`Arc::downgrade` 接受 `&Arc<T>`，**不改**）

- [ ] **Step 4: 外部调用点适配（coordinator.rs）**

```rust
    // 165（command_rx 循环闭包内，coordinator 每轮复用 → clone 保留下轮）
    let rst = coordinator.clone().apply_channel_key(&channel_id, &new_key, agent_id).await;

    // 201（new 末尾循环内）
    coordinator.clone().ensure_session(&key, &ch.channel_id).await;

    // 205（最后调用，直接 move）
    coordinator.connect_channels().await;
```

- [ ] **Step 5: command_router.rs:296 调用点适配**

execute 收 `&Arc<AgentCoordinator>`（不变），内部调 `set_session_model`（现 receiver `self: Arc<Self>`）需 clone：

```rust
    coordinator.clone().set_session_model(channel_id, pm.clone()).await?;
```

（execute 签名 `coordinator: &Arc<AgentCoordinator>` 不变；handle_admin_command 内调用点 `CommandRouter::execute(&cmd, &self.config, self, channel_id).await` 传 `&self`（`&Arc<Self>` 借用）**不改**）

- [ ] **Step 6: connect_channels 用全局唯一 Terminal（删 _keepalive；client 暂仍插 channel_clients，Task 3 迁移）**

`connect_channels`（650）整体替换为：

```rust
    async fn connect_channels(self: Arc<Self>) {
        let reconnect_secs = self.config.ws_reconnect_interval_secs();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        let coordinator = self.clone();

        // Terminal 即 Coordinator 自身（全局唯一）：不再新建 TerminalHandle 适配器，
        // 循环外建一次 Terminal 视图，所有 channel client 的 Weak<dyn Terminal> 指向同一目标；
        // 强引用由 coordinator 其余 Arc（command_rx 循环等）保活——无需任务内 _keepalive
        let terminal: Arc<dyn Terminal> = coordinator.clone();

        // 遍历 NexusRepo 中所有 channel，enabled 才连接
        for (_, ch) in self.config.channels().await {
            if !ch.enabled {
                continue; // 未启用：不连接
            }
            let channel_id = ch.channel_id.to_string();
            let ws_url = ch.ws_url.to_string();

            let client = ChannelClient::new(channel_id.clone(), Arc::downgrade(&terminal));

            // 断线通知
            let notify = Arc::new(tokio::sync::Notify::new());
            coordinator.disconnect_notify.insert(channel_id.clone(), notify.clone());
            coordinator.channel_clients.insert(channel_id.clone(), client);

            let client_clone = coordinator.channel_clients.get(&channel_id).unwrap().clone();
            let api_key = api_key.clone();
            // 重连循环内实时读取绑定身份（/bind 回写后重连即生效），需持有 coordinator 引用
            let coordinator_clone = coordinator.clone();

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
                        Ok(()) => {
                            info!("已连接 channel: {}", channel_id);
                            // 绑定身份实时读取（bind_users 逐个绑定；BindRequest.messenger_id 用绑定身份的 messenger 标识，如 "web"）
                            let bind_users = coordinator_clone.config.channel(&channel_id).await
                                .map(|c| c.bind_users.clone());
                            if let Some(bus) = bind_users {
                                for bu in bus {
                                    let _ = client_clone.bind(BindRequest {
                                        messenger_id: Arc::new(bu.messenger_id.clone()),
                                        user_id: Arc::new(bu.user_id.clone()),
                                    }).await;
                                }
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
        }
    }
```

（相比原版：receiver `self: Arc<Self>`；循环外 `let terminal: Arc<dyn Terminal> = coordinator.clone()`；client 用 `Arc::downgrade(&terminal)`；spawn 闭包内删 `let _keepalive = terminal;`）

- [ ] **Step 7: 验证**

Run: `cd kissbot-agent && cargo check && cargo test 2>&1 | grep "test result"`
Expected: check 通过；`test result: ok. 106 passed; 0 failed`

- [ ] **Step 8: Commit**

```bash
git add kissbot-agent/src/coordinator.rs kissbot-agent/src/command_router.rs
git commit -m "refactor(agent): 删 TerminalHandle，AgentCoordinator 直接 impl Terminal（trait receiver 改 self: Arc<Self> 后无需适配器，回调逻辑搬入 impl）；8 个固有方法 &Arc<Self> 统一 self: Arc<Self>（方法内/外部调用点 self.clone() 适配，execute 保持 &Arc 传 &self）；connect_channels 循环外建全局唯一 Terminal 视图（client 弱引用共享同一目标），删任务内 _keepalive（强引用由 coordinator 其余 Arc 保活）"
```

---

### Task 3: ChannelClient 归入 ChannelContext，删 channel_clients

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: Task 2 的 `connect_channels(self: Arc<Self>)`、`AgentCoordinator.channel_contexts`
- Produces: `ChannelContext::bind_client(&self, Arc<ChannelClient>)`、`ChannelContext::client(&self) -> Option<Arc<ChannelClient>>`；`AgentCoordinator` 不再有 `channel_clients` 字段

- [ ] **Step 1: ChannelContext 加 client 字段（coordinator.rs:46-68 区域）**

```rust
    /// 运行态模式（ArcSwap 无锁读写；/mode 切换不回写，重启回 Role）
    mode: ArcSwap<Mode>,
    /// 本 channel 的 ChannelClient（connect_channels 时绑定；消息/回复路径从本字段取 client，
    /// ArcSwapOption 无锁读写——连接与回调并发访问安全）
    client: ArcSwapOption<ChannelClient>,
    /// 合批生产侧（绑定会话时从 session.batch_producer 取 clone；会话重定位后刷新，None 时 enqueue 懒绑定）
    /// BatchProducer 字段全 Clone/Arc，无需锁——ArcSwapOption 原子替换/读取（与 agent_id 同模式）
    producer: ArcSwapOption<crate::session_manager::BatchProducer>,
```

`ChannelContext::new()` 加初始化：

```rust
            mode: ArcSwap::from_pointee(Mode::Role),
            client: ArcSwapOption::new(None),
            producer: ArcSwapOption::new(None),
```

`impl ChannelContext` 内加方法（放在 `new()` 之后）：

```rust
    /// 绑定 channel client（connect_channels 时绑定；每次重连循环启动更新）
    fn bind_client(&self, client: Arc<ChannelClient>) {
        self.client.store(Some(client));
    }

    /// 取 channel client（未连接/未绑定为 None）
    fn client(&self) -> Option<Arc<ChannelClient>> {
        self.client.load_full()
    }
```

- [ ] **Step 2: connect_channels 绑定 client 到 ChannelContext（替换 Step 6 中的三段）**

```rust
            let client = ChannelClient::new(channel_id.clone(), Arc::downgrade(&terminal));

            // 断线通知
            let notify = Arc::new(tokio::sync::Notify::new());
            coordinator.disconnect_notify.insert(channel_id.clone(), notify.clone());
            // ChannelClient 归入该 channel 的 ChannelContext（懒建后 bind；消息/回复路径从 ctx 取 client）
            let ctx = coordinator.channel_contexts
                .entry(channel_id.clone())
                .or_insert_with(|| Arc::new(ChannelContext::new()))
                .clone();
            ctx.bind_client(client.clone());

            let client_clone = client;
            let api_key = api_key.clone();
            // 重连循环内实时读取绑定身份（/bind 回写后重连即生效），需持有 coordinator 引用
            let coordinator_clone = coordinator.clone();

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
```

（相比 Task 2：删 `coordinator.channel_clients.insert(channel_id.clone(), client);` 与 `let client_clone = coordinator.channel_clients.get(&channel_id).unwrap().clone();`，改为 `ctx.bind_client(client.clone()); let client_clone = client;`——任务直接 move Arc）

- [ ] **Step 3: 删 AgentCoordinator.channel_clients 字段与初始化**

```rust
    // 119 字段（连同上方注释行）删除：
    /// 按 agent 内部 channel_id 索引的 ChannelClient
    channel_clients: Arc<DashMap<String, Arc<ChannelClient>>>,
```

```rust
    // 150 初始化删除：
    channel_clients: Arc::new(DashMap::new()),
```

- [ ] **Step 4: 两个使用点改从 channel_contexts 取 client**

`send_admin_reply`（原 917）：

```rust
        let Some(client) = self.channel_contexts.get(channel_id).and_then(|ctx| ctx.client()) else {
            warn!("send_admin_reply: 未找到 channel client: {}", channel_id);
            return;
        };
```

`send_outgoing`（原 1228）：

```rust
        let Some(client) = self.channel_contexts.get(out_channel.channel_id.as_str()).and_then(|ctx| ctx.client()) else {
            warn!("send_outgoing: 未找到 channel client: {}", out_channel.channel_id);
            return;
        };
```

- [ ] **Step 5: 验证**

Run: `cd kissbot-agent && cargo test 2>&1 | grep "test result" && cargo build 2>&1 | grep -c warning`
Expected: `test result: ok. 106 passed; 0 failed`；warnings 计数为 0

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): ChannelClient 归入 ChannelContext（ArcSwapOption 无锁字段 + bind_client/client），删 coordinator.channel_clients DashMap——send_admin_reply/send_outgoing 改从 channel_contexts 取 client；connect_channels 任务直接 move client"
```

---

## Self-Review

**1. Spec coverage：**
- ① trait 签名 → Task 1 Step 1 ✓
- ② 删 TerminalHandle + impl Terminal → Task 2 Step 1 ✓
- ③ 固有方法 Arc<Self> → Task 2 Step 2-5（含 command_router 调用点）✓
- ④ connect_channels 全局 Terminal + 删 _keepalive → Task 2 Step 6 ✓
- ⑤ ChannelContext.client + 删 channel_clients + 使用点 → Task 3 ✓
- ⑥ CLI 同步 → Task 1 Step 3 ✓
- ⑦ 验证 → 各任务末步 ✓

**2. Placeholder scan：** 无 TBD/TODO；所有代码块为完整可粘贴内容 ✓

**3. Type consistency：** `self: Arc<Self>` receiver 在 trait（Task 1）与 impl（Task 2）一致；`bind_client`/`client` 签名 Task 3 内自洽；`CommandRouter::execute(&cmd, &self.config, self, channel_id)` 传 `&self`（`&Arc<AgentCoordinator>`）与 execute 签名（command_router.rs:175）一致 ✓
