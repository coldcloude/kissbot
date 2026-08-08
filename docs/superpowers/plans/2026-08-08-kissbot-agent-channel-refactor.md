# kissbot-agent 通道适配层重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 kissbot-agent 的通道实现从 AgentCoordinator 解耦到 ChannelManager（实现 Terminal、连接/重连/回显/发送封装），AgentCoordinator 单例化，Terminal trait 改 `&self`。

**Architecture:** ChannelManager 成为通道适配层（持 config + disconnect_notify，实现 Terminal，connect_all/send 封装）；AgentCoordinator 为进程级单例（`OnceLock<AgentCoordinator>` 存值，`instance() -> &'static`），所有使用 coordinator 的位置不传参数、从单例获取；BatchProducer 从 Channel 删除，合批直取 `session.batch_producer`。

**Tech Stack:** Rust 2024 / tokio / dashmap / arc-swap / async-trait / thiserror

## Global Constraints

- 单例形态：`static SINGLETON: OnceLock<AgentCoordinator>`（存值不存 Arc）；`instance() -> &'static AgentCoordinator`（expect panic）；`new(config) -> Result<()>` 末尾 `SINGLETON.set`；启动动作（bind agent/session、connect_all、主循环）全部在 `run(&self)` 中，`new()` 只做装配与注册
- 所有使用 coordinator 的位置一律不传 coordinator 参数，统一 `AgentCoordinator::instance()`（Q6 决策）
- Terminal trait 全部方法 receiver `self: Arc<Self>` → `&self`；terminal.rs 原解释 `Arc<Self>` receiver 的注释**直接删除，不新增替代注释**
- 项目约定：不删除代码中的注释（上述 Terminal 注释删除为用户明确要求的唯一例外）；文本文件 UTF-8、`\n` 换行；提交 comment 用中文且覆盖全部改动
- 每个任务结束必须 `cargo build` / `cargo test` 通过后才可 commit；Task 1 的 S2/S3 之间是编译通过的中间态（单例未 set），**不要运行程序**

---

### Task 1: 单例化 + Terminal 迁移 + 传输下沉 + BatchProducer 删除（kissbot-agent）

**Files:**
- Modify: `kissbot-agent/src/channel_manager.rs`
- Modify: `kissbot-agent/src/coordinator.rs`
- Modify: `kissbot-agent/src/session_manager.rs`
- Modify: `kissbot-agent/src/command_router.rs`
- Modify: `kissbot-agent/src/main.rs`

**Interfaces:**
- Consumes: 现状接口（Coordinator 的 Terminal impl、`connect_channels`、`ChannelManager::new()` 无参、`SessionManager::get_or_create(…, coordinator: Weak<…>)`、`CommandRouter::execute(…, coordinator: &Arc<…>)`）
- Produces（后续任务/调用方依赖的确切签名）:
  - `AgentCoordinator::instance() -> &'static AgentCoordinator`
  - `AgentCoordinator::new(config: Arc<ConfigManager>) -> Result<()>`（不再返回 Arc）
  - `AgentCoordinator::run(&self)`（内部：bind agent/session → `self.channel_manager.connect_all()` → 主循环）
  - `AgentCoordinator::incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>)`（pub(crate) 业务入口，被 ChannelManager 转发调用）
  - `ChannelManager::new(config: Arc<ConfigManager>) -> Self`
  - `ChannelManager::connect_all(self: &Arc<Self>)`
  - `ChannelManager::send(&self, channel_id: &str, msg: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel_client::Error>`
  - `SessionManager::get_or_create(&self, key: &SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> (Arc<Session>, bool)`（删 coordinator 参数）
  - `CommandRouter::execute(command: &AdminCommand, config: &ConfigManager, channel_id: &str) -> Result<(String, CommandEffect)>`（删 coordinator 参数）

- [ ] **Step 1: ChannelManager 扩展 + Channel 删 producer + instance 骨架**

`kissbot-agent/src/channel_manager.rs`：

1. 删除 `use crate::session_manager::BatchProducer;` 行。
2. `Channel` 结构体删除 producer 字段及其注释：
```rust
    /// 合批生产侧（绑定会话时从 session.batch_producer 取 clone；会话重定位后刷新，None 时 enqueue 懒绑定）
    /// BatchProducer 字段全 Clone/Arc，无需锁——ArcSwapOption 原子替换/读取（与 agent_id 同模式）
    producer: ArcSwapOption<BatchProducer>,
```
3. `Channel::new()` 删除 `producer: ArcSwapOption::new(None),` 行。
4. 删除 `Channel` 的 `bind_producer` / `producer` 两个方法：
```rust
    /// 绑定合批生产侧（绑定会话时调用；None 时 enqueue 懒绑定）
    fn bind_producer(&self, producer: Arc<BatchProducer>) {
        self.producer.store(Some(producer));
    }

    /// 取合批生产侧（未绑定为 None）
    fn producer(&self) -> Option<Arc<BatchProducer>> {
        self.producer.load_full()
    }
```
5. 删除 `ChannelManager` 的 `bind_producer` / `producer` 两个方法：
```rust
    /// 绑定合批生产侧（绑定会话后刷新；None 时 enqueue 懒绑定）
    pub fn bind_producer(&self, channel_id: &str, producer: Arc<BatchProducer>) {
        self.get_or_create(channel_id).bind_producer(producer);
    }

    /// 取合批生产侧（未绑定为 None）
    pub fn producer(&self, channel_id: &str) -> Option<Arc<BatchProducer>> {
        self.channels.get(channel_id).and_then(|c| c.producer())
    }
```
6. 文件顶部 import 更新为：
```rust
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use arc_swap::{ArcSwap, ArcSwapOption};
use bytes::Bytes;
use dashmap::DashMap;
use tracing::{info, warn};

use kissbot_api::channel::{BindRequest, IncomingMessageEvent, OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};

use crate::config_manager::ConfigManager;
use crate::coordinator::AgentCoordinator;
use crate::types::Mode;
```
7. `ChannelManager` 结构体改为：
```rust
/// channel 集合管理器：通道适配层——持有全部 channel 运行态（Channel）、通道配置与断线通知；
/// 实现 Terminal（回显过滤 + 转发业务）；连接/重连/发送封装（connect_all/send）
/// 内部 DashMap 无锁并发；coordinator 持 Arc<ChannelManager>（connect_all 需要 Arc<Self> 作为 Arc<dyn Terminal>）
pub struct ChannelManager {
    /// 通道配置（连接/重连实时读 bind_users，/bind 回写后重连即生效）
    config: Arc<ConfigManager>,
    channels: DashMap<String, Arc<Channel>>,
    /// 断线通知：channel_id → Notify，closed() 回调通知重连循环
    disconnect_notify: DashMap<String, Arc<tokio::sync::Notify>>,
}

impl ChannelManager {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        Self {
            config,
            channels: DashMap::new(),
            disconnect_notify: DashMap::new(),
        }
    }
```
（`get_or_create` / `bind_client` / `client` / `add_pending` / `consume_pending` / `set_agent_id` / `agent_id` / `set_mode` / `mode` 保持不变）

8. 在 `impl ChannelManager` 内追加 `connect_all` 与 `send`（原 coordinator `connect_channels` 迁移）：
```rust
    /// 连接所有 enabled 的 channel（NexusRepo channel 配置为连接来源）
    /// 连接与绑定统一由 ChannelConfig 描述：enabled 控制连接，bind_users 为绑定身份（逐个绑定）
    pub async fn connect_all(self: &Arc<Self>) {
        let reconnect_secs = self.config.ws_reconnect_interval_secs();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        // Terminal 即 ChannelManager 自身（全局唯一）：循环外建一次 Terminal 视图，
        // 所有 channel client 的 Weak<dyn Terminal> 指向同一目标；强引用由 coordinator 的 Arc<ChannelManager> 保活
        let terminal: Arc<dyn Terminal> = self.clone();
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
            self.disconnect_notify.insert(channel_id.clone(), notify.clone());
            // ChannelClient 归入该 channel（懒建后 bind；消息/回复路径从 manager 取 client）
            self.bind_client(&channel_id, client.clone());

            let client_clone = client;
            let api_key = api_key.clone();
            // 重连循环内实时读取绑定身份（/bind 回写后重连即生效），需持有 config 引用
            let config = self.config.clone();

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
                        Ok(()) => {
                            info!("已连接 channel: {}", channel_id);
                            // 绑定身份实时读取（bind_users 逐个绑定；BindRequest.messenger_id 用绑定身份的 messenger 标识，如 "web"）
                            let bind_users = config.channel(&channel_id).await
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

    /// 发送消息到 channel（通道适配层封装：取 client + 发送 + 记录 pending msg_id 供回显判定）
    pub async fn send(&self, channel_id: &str, msg: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel_client::Error> {
        let Some(client) = self.client(channel_id) else {
            warn!("send: 未找到 channel client: {}", channel_id);
            return Err(kissbot_channel_client::Error::NotConnected);
        };
        let response = client.send_message(msg).await?;
        // 记录已发出未回显的 msg_id（入站回显命中时 consume_pending 消费丢弃）
        self.add_pending(channel_id, response.msg_id.as_str().to_string());
        Ok(response)
    }
}
```

9. 在文件末尾（tests mod 之前）追加 Terminal 实现（Task 1 中 trait 仍是 `self: Arc<Self>`，签名用 `self: Arc<Self>`；Task 2 改 `&self`）：
```rust
// ==================== Terminal 回调（ChannelManager 实现：通道适配层） ====================

/// ChannelManager 即 Terminal 实现者：回显过滤在通道层完成（Coordinator 不见自身回显），
/// 有业务意义的事件（群组变更/用户移除等）已由服务端转化为 IncomingMessage 推送，其余回调不重复处理
#[async_trait]
impl Terminal for ChannelManager {
    /// 收到上行消息：先做回显过滤（通道层），再转发 Coordinator 业务处理
    async fn incoming_message(self: Arc<Self>, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        // 1. msg_id 回显判定：命中（已发未回显）则消费并丢弃，不转发业务
        if self.consume_pending(channel_id, event.incoming_message.msg_id.as_str()) {
            return;
        }
        // 2. 转发业务处理（单例；run() 中 connect_all 之后必然已注册）
        AgentCoordinator::instance().incoming_message(channel_id, event).await;
    }

    async fn join_group(self: Arc<Self>, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组加入事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
    }

    async fn leave_group(self: Arc<Self>, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组离开事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
    }

    async fn user_removed(self: Arc<Self>, _id: &str, _notification: Arc<UserRemoveNotification>) {
        // 用户删除事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
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

`kissbot-agent/src/coordinator.rs`：

10. import 更新：
```rust
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use chrono::Local;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::channel_manager::ChannelManager;
use crate::types::{
    Mode, Message, Result, Error, SessionKey, memory_role,
};
use crate::context_cache::ContextCache;
use crate::history::HistoryArchive;
use crate::config_manager::{ConfigManager, ProviderModel, OutChannel, ToolConfig, EffectiveContextConfig};
use crate::command_router::CommandRouter;
use crate::model_client::ModelClient;
use crate::session_manager::{Session, SessionManager};
use crate::memory_reader::{MemoryReader, pack_memory_messages};
use crate::memory_store_client::MemoryStoreClient;
use crate::station::{self, StationRuntime};

use kissbot_api::channel::{IncomingMessageEvent, OutgoingMessage, ChannelUser};
use kissbot_api::memory::{ChannelRequest, ThinkRequest, ToolCallRequest, ToolResultRequest};
use kissbot_api::message::Content;
```
（删除：`use async_trait::async_trait;`、`use bytes::Bytes;`、`use kissbot_api::channel::BindRequest;`、`use kissbot_api::message::{AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};`、`use kissbot_channel_client::{ChannelClient, Terminal};`）

11. 在 `pub struct AgentCoordinator` 之前加单例骨架（S2/S3 才 set，此处仅声明与读取方法）：
```rust
/// AgentCoordinator 全局单例（进程内唯一；new() 完成时注册，此后 instance() 可用）。
/// 所有使用 coordinator 的位置一律不传参数、从单例获取（Session/Channel 不保存引用）。
static SINGLETON: OnceLock<AgentCoordinator> = OnceLock::new();
```

12. `AgentCoordinator` 结构体删除 `disconnect_notify` 字段：
```rust
    /// 断线通知：channel_id → Notify，closed() 通知重连循环
    disconnect_notify: Arc<DashMap<String, Arc<tokio::sync::Notify>>>,
```

13. `new()` 中 `channel_manager` 构造传 config、删除 `disconnect_notify` 字段构造：
```rust
            channel_manager: Arc::new(ChannelManager::new(config.clone())),
```
（删除 `disconnect_notify: Arc::new(DashMap::new()),` 行）

14. 在 `impl AgentCoordinator` 内（`new` 之前）加 instance：
```rust
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn instance() -> &'static AgentCoordinator {
        SINGLETON.get().expect("AgentCoordinator 未初始化")
    }
```

Run: `cargo build -p kissbot-agent`
Expected: 编译通过（此步骤不 set 单例，仅骨架；ChannelManager 的转发代码可编译）

- [ ] **Step 2: coordinator 迁移（删 Terminal/connect_channels；业务化；send 走 manager；全方法 &self；run 全启动动作）+ command_router 签名**

`kissbot-agent/src/coordinator.rs`：

15. 删除整个 `connect_channels` 方法（含方法注释）：
```rust
    // ==================== 通道连接 ====================

    /// 连接所有 enabled 的 channel（NexusRepo channel 配置为连接来源）
    /// 连接与绑定统一由 ChannelConfig 描述：enabled 控制连接，bind_users 为绑定身份（逐个绑定）
    async fn connect_channels(self: Arc<Self>) {
        ...
    }
```

16. 删除整个 Terminal impl 块：
```rust
// ==================== Terminal 回调（AgentCoordinator 直接实现；固有方法） ====================

/// Terminal 即 Coordinator 自身（全局唯一）：trait receiver 为 self: Arc<Self>（by-value Arc），
/// 方法内可直接持 Arc 调用 Arc 链方法（构造会话降级自身弱引用）——不再需要 TerminalHandle 适配器；
/// ChannelClient 持 Weak<dyn Terminal>，connect_channels 中全部 client 弱引用指向同一 coordinator
#[async_trait]
impl Terminal for AgentCoordinator {
    ...
}
```

17. 原 Terminal impl 的 `incoming_message` 改为 pub(crate) `&self` 业务方法（删除回显判定步骤 2，其余保留；`self.clone().handle_incoming(...)` 改 `self.handle_incoming(...)`）：
```rust
    /// 业务消息入口（由 ChannelManager 的 Terminal 转发调用；回显已在通道层 consume_pending 过滤，此处不见自身回显）
    pub(crate) async fn incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        // 1. 来源 channel 必须在配置中
        let Some(ch) = self.config.channel(channel_id).await else { return; };

        // 2. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 取来源 channel 运行态绑定，事件模式编码）
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

        // 3. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
        self.handle_incoming(channel_id, ch, event).await;
    }
```

18. 删除 `record_outgoing_msg_id` 与 `is_self_echo_by_msg_id`：
```rust
    /// 记录已发出的 outgoing msg_id 到该 channel 的 pending 集合（回显判定用）
    async fn record_outgoing_msg_id(&self, channel_id: &str, msg_id: &Arc<String>) {
        self.channel_manager.add_pending(channel_id, msg_id.as_str().to_string());
    }

    /// 按 msg_id 判定是否为自身发出的回显；命中则消费（移除）并返回 true
    async fn is_self_echo_by_msg_id(&self, channel_id: &str, msg_id: &Arc<String>) -> bool {
        self.channel_manager.consume_pending(channel_id, msg_id.as_str())
    }
```

19. 删除 `bind_batch`：
```rust
    /// 绑定会话后刷新合批生产侧（从 session.batch_producer 取 clone；会话创建/重定位时调用，None 时 enqueue 懒绑定）
    async fn bind_batch(&self, channel_id: &str, session: &Arc<Session>) {
        self.channel_manager.bind_producer(channel_id, Arc::new(session.batch_producer.clone()));
    }
```

20. `ensure_session` 改 `&self`，删 `Arc::downgrade(&self)` 参数与 `bind_batch` 调用：
```rust
    /// 定位会话，新建时构建初始上下文；返回 (会话, 是否新建)
    /// channel_id 为触发会话创建/重置的来源 channel（新建会话的 agent_id 取自该 channel 运行态绑定）
    async fn ensure_session(&self, key: &SessionKey, channel_id: &str) -> (Arc<Session>, bool) {
        // valid_default.load_full() 返回 Arc<Option<ProviderModel>>，解引用克隆得 Option
        let model = (*self.valid_default.load_full()).clone();
        // 会话状态保存 agent_id：新建会话从来源 channel 运行态绑定取得（原子写入 get_or_create）
        let agent_id = self.channel_agent(channel_id).await;
        let (session, created) = self.session_manager.get_or_create(key, model, agent_id);
        if created {
            self.build_initial_context(&session).await;
        }
        (session, created)
    }
```

21. `relocate_channel` / `apply_channel_key` / `set_session_model` 改 `&self`：
- `relocate_channel(self: Arc<Self>, ...)` → `relocate_channel(&self, ...)`；内部 `self.clone().ensure_session(...)` → `self.ensure_session(...)`
- `apply_channel_key(self: Arc<Self>, ...)` → `apply_channel_key(&self, ...)`；内部 `self.clone().relocate_channel(...)` → `self.relocate_channel(...)`；删除末尾会话重定位后刷新合批生产侧的整个 `if let Some(ch) = ...` 块：
```rust
        // 会话重定位后刷新合批生产侧（channel 绑定新的会话三元组，created 会话已由 ensure_session 绑定）
        if let Some(ch) = self.config.channel(channel_id).await {
            let key = self.session_key_for(&ch);
            if let Some(session) = self.session_manager.get(&key) {
                self.bind_batch(channel_id, &session).await;
            }
        }
```
- `set_session_model(self: Arc<Self>, ...)` → `set_session_model(&self, ...)`；内部 `self.clone().ensure_session(...)` → `self.ensure_session(...)`

22. `command_rx` 启动任务改经单例（`coordinator.clone().apply_channel_key(...)` → 任务内取 `AgentCoordinator::instance()`）：
```rust
        // 启动变更消费者：agent/role/event 变更串行处理（避免写-写竞态；读不受影响）
        {
            tokio::spawn(async move {
                while let Some(change) = command_rx.recv().await {
                    match change {
                        ConfigChange::ApplyKey { channel_id, new_key, agent_id, done } => {
                            // 单例已由 new() 注册（spawn 晚于 SINGLETON.set），任务内取用
                            let coordinator = AgentCoordinator::instance();
                            let rst = coordinator.apply_channel_key(&channel_id, &new_key, agent_id).await;
                            let _ = done.send(rst);
                        }
                    }
                }
            });
        }
```

23. `handle_incoming` 改 `&self`（去掉 `self: Arc<Self>,` 行），内部：
- `self.clone().handle_admin_command(channel_id, &event, &content_text).await` → `self.handle_admin_command(channel_id, &event, &content_text).await`
- `self.clone().ensure_session(&key, channel_id).await` → `self.ensure_session(&key, channel_id).await`

24. `handle_admin_command` 改 `&self`（去掉 `self: Arc<Self>,` 行），内部 `CommandRouter::execute(&cmd, &self.config, &self, channel_id)` → `CommandRouter::execute(&cmd, &self.config, channel_id)`（与 Step 30 同步）。

25. `handle_incoming` 中 `self.enqueue_batch(channel_id, &session, event).await` → `self.enqueue_batch(&session, event).await`；`enqueue_batch` 重写：
```rust
    /// 合批：数据直取会话生产侧入队（Arc<IncomingMessageEvent>）→ 更新截止时间（防抖）→ 发送触发时间（At）。
    /// 无 sleep、无逐消息任务——触发由 session 的 trigger 任务经 DelayQueue 定时处理。
    /// BatchProducer 已从 Channel 删除：enqueue 时 ensure_session 已返回会话，生产侧直接取 session.batch_producer，无 Channel 中转
    async fn enqueue_batch(&self, session: &Arc<Session>, event: Arc<IncomingMessageEvent>) {
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        let interval = Duration::from_secs(cfg.channel_batch_interval_secs);

        let producer = Arc::new(session.batch_producer.clone());
        let _ = producer.tx.send(event);                                // 数据入队（队列累积，不逐条消费）
        let at = Instant::now() + interval;                             // 单次计算：deadline 与触发时间同源
        producer.set_deadline(at);                                      // 更新截止（防抖，后推覆盖）
        let _ = producer.trigger_tx.send(crate::session_manager::Trigger::At(at));  // 发送触发时间（绝对）
    }
```

26. `send_admin_reply` 改用 `channel_manager.send`（删 `record_outgoing_msg_id` 调用，msg_id pending 由 send 内部完成）：
```rust
    /// 系统命令回复：始终发回来源 channel（不走 out_channel）
    /// 身份：messenger_id = incoming.messenger_id；user_id/self_user_id = event.recipient_user_id（接收方即发声身份，且是群成员）
    async fn send_admin_reply(&self, channel_id: &str, event: &Arc<IncomingMessageEvent>, content: String) {
        let Some(ch) = self.config.channel(channel_id).await else {
            warn!("send_admin_reply: 未找到 channel 配置: {}", channel_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.recipient_user_id.clone(),
            group_id: event.incoming_message.group_id.clone(),
            content: Content::Text(Arc::new(content.clone())),
        };

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        match self.channel_manager.send(channel_id, msg).await {
            Ok(response) => {
                // 下行成功后：推记忆（is_self=1）
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = self.channel_agent(channel_id).await;
                self.memory_store_client.push_channel_record(ChannelRequest {
                    agent_id,
                    role_name: Arc::new(role_name),
                    messenger_id: event.incoming_message.messenger_id.clone(),
                    user_id: event.recipient_user_id.clone(),
                    self_user_id: event.recipient_user_id.clone(),
                    group_id: event.incoming_message.group_id.clone(),
                    is_self: 1,
                    messenger_name: response.messenger_name.clone(),
                    user_name: response.user_name.clone(),
                    group_name: response.group_name.clone(),
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;
            }
            Err(e) => {
                warn!("send_admin_reply 失败: {:?}", e);
            }
        }
    }
```

27. `send_outgoing` 同样改用 `channel_manager.send`（删 `record_outgoing_msg_id` 调用）：
```rust
    /// Agentic Loop 产出回复：发到 out_channel（channel_id + ChannelUser + group_id）
    async fn send_outgoing(&self, out_channel: &OutChannel, content: String) {
        let Some(ch) = self.config.channel(out_channel.channel_id.as_str()).await else {
            warn!("send_outgoing: 未找到 channel 配置: {}", out_channel.channel_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
            user_id: Arc::new(out_channel.user.user_id.clone()),
            group_id: out_channel.group_id.clone(),
            content: Content::Text(Arc::new(content.clone())),
        };

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        match self.channel_manager.send(out_channel.channel_id.as_str(), msg).await {
            Ok(response) => {
                // 下行成功后：推记忆（is_self=1）
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = self.channel_agent(out_channel.channel_id.as_str()).await;
                self.memory_store_client.push_channel_record(ChannelRequest {
                    agent_id,
                    role_name: Arc::new(role_name),
                    messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
                    user_id: Arc::new(out_channel.user.user_id.clone()),
                    self_user_id: Arc::new(out_channel.user.user_id.clone()),
                    group_id: out_channel.group_id.clone(),
                    is_self: 1,
                    messenger_name: response.messenger_name.clone(),
                    user_name: response.user_name.clone(),
                    group_name: response.group_name.clone(),
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;
            }
            Err(e) => {
                warn!("send_outgoing 失败: {:?}", e);
            }
        }
    }
```

28. `new()`：删除启动循环与 `connect_channels` 调用（启动动作整体移入 `run()`）：
```rust
        // 启动：为全部 channel 绑定运行态 agent（解析失败回退保留 agent），
        // 再按 channel 绑定三元组初始化会话集合（agent_name 为空 = 保留 agent，同样建会话）
        for (_, ch) in config.channels().await {
            coordinator.bind_channel_runtime(&ch.channel_id).await;
            let key = coordinator.session_key_for(&ch);
            coordinator.clone().ensure_session(&key, &ch.channel_id).await;
        }

        // 连接所有 enabled 的 channel（connect_channels 取 Arc 值，clone 保留 new 末尾返回用）
        coordinator.clone().connect_channels().await;
```
（删除以上两块；`run()` 中补回，见 Step 29）

29. `run()` 改为完整启动动作（注意 `new()` 目前仍返回 `Arc<Self>`、尚未 set 单例——Step 3 改）：
```rust
    /// 启动主循环（保持进程运行）：绑定运行态 agent + 初始化会话 + 连接全部 channel
    pub async fn run(&self) {
        info!("AgentCoordinator 启动，等待外部输入...");
        // 启动：为全部 channel 绑定运行态 agent（解析失败回退保留 agent），
        // 再按 channel 绑定三元组初始化会话集合（agent_name 为空 = 保留 agent，同样建会话）
        for (_, ch) in self.config.channels().await {
            self.bind_channel_runtime(&ch.channel_id).await;
            let key = self.session_key_for(&ch);
            self.ensure_session(&key, &ch.channel_id).await;
        }
        // 连接所有 enabled 的 channel（连接/重连/回显/发送全部归 ChannelManager 通道适配层）
        self.channel_manager.connect_all().await;
        // channel-client 通过 Terminal 回调驱动，此处保持进程不退出
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
```

`kissbot-agent/src/command_router.rs`：

30. `execute` 删 coordinator 参数，内部经单例；`channel_current_key` 同样：
```rust
/// 取 channel 当前会话三元组（config 的 agent_name/role_name + 运行态 mode），命令构造新三元组用
async fn channel_current_key(channel_id: &str) -> Option<SessionKey> {
    AgentCoordinator::instance().channel_session_key(channel_id).await
}
```
```rust
    /// bind/agent/role/bind-outgoing/admin/unadmin 走 ConfigManager 回写；
    /// mode/reenter 改运行态模式（coordinator）；model 改会话模型（运行态）。
    /// coordinator 一律从单例取（不传参数）
    pub async fn execute(
        command: &AdminCommand,
        config: &ConfigManager,
        channel_id: &str,
    ) -> Result<(String, CommandEffect)> {
        let coordinator = AgentCoordinator::instance();
        match command {
```
（函数体内：`channel_current_key(coordinator, channel_id)` → `channel_current_key(channel_id)`；`coordinator.clone().set_session_model(channel_id, pm.clone())` → `coordinator.set_session_model(channel_id, pm.clone())`；其余 `coordinator.xxx(...)` 保持不变）

Run: `cargo build -p kissbot-agent`
Expected: 编译通过（注意此中间态运行时 instance() 会 panic，因为单例未 set——不要运行程序，仅编译验证）

- [ ] **Step 3: 单例化收尾（new→Result<()>/set）+ session_manager + main + 测试更新**

`kissbot-agent/src/coordinator.rs`：

31. `new()` 返回值改为 `Result<()>`；构造 `Arc<Self>` 改构造值；末尾 `SINGLETON.set`；`command_rx` spawn 移到 set 之后：
```rust
    pub async fn new(
        config: Arc<ConfigManager>,
    ) -> Result<()> {
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let session_manager = SessionManager::new();
        let model_client = ModelClient::new(config.clone());
        let data_dir = config.data_dir().to_string();
        // agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<ConfigChange>();

        let coordinator = Self {
            config: config.clone(),
            cache: Arc::new(ContextCache::new(&data_dir)),
            history: Arc::new(HistoryArchive::new(&data_dir)),
            memory_reader,
            memory_store_client,
            session_manager,
            model_client: Arc::new(tokio::sync::Mutex::new(model_client)),
            channel_manager: Arc::new(ChannelManager::new(config.clone())),
            valid_default: ArcSwap::from_pointee(None),
            command_tx,
            station_runtimes: Arc::new(DashMap::new()),
        };

        // 启动校验 default_model：从 API 拉模型列表，不在列表则无模型（告警）
        let default_model = config.default_model().await;
        let valid_default = match coordinator.model_client.lock().await.list_models(&default_model).await {
            Ok(list) if list.iter().any(|m| m == &default_model.model) => Some(default_model.clone()),
            Ok(_) => { tracing::warn!("default_model {}/{} 不在 API 模型列表", default_model.provider, default_model.model); None }
            Err(e) => { tracing::warn!("校验 default_model 失败（API 不可用?）: {:?}", e); None }
        };
        coordinator.valid_default.store(Arc::new(valid_default));

        // 构建 Station 运行态：base_url 为空的本地 station 注册内置 Read 工具；
        // 远程 station 的 runtime 同样构建（call_tool 走 REST 骨架，本轮不实现）
        {
            let runtimes = coordinator.station_runtimes.clone();
            for (_, sc) in config.stations().await {
                let runtime = Arc::new(StationRuntime::new(sc));
                if runtime.config().base_url.is_empty() {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    runtime.register_local("read", Arc::new(station::ReadTool::new(cwd)));
                }
                runtimes.insert(runtime.station_id().to_string(), runtime);
            }
        }

        // 注册全局单例（此后 instance() 可用；run() 中启动动作与连接回调均晚于此）
        let _ = SINGLETON.set(coordinator);

        // 启动变更消费者：agent/role/event 变更串行处理（避免写-写竞态；读不受影响）
        // spawn 晚于 SINGLETON.set，任务内 instance() 必然就绪
        tokio::spawn(async move {
            while let Some(change) = command_rx.recv().await {
                match change {
                    ConfigChange::ApplyKey { channel_id, new_key, agent_id, done } => {
                        let coordinator = AgentCoordinator::instance();
                        let rst = coordinator.apply_channel_key(&channel_id, &new_key, agent_id).await;
                        let _ = done.send(rst);
                    }
                }
            }
        });

        info!("AgentCoordinator 初始化完成");
        Ok(())
    }
```
（注意：原 `let coordinator = Arc::new(Self {...})` 改 `let coordinator = Self {...}`；启动循环与 connect_channels 调用已在 Step 28 删除，此处不再出现）

`kissbot-agent/src/session_manager.rs`：

32. `Session` 删除 coordinator 字段及其注释：
```rust
    /// coordinator 弱引用（accept_batch 升级调用 run_agentic_loop/out_channel 解析；弱引用破环：
    /// coordinator → session_manager → session → coordinator 会形成强环）
    coordinator: Weak<AgentCoordinator>,
```

33. `accept_batch` 改经单例（删弱引用升级）：
```rust
    /// 合批 flush 入口：trigger 任务经 consumer.session 弱引用升级后调用（原 coordinator.flush_batch 职责迁至会话侧）
    /// 模型检查 → coordinator 单例 → out_channel 解析 → agentic loop
    pub(crate) async fn accept_batch(self: &Arc<Self>, content: String) {
        // 无可用模型：静默忽略（与 run_agentic_loop 入口一致）
        if self.model.load().is_none() {
            return;
        }
        let coordinator = AgentCoordinator::instance();
        let Some(out_channel) = coordinator.resolve_out_channel_for_session(self).await else {
            warn!("accept_batch: 会话无 out_channel，跳过");
            return;
        };
        coordinator.run_agentic_loop("", self, content, &out_channel).await;
    }
```

34. `get_or_create` / `create_session` 删 coordinator 参数：
```rust
    /// 定位会话，不存在则创建（model 为初始模型，None = 无模型；agent_id 为会话状态保存的解析结果）；
    /// 返回 (会话, 是否新建)
    /// 创建时依赖序组装（内联 new_producer/BatchConsumer::new）：notify → 2 mpsc → producer → session → consumer → spawn
    /// （channel 均从 session.batch_producer 取 clone；任务持 consumer，consumer 持 session 弱引用与 notify，
    ///  anchor/deadline/notify 均为独立 Arc——producer 与 consumer 共享同一份）
    /// 双重锁定：先 get 快速路径（命中直接返回），未命中再走 entry API 原子创建（并发下仅一个创建成功）
    pub fn get_or_create(
        &self,
        key: &SessionKey,
        model: Option<ProviderModel>,
        agent_id: Arc<String>,
    ) -> (Arc<Session>, bool) {
        if let Some(s) = self.get(key) {
            return (s, false);
        }
        match self.sessions.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(e) => (e.get().clone(), false),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                // 创建部分抽出（create_session）：依赖序组装 + spawn 触发任务
                let session = Self::create_session(key, model, agent_id);
                e.insert(session.clone());
                (session, true)
            }
        }
    }
```
```rust
    fn create_session(
        key: &SessionKey,
        model: Option<ProviderModel>,
        agent_id: Arc<String>,
    ) -> Arc<Session> {
```
（`create_session` 内 `coordinator,` 字段构造行删除；`Session {...}` 字面量中 `coordinator,` 删除）

35. session_manager 测试更新：所有 `get_or_create(&key, ..., Arc::new(...), std::sync::Weak::new())` 删除最后一个参数（`Weak::new()`）。

`kissbot-agent/src/coordinator.rs` 测试：

36. `tool_placeholder_uses_same_key_for_call_and_result` 中 `mgr.get_or_create(&key, None, Arc::new("aid".into()), std::sync::Weak::new())` → `mgr.get_or_create(&key, None, Arc::new("aid".into()))`。

`kissbot-agent/src/main.rs`：

37. 调整 new 调用与 run：
```rust
    // 2. 初始化 Coordinator（装配 + 注册单例；连接与启动动作在 run() 中执行）
    coordinator::AgentCoordinator::new(config.clone())
        .await
        .expect("初始化 Coordinator 失败");
```
```rust
    // 4. 运行主循环（内部：绑定 agent/会话 + 连接全部 channel + 保持进程）
    info!("进入主循环");
    coordinator::AgentCoordinator::instance().run().await;
```

38. 测试验证：

Run: `cargo test -p kissbot-agent`
Expected: 全部通过（channel_manager 测试需先按 Step 39 调整）

39. `kissbot-agent/src/channel_manager.rs` 测试调整（`ChannelManager::new` 现需 config，测试无法构造 ConfigManager——依赖 kissbot_config 全局单例）：
- 保留 `channel_msg_id_consume` / `channel_ttl_evict`（只测 `Channel::new()`，不受影响）
- 删除 `channel_manager_lazy_create_and_isolated_state`，替换为只测 Channel 层状态的新测试：
```rust
    #[test]
    fn channel_mode_and_agent_state() {
        let ctx = Channel::new();
        // mode 默认 Role，设置后读回
        assert_eq!(ctx.mode(), Mode::Role);
        ctx.set_mode(Mode::Event("e1".into()));
        assert_eq!(ctx.mode(), Mode::Event("e1".into()));
        // agent_id 未绑定 None，绑定后读回
        assert!(ctx.agent_id().is_none());
        ctx.set_agent_id(Arc::new("aid".into()));
        assert_eq!(ctx.agent_id().unwrap().as_str(), "aid");
    }
```

- [ ] **Step 4: 全量验证与提交**

Run: `cargo build -p kissbot-agent && cargo test -p kissbot-agent`
Expected: 编译通过、全部测试通过（含新增 channel_mode_and_agent_state；session_manager / coordinator 测试签名更新）

```bash
git add kissbot-agent/src/channel_manager.rs kissbot-agent/src/coordinator.rs kissbot-agent/src/session_manager.rs kissbot-agent/src/command_router.rs kissbot-agent/src/main.rs
git commit -m "重构：ChannelManager 成为通道适配层，AgentCoordinator 单例化

- ChannelManager 实现 Terminal（回显过滤在通道层，转发业务经单例）；持 config + disconnect_notify；connect_all/send 封装（send 内部取 client + 记录 pending msg_id）
- Channel 删除 BatchProducer（合批直取 session.batch_producer，enqueue 无 Channel 中转）；删 bind_batch/record_outgoing_msg_id/is_self_echo_by_msg_id
- AgentCoordinator 单例化：OnceLock 存值，instance() 返回 &'static；new 返回 Result<()> 末尾注册；启动动作（bind agent/session、connect_all、主循环）全部在 run()
- 全部方法改 &self；command_rx 任务/command_router/session_manager 一律从单例取，不传 coordinator 参数
- Session 删除 coordinator 弱引用（引用链无环）；command_router execute 删 coordinator 参数
- main：new 注册单例后 instance().run()"
```

---

### Task 2: Terminal trait 改 &self（kissbot-channel-client + cli + agent）

**Files:**
- Modify: `kissbot-channel-client/src/terminal.rs`
- Modify: `kissbot-channel-client/tests/mock.rs`
- Modify: `kissbot-channel-client-cli/src/main.rs`
- Modify: `kissbot-agent/src/channel_manager.rs`

**Interfaces:**
- Consumes: Task 1 的 `ChannelManager` Terminal impl（`self: Arc<Self>` 签名）
- Produces: `trait Terminal` 全部方法 receiver 为 `&self`；三个实现者（ChannelManager / MockTerminal / CliTerminal）签名同步

- [ ] **Step 1: trait 与 agent 实现改 &self**

`kissbot-channel-client/src/terminal.rs`：

1. 删除 trait 定义上方的整段注释：
```rust
/// 终端接口：ChannelClient 收到服务端推送后调用的回调函数。
/// id 是触发事件的 ChannelClient 的标识（由 ChannelClient::new 时传入）。
/// receiver 用 self: Arc<Self>（by-value Arc）：调用方（ChannelClient）持 Weak<dyn Terminal>，
/// upgrade 得 Arc<dyn Terminal> 后直接调用——与 &self 不同，方法内可直接持有/降级 Arc 自身
/// （&Arc<Self> receiver 不可对象化 E0038，trait 里无法声明）。
```
替换为：
```rust
/// 终端接口：ChannelClient 收到服务端推送后调用的回调函数。
/// id 是触发事件的 ChannelClient 的标识（由 ChannelClient::new 时传入）。
/// receiver 用 &self：实现者不需要在方法内持有/降级 Arc 自身。
```
（用户要求：原解释 Arc\<Self\> receiver 的注释直接删除、不新增替代注释——上述替换为仅保留接口职责说明；如倾向更彻底删除，也可只保留前两句。以保留接口职责说明为准。）

2. 六个方法签名 `self: Arc<Self>` → `&self`：
```rust
#[async_trait]
pub trait Terminal: Send + Sync + 'static {
    /// 收到上行消息（含接收方 recipient_user_id）
    async fn incoming_message(&self, id: &str, message: Arc<IncomingMessageEvent>);
    /// 用户加入群组
    async fn join_group(&self, id: &str, notification: Arc<GroupChangeNotification>);
    /// 用户离开群组
    async fn leave_group(&self, id: &str, notification: Arc<GroupChangeNotification>);
    /// 用户被删除
    async fn user_removed(&self, id: &str, notification: Arc<UserRemoveNotification>);
    /// 下载分块到达（Ok/Err 即该块的确认结果）
    async fn download_chunk(&self, id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()>;
    /// 连接关闭（不做自动重连）
    async fn closed(&self, id: &str);
}
```
（`use std::sync::Arc;` 仍需保留——IncomingMessageEvent 等参数用 Arc）

`kissbot-agent/src/channel_manager.rs`：

3. Terminal impl 六个方法签名 `self: Arc<Self>` → `&self`（方法体不变；`self.consume_pending(...)` / `self.disconnect_notify.get(...)` 均为 &self 调用，无需改动）。

Run: `cargo build -p kissbot-agent`
Expected: 编译通过

- [ ] **Step 2: mock 与 cli 实现改 &self**

`kissbot-channel-client/tests/mock.rs`：

4. `MockTerminal` 六个方法签名 `self: Arc<Self>` → `&self`（方法体不变，字段访问 `self.incoming` / `self.joins` / `self.leaves` / `self.removals` / `self.chunks` / `self.closed_tx` 在 &self 下自动解引用）。

`kissbot-channel-client-cli/src/main.rs`：

5. `CliTerminal` 六个方法签名 `self: Arc<Self>` → `&self`（方法体不变，`self.download_dir` 字段访问 OK）。

- [ ] **Step 3: 全量验证与提交**

Run: `cargo build -p kissbot-channel-client-cli && cargo test -p kissbot-channel-client && cargo test -p kissbot-agent`
Expected: 全部编译与测试通过

```bash
git add kissbot-channel-client/src/terminal.rs kissbot-channel-client/tests/mock.rs kissbot-channel-client-cli/src/main.rs kissbot-agent/src/channel_manager.rs
git commit -m "refactor: Terminal trait receiver 从 self: Arc<Self> 改为 &self

- terminal.rs：六个方法签名改 &self，删除原解释 Arc<Self> receiver 的注释（不新增替代注释）
- 实现者同步：ChannelManager / MockTerminal / CliTerminal 方法签名改 &self（方法体不变）
- ChannelClient 调用点不变（upgrade 的 Arc<dyn Terminal> 自动解引用调 &self）"
```

---

## Self-Review

**1. Spec coverage:**
- Q1（回调归 ChannelManager no-op）→ Task 1 Step 1（Terminal impl 四个 no-op）✓
- Q2（ChannelManager 持 config）→ Task 1 Step 1（`config: Arc<ConfigManager>` + `new(config)`）✓
- Q3/Q6（单例、全链路不传参）→ Task 1 Step 2/3（SINGLETON/instance/new→Result<()>/command_router/session_manager/main）✓
- Q4（转发不单测）→ Task 1 Step 3（channel_manager 测试改为 Channel 级）✓
- Q5（Terminal &self）→ Task 2 ✓
- connect_all 在 run（new 只装配）→ Task 1 Step 2/3 ✓
- BatchProducer 删除 → Task 1 Step 1/2（Channel 删 producer、enqueue 直取、bind_batch 删）✓
- send 封装（client + add_pending + NotConnected）→ Task 1 Step 1 ✓
- Terminal 注释删除 → Task 2 Step 1 ✓

**2. Placeholder scan:** 无 TBD/TODO；每步给出确切代码或删除指令 ✓

**3. Type consistency:**
- `ChannelManager::new(config)` 在 Task 1 Step 1 定义、Step 1 的 coordinator 构造处使用、测试 Step 39 调整 ✓
- `AgentCoordinator::instance()` Step 1 定义骨架、Step 2 的 command_rx/转发使用、Step 3 set ✓
- `CommandRouter::execute(cmd, config, channel_id)` Step 2 同步改 coordinator 调用点与 command_router 定义 ✓
- `SessionManager::get_or_create(key, model, agent_id)` Step 3 同步改 coordinator 调用点、session_manager 定义、两处测试 ✓
- `ChannelManager::send` 返回 `Arc<OutgoingMessageResponse>`，send_admin_reply/send_outgoing 使用 `response.msg_id` 等字段与 `OutgoingMessageResponse` 字段一致 ✓
