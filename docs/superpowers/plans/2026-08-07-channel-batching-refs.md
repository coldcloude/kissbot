# Channel 合批引用重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将合批的 session/coordinator/producer 引用改为依赖序构造（notify+mpsc → producer → session → consumer → spawn），删除全部 OnceLock 升级槽与 weak_self，session 持 producer、consumer 持 session 弱引用，flush 走 `session.accept_batch`。

**Architecture:** 全链路 channel → producer → consumer → session → coordinator；`BatchProducer` 只剩 tx/trigger_tx/anchor/deadline（去槽去 notify）；`BatchConsumer` 增 session 弱引用 + notify（任务退出信号，与 Session.notify 同一 Arc）；`Session` 增 coordinator 弱引用 + notify，持 producer；`ChannelContext` 用 `ArcSwapOption<BatchProducer>` 无锁持 producer；coordinator 删 weak_self，Arc 链方法用 `self: &Arc<Self>`（trait 边界用 `TerminalHandle` 薄适配器）。

**Tech Stack:** Rust / tokio（mpsc, Notify, select!）/ tokio-util DelayQueue / futures-util StreamExt / arc-swap / dashmap / async-trait

## Global Constraints

- 不要删除代码中的注释（更新措辞而非删除）
- 读写文件必须用 Read/Edit/Write 工具，禁止 sed/python 修改文件
- 测试运行：`cd kissbot-agent && cargo test`（各 crate 独立，无根 workspace）；期望 111 passed、`cargo build` 0 warnings
- 提交 comment 用中文，覆盖本次改动全部内容
- 项目编码规范：非枚举/非 Map Key/非 Vec 字段用 `Arc<T>` 包裹
- spec 参照：`docs/superpowers/specs/2026-08-07-channel-batching-refs-design.md`

---
## 文件结构

- `kissbot-agent/src/batching.rs`：BatchProducer/BatchConsumer 结构调整、new_producer、BatchConsumer::new、try_flush 升级路径、spawn_trigger notify 源
- `kissbot-agent/src/session_manager.rs`：Session 新字段/构造、accept_batch、get_or_create 依赖序组装、Drop
- `kissbot-agent/src/coordinator.rs`：TerminalHandle 适配器、Arc 链、ChannelContext 无锁 producer、bind/enqueue、删 flush_batch/weak_self

### Task 1: Arc<Self> 链 + TerminalHandle 适配器（行为等价）

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`（impl Terminal 块、connect_channels、四个方法签名）

**Interfaces:**
- Consumes: 无（纯签名/接线重构）
- Produces: `TerminalHandle(Arc<AgentCoordinator>)`（impl Terminal）；`AgentCoordinator::incoming_message/handle_incoming/ensure_session/relocate_channel/apply_channel_key` 改 `self: &Arc<Self>`

> 背景：trait 方法 receiver 只能 `&self`（实测 `self: &Arc<Self>` 报 E0053 不兼容；trait 声明 `&Arc<Self>` 报 E0038 不可 dyn 分发）。`ensure_session` 需要降级自身弱引用传给 session，故 Arc 链必须拿到 `&Arc<Self>`——ChannelClient 只持 `Weak<dyn Terminal>`，消息路径用 `TerminalHandle` 薄适配器包住 `Arc<AgentCoordinator>`。

- [ ] **Step 1: 将 `impl Terminal for AgentCoordinator` 块改为 TerminalHandle 适配器 + 固有方法**

把 `kissbot-agent/src/coordinator.rs` 的 `impl Terminal for AgentCoordinator { ... }` 整块（约 733-796 行）替换为：

```rust
/// Terminal trait 适配器：ChannelClient 持 Weak<dyn Terminal> 回调（trait 方法 receiver 限 &self，
/// 无法用 self: &Arc<Self>——E0053/E0038）；适配器持 Arc<AgentCoordinator>，
/// 使消息路径可调用 coordinator 的 self: &Arc<Self> 方法（构造会话时降级自身弱引用）
struct TerminalHandle(Arc<AgentCoordinator>);

#[async_trait]
impl Terminal for TerminalHandle {
    /// 收到上行消息（event 含接收方 recipient_user_id）
    async fn incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        self.0.incoming_message(channel_id, event).await;
    }

    async fn join_group(&self, id: &str, notification: Arc<GroupChangeNotification>) {
        self.0.join_group(id, notification).await;
    }

    async fn leave_group(&self, id: &str, notification: Arc<GroupChangeNotification>) {
        self.0.leave_group(id, notification).await;
    }

    async fn user_removed(&self, id: &str, notification: Arc<UserRemoveNotification>) {
        self.0.user_removed(id, notification).await;
    }

    async fn download_chunk(&self, id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> std::result::Result<(), kissbot_channel_client::Error> {
        self.0.download_chunk(id, info, pos, data).await
    }

    async fn closed(&self, id: &str) {
        self.0.closed(id).await;
    }
}

// ==================== Terminal 固有方法（适配器委托入口） ====================

impl AgentCoordinator {
    /// 收到上行消息（event 含接收方 recipient_user_id；TerminalHandle 委托入口）
    async fn incoming_message(self: &Arc<Self>, channel_id: &str, event: Arc<IncomingMessageEvent>) {
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
        self.handle_incoming(channel_id, ch, event).await;
    }

    async fn join_group(&self, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组加入事件，当前暂不处理
    }

    async fn leave_group(&self, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组离开事件，当前暂不处理
    }

    async fn user_removed(&self, _id: &str, _notification: Arc<UserRemoveNotification>) {
        // 用户删除事件，当前暂不处理
    }

    async fn download_chunk(&self, _id: &str, _info: Arc<AttachmentInfoResponse>, _pos: u64, _data: Bytes) -> std::result::Result<(), kissbot_channel_client::Error> {
        // 当前未使用附件下载
        Ok(())
    }

    async fn closed(&self, id: &str) {
        info!("channel 连接关闭: {}，准备重连", id);
        // 通知重连循环
        if let Some(notify) = self.disconnect_notify.get(id) {
            notify.notify_one();
        }
    }
}
```

- [ ] **Step 2: 改 Arc 链方法签名为 `self: &Arc<Self>`**

`kissbot-agent/src/coordinator.rs` 四处方法签名：

- `async fn handle_incoming(\n        &self,\n        channel_id: &str,` → `async fn handle_incoming(\n        self: &Arc<Self>,\n        channel_id: &str,`
- `async fn relocate_channel(&self, channel_id: &str)` → `async fn relocate_channel(self: &Arc<Self>, channel_id: &str)`
- `async fn apply_channel_key(&self, channel_id: &str, new_key: &SessionKey, agent_id: Option<Arc<String>>) -> Result<()>` → `async fn apply_channel_key(self: &Arc<Self>, channel_id: &str, new_key: &SessionKey, agent_id: Option<Arc<String>>) -> Result<()>`
- `async fn ensure_session(&self, key: &SessionKey, channel_id: &str) -> (Arc<Session>, bool)` → `async fn ensure_session(self: &Arc<Self>, key: &SessionKey, channel_id: &str) -> (Arc<Session>, bool)`

方法体不变（`&Arc<Self>` 自动解引用访问字段/方法）。

- [ ] **Step 3: connect_channels 接线改为 TerminalHandle**

`connect_channels`（约 675-679 行）：

```rust
            let client = ChannelClient::new(
                channel_id.clone(),
                Arc::downgrade(&(coordinator.clone() as Arc<dyn Terminal>)),
            );
```

改为：

```rust
            // Terminal trait 回调限 &self receiver，用 TerminalHandle 适配器包 Arc<Self>（Arc 链方法入口）
            let terminal: Arc<dyn Terminal> = Arc::new(TerminalHandle(coordinator.clone()));
            let client = ChannelClient::new(channel_id.clone(), Arc::downgrade(&terminal));
```

- [ ] **Step 4: 构建并跑全量测试（行为等价确认）**

Run: `cd kissbot-agent && cargo build 2>&1 | grep -c warning; cargo test 2>&1 | grep "test result"`
Expected: build 0 warnings；test result: ok. 111 passed（无行为变化，TerminalHandle 仅委托）

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): Terminal 回调改 TerminalHandle 适配器 + Arc 链方法改 self: &Arc<Self>——trait receiver 限 &self（&Arc<Self> 报 E0053、trait 声明报 E0038 不可 dyn 分发），适配器持 Arc<AgentCoordinator> 供消息路径调用 Arc 链（incoming_message/handle_incoming/ensure_session/relocate_channel/apply_channel_key），为删 weak_self 铺路"
```

### Task 2: batching/session 结构调整 + accept_batch + 删槽删 weak_self

**Files:**
- Modify: `kissbot-agent/src/batching.rs`、`kissbot-agent/src/session_manager.rs`、`kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: Task 1 的 Arc 链方法签名
- Produces: `new_producer() -> (BatchProducer, UnboundedReceiver<BatchItem>, UnboundedReceiver<Trigger>)`；`BatchConsumer::new(rx, trigger_rx, session: &Arc<Session>, notify: Arc<Notify>)`；`Session::new(key, model, agent_id, coordinator: Weak<AgentCoordinator>, batch: BatchProducer, notify: Arc<Notify>)`；`Session::accept_batch(self: &Arc<Self>, content: String)`（pub(crate)）；`SessionManager::get_or_create(key, model, agent_id, coordinator: Weak<AgentCoordinator>) -> (Arc<Session>, bool)`；coordinator 删 weak_self；`run_agentic_loop`/`resolve_out_channel_for_session` 改 pub(crate)；删 `flush_batch`

- [ ] **Step 1: batching.rs 结构调整**

`kissbot-agent/src/batching.rs`：

1. 顶部 import 去掉 OnceLock：`use std::sync::{Arc, OnceLock, Weak};` → `use std::sync::{Arc, Weak};`

2. `BatchProducer` 删除三个字段（notify/coordinator/session），并更新注释：

```rust
/// 生产侧：Session 持有（直接值，可 Clone——字段均为 Clone/Arc 共享类型）；全无锁共享
#[derive(Clone)]
pub struct BatchProducer {
    pub tx: mpsc::UnboundedSender<BatchItem>,
    pub trigger_tx: mpsc::UnboundedSender<Trigger>,
    /// 编码基准：固定 Instant（所有 clone 共享）；u64 毫秒 = 相对此基准（参照 kai-ws WsHeartbeatHandler 的 anchor 方法）
    anchor: Arc<Instant>,
    /// 截止时间（u64 毫秒，相对 anchor；0 = 无待 flush（原 None）哨兵）——Arc<AtomicU64> 无锁共享
    pub deadline: Arc<AtomicU64>,
}
```

3. `BatchConsumer` 增 session 弱引用 + notify，字段改私有，新增构造器：

```rust
/// 消费侧：trigger 任务独占（随 spawn move 进任务，任务内 mut 访问，零锁）
/// 持 session 弱引用（flush 升级用；弱引用不强持会话——会话销毁由 session_manager/channel 决定）
/// 持 notify（任务 select 等待会话销毁通知；与 Session.notify 同一 Arc，见 get_or_create 组装）
pub struct BatchConsumer {
    rx: mpsc::UnboundedReceiver<BatchItem>,
    trigger_rx: mpsc::UnboundedReceiver<Trigger>,
    delay: DelayQueue<Trigger>,
    session: Weak<Session>,
    notify: Arc<Notify>,
}

impl BatchConsumer {
    /// 组装消费侧（依赖序：session 已建）；仅 session_manager.get_or_create 调用
    pub fn new(
        rx: mpsc::UnboundedReceiver<BatchItem>,
        trigger_rx: mpsc::UnboundedReceiver<Trigger>,
        session: &Arc<Session>,
        notify: Arc<Notify>,
    ) -> Self {
        Self {
            rx,
            trigger_rx,
            delay: DelayQueue::new(),
            session: Arc::downgrade(session),
            notify,
        }
    }
}
```

4. 删除 `new_batch()`，替换为 `new_producer()`：

```rust
/// 创建合批生产侧 + 原始接收端（无依赖：不涉及 session）；consumer 由 get_or_create 组装
/// （依赖序：notify + mpsc → producer → session → consumer，consumer 需 session 弱引用）
pub fn new_producer() -> (BatchProducer, mpsc::UnboundedReceiver<BatchItem>, mpsc::UnboundedReceiver<Trigger>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
    let producer = BatchProducer {
        tx,
        trigger_tx,
        anchor: Arc::new(Instant::now()),
        deadline: Arc::new(AtomicU64::new(0)),
    };
    (producer, rx, trigger_rx)
}
```

5. `spawn_trigger` 的 select 分支 `_ = producer.notify.notified() => break` 改为 `_ = consumer.notify.notified() => break`（注释更新为「会话销毁（session.notify notify_one）→ 退出」），并把函数头注释「随 Session::new 调用」改为「随 session_manager.get_or_create 调用」。

6. `try_flush` 升级路径改为 consumer.session 弱引用 + accept_batch：

```rust
    if items.is_empty() {
        return;
    }
    // 任务持 consumer.session 弱引用升级会话（失败 = 会话已销毁，数据已 drain 清走，仅丢弃打包内容）
    let Some(session) = consumer.session.upgrade() else {
        return;
    };
    let content = pack_events(&items);
    session.accept_batch(content).await;
```

（删除原 `producer.session.get()...` / `producer.coordinator.get()...` 双槽升级与 `c.flush_batch(&s, content)` 调用）

- [ ] **Step 2: session_manager.rs Session 结构/构造 + accept_batch + get_or_create 组装**

`kissbot-agent/src/session_manager.rs`：

1. 顶部 import 增补：

```rust
use std::collections::HashSet;
use std::sync::{Arc, Weak};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use log::warn;

use crate::config_manager::ProviderModel;
use crate::coordinator::AgentCoordinator;
use crate::types::{Message, Mode, SessionKey};
```

2. `Session` 结构增两个字段（batch 保留），`Session::new` 新签名，`Drop` 改 notify：

```rust
pub struct Session {
    pub agent_name: Arc<String>,    // 运行态：从 key 复制（context 配置查找用）
    pub role_name: Arc<String>,     // 运行态：从 key 复制（身份读取源；SessionKey 仅作去重，不存于 Session）
    pub mode: Arc<Mode>,            // 运行态：从 key 复制
    pub context: tokio::sync::Mutex<SessionContext>,
    /// 合批生产侧（依赖序构造时经 get_or_create 传入；channel 均从本字段取 producer 绑定）
    pub batch: crate::batching::BatchProducer,
    /// 会话级模型（创建时取 default_model，/model 调整）；None = 无模型（普通消息静默忽略）
    pub model: ArcSwap<Option<ProviderModel>>,
    /// 会话状态保存的 agent_id（UUID；创建时取自触发 channel 的运行态绑定，之后不变）
    /// 取记忆/ego 一律用本字段（agent_name 仅作 context 配置查找，不参与记忆/ego 定位）
    pub agent_id: Arc<String>,
    /// coordinator 弱引用（accept_batch 升级调用 run_agentic_loop/out_channel 解析；弱引用破环：
    /// coordinator → session_manager → session → coordinator 会形成强环）
    coordinator: Weak<AgentCoordinator>,
    /// 会话销毁通知（Drop 时 notify_one → trigger 任务退出；与 consumer.notify 同一 Arc）
    pub notify: Arc<tokio::sync::Notify>,
}

impl Session {
    pub fn new(
        key: &SessionKey,
        model: Option<ProviderModel>,
        agent_id: Arc<String>,
        coordinator: Weak<AgentCoordinator>,
        batch: crate::batching::BatchProducer,
        notify: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new()),
            batch,
            model: ArcSwap::from_pointee(model),
            agent_id,
            coordinator,
            notify,
        }
    }

    /// 合批 flush 入口：trigger 任务经 consumer.session 弱引用升级后调用（原 coordinator.flush_batch 职责迁至会话侧）
    /// 模型检查 → coordinator 弱引用升级 → out_channel 解析 → agentic loop
    pub(crate) async fn accept_batch(self: &Arc<Self>, content: String) {
        // 无可用模型：静默忽略（与 run_agentic_loop 入口一致）
        if self.model.load().is_none() {
            return;
        }
        let Some(coordinator) = self.coordinator.upgrade() else {
            return;
        };
        let Some(out_channel) = coordinator.resolve_out_channel_for_session(self).await else {
            warn!("accept_batch: 会话无 out_channel，跳过");
            return;
        };
        coordinator.run_agentic_loop("", self, content, &out_channel).await;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // 会话销毁：通知 trigger 任务退出（notify_one permit 语义：任务错过唤醒后下一轮 notified 立即完成）
        self.notify.notify_one();
    }
}
```

3. `get_or_create` 新签名 + 依赖序组装：

```rust
    /// 定位会话，不存在则创建（model 为初始模型，None = 无模型；agent_id 为会话状态保存的解析结果；
    /// coordinator 弱引用由 Arc 链调用方降级传入）；返回 (会话, 是否新建)
    /// 创建时依赖序组装：notify → 2 mpsc → producer → session → consumer → spawn
    /// （channel 均从 session.batch 取 producer；任务持 consumer，consumer 持 session 弱引用与 notify）
    /// 双重锁定：先 get 快速路径（命中直接返回），未命中再走 entry API 原子创建（并发下仅一个创建成功）
    pub fn get_or_create(
        &self,
        key: &SessionKey,
        model: Option<ProviderModel>,
        agent_id: Arc<String>,
        coordinator: Weak<AgentCoordinator>,
    ) -> (Arc<Session>, bool) {
        if let Some(s) = self.get(key) {
            return (s, false);
        }
        match self.sessions.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(e) => (e.get().clone(), false),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                let notify = Arc::new(tokio::sync::Notify::new());
                let (producer, rx, trigger_rx) = crate::batching::new_producer();
                let session = Arc::new(Session::new(key, model, agent_id, coordinator, producer, notify.clone()));
                let consumer = crate::batching::BatchConsumer::new(rx, trigger_rx, &session, notify);
                crate::batching::spawn_trigger(session.batch.clone(), consumer);
                e.insert(session.clone());
                (session, true)
            }
        }
    }
```

- [ ] **Step 3: coordinator.rs 删 weak_self + 槽设置改降级传参 + 删 flush_batch**

`kissbot-agent/src/coordinator.rs`：

1. 删除 `weak_self` 字段声明、`new()` 中的初始化与设置（约 132/161/165 行）；顶部 import `use std::sync::{Arc, OnceLock, Weak};` → `use std::sync::Arc;`（OnceLock/Weak 仅 weak_self 用；coordinator 测试内 `Weak::new()` 用全限定路径）

2. `ensure_session` 的 created 分支删除两个槽设置，改为把自身弱引用传入 get_or_create：

```rust
    /// 定位会话，新建时构建初始上下文；返回 (会话, 是否新建)
    /// channel_id 为触发会话创建/重置的来源 channel（新建会话的 agent_id 取自该 channel 运行态绑定）
    async fn ensure_session(self: &Arc<Self>, key: &SessionKey, channel_id: &str) -> (Arc<Session>, bool) {
        // valid_default.load_full() 返回 Arc<Option<ProviderModel>>，解引用克隆得 Option
        let model = (*self.valid_default.load_full()).clone();
        // 会话状态保存 agent_id：新建会话从来源 channel 运行态绑定取得（原子写入 get_or_create）
        let agent_id = self.channel_agent(channel_id).await;
        // coordinator 弱引用直接降级传入（依赖序构造，无 OnceLock 后置设置）
        let (session, created) = self.session_manager.get_or_create(key, model, agent_id, Arc::downgrade(&self));
        if created {
            self.build_initial_context(&session).await;
            // 随会话创建：绑定合批发送端（channel 持 tx/trigger_tx）
            self.bind_batch_tx(channel_id, &session).await;
        }
        (session, created)
    }
```

3. 删除 `flush_batch` 方法（约 983-993 行）；`run_agentic_loop` 与 `resolve_out_channel_for_session` 的 `async fn` 改为 `pub(crate) async fn`。

4. 协调处引用清理：`bind_batch_tx` 内 `session.batch.tx.clone()` / `session.batch.trigger_tx.clone()` 不变（producer 字段仍在）；`reset_context` 内 `session.batch.trigger_tx.send(...)` 不变；`enqueue_batch` 内 `session.batch.set_deadline(at)` 不变。

5. coordinator 测试 `tool_placeholder_uses_same_key_for_call_and_result` 的 `Session::new(&key, None, Arc::new("aid".into()))` 改为：

```rust
        let notify = Arc::new(tokio::sync::Notify::new());
        let (producer, _rx, _trigger_rx) = crate::batching::new_producer();
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into()), std::sync::Weak::new(), producer, notify));
```

- [ ] **Step 4: 重写 batching.rs 测试**

`kissbot-agent/src/batching.rs` 测试模块（`#[cfg(test)]` 内）：加 `use crate::types::{Mode, SessionKey};`，新增两个辅助函数，并把用到 `new_batch()` 的用例改为辅助构造：

```rust
    /// 测试 producer/consumer 对（未 spawn；consumer 持测试会话弱引用 + 会话 notify）
    fn test_pair() -> (BatchProducer, BatchConsumer, Arc<Session>) {
        let key = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let notify = Arc::new(Notify::new());
        let (producer, rx, trigger_rx) = new_producer();
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into()), Weak::new(), producer, notify.clone()));
        let consumer = BatchConsumer::new(rx, trigger_rx, &session, notify);
        (producer, consumer, session)
    }
```

用例改写（`SessionKey`/`Mode` 需在测试模块 import；`Weak` 用 `std::sync::Weak` 全限定或随模块 import）：

- `producer_consumer_new_pairs_channels`：`let (producer, mut consumer) = new_batch();` → `let (producer, mut consumer, _session) = test_pair();`
- `try_flush_drains_consumer_without_lock`：`let (producer, mut consumer) = new_batch();` → `let (producer, mut consumer, _session) = test_pair();`；`let (p2, mut c2) = new_batch();` → `let (p2, mut c2, _) = test_pair();`
- `flush_ready_respects_deadline_and_force`：三处 `let (producer, _consumer) = new_batch();` → `let (producer, _, _) = new_producer();`（producer 无 session 依赖，无需建会话）
- `spawn_trigger_flushes_on_at_trigger`：`let (producer, consumer) = new_batch();` → `let (producer, consumer, _session) = test_pair();`
- `spawn_trigger_flushes_on_forced`：同上
- `spawn_trigger_exits_on_notify`：`let (producer, consumer) = new_batch();` → `let (producer, consumer, session) = test_pair();`，且 `producer.notify.notify_one();` → `session.notify.notify_one();`（任务等待 consumer.notify 与 session.notify 同一 Arc）

- [ ] **Step 5: 更新 session_manager.rs 测试**

- `get_or_create_dedupes` / `retain_prunes_unbound` / `get_or_create_with_none_model`：`mgr.get_or_create(&k, Some(model.clone()), Arc::new("a1".into()))` 等调用末尾补 `, Weak::new()`（第四参）。
- `session_copies_role_name_and_mode_from_key`：`Session::new(&key, model, agent_id)` 改为：

```rust
        let notify = Arc::new(tokio::sync::Notify::new());
        let (producer, _rx, _trigger_rx) = crate::batching::new_producer();
        let session = Session::new(&key, model, agent_id, Weak::new(), producer, notify);
```

- [ ] **Step 6: 构建并跑全量测试**

Run: `cd kissbot-agent && cargo build 2>&1 | grep -c warning; cargo test 2>&1 | grep "test result"`
Expected: build 0 warnings；test result: ok. 111 passed（batching 7 个用例重写后数量不变）

- [ ] **Step 7: Commit**

```bash
git add kissbot-agent/src/batching.rs kissbot-agent/src/session_manager.rs kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): 合批引用改依赖序构造、删全部 OnceLock——BatchProducer 去 notify/升级槽，BatchConsumer 增 session 弱引用+notify（new_producer/BatchConsumer::new）；Session 增 coordinator 弱引用+notify、get_or_create 组装 notify+mpsc→producer→session→consumer→spawn；flush 改 session.accept_batch（原 flush_batch 删除，run_agentic_loop/resolve_out_channel_for_session 改 pub(crate)）；coordinator 删 weak_self，ensure_session 降级传参；测试适配"
```

### Task 3: ChannelContext 无锁持 producer + bind/enqueue 简化

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: Task 2 的 `Session.batch`（BatchProducer）、`ensure_session` Arc 链
- Produces: `ChannelContext.producer: ArcSwapOption<BatchProducer>`；`bind_batch(channel_id, session)`；`ensure_session` 每次调用都绑定；`enqueue_batch` 无锁读 producer

- [ ] **Step 1: ChannelContext 字段 batch_tx/trigger_tx → producer**

`kissbot-agent/src/coordinator.rs` `ChannelContext` 结构与 `new()`：

```rust
struct ChannelContext {
    /// 已发出未回显的 msg_id -> 记录时间（DashMap 无锁并发访问）
    pending_outgoing: DashMap<String, Instant>,
    /// 运行态 agent_id（ArcSwapOption 无锁读写；未绑定为 None，channel_agent 懒绑定）
    agent_id: ArcSwapOption<String>,
    /// 运行态模式（ArcSwap 无锁读写；/mode 切换不回写，重启回 Role）
    mode: ArcSwap<Mode>,
    /// 合批生产侧（绑定会话时从 session.batch 取 clone；会话重定位后刷新，None 时 enqueue 懒绑定）
    /// BatchProducer 字段全 Clone/Arc，无需锁——ArcSwapOption 原子替换/读取（与 agent_id 同模式）
    producer: ArcSwapOption<crate::batching::BatchProducer>,
}
```

`new()` 中 `batch_tx: tokio::sync::Mutex::new(None), trigger_tx: tokio::sync::Mutex::new(None),` → `producer: ArcSwapOption::new(None),`

- [ ] **Step 2: bind_batch_tx → bind_batch（绑 producer）**

```rust
    /// 绑定会话后刷新合批生产侧（从 session.batch 取 clone；会话创建/重定位时调用，None 时 enqueue 懒绑定）
    async fn bind_batch(&self, channel_id: &str, session: &Arc<Session>) {
        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        ctx.producer.store(Some(session.batch.clone()));
    }
```

（替换原 `bind_batch_tx` 定义；调用点 `self.bind_batch_tx(...)` 两处——`ensure_session` 与 `apply_channel_key`——改 `self.bind_batch(...)`）

- [ ] **Step 3: ensure_session 每次绑定（created 与非 created 幂等）**

`ensure_session` 中把绑定移出 created 分支：

```rust
        let (session, created) = self.session_manager.get_or_create(key, model, agent_id, Arc::downgrade(&self));
        if created {
            self.build_initial_context(&session).await;
        }
        // 绑定合批生产侧（每次调用幂等；多 channel 共享同一 session 时共享同一 producer 对）
        self.bind_batch(channel_id, &session).await;
        (session, created)
```

（删除 created 分支内的 `self.bind_batch(channel_id, &session).await;` 一行）

- [ ] **Step 4: enqueue_batch 无锁读 producer**

```rust
    async fn enqueue_batch(&self, channel_id: &str, session: &Arc<Session>, event: Arc<IncomingMessageEvent>) {
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        let interval = Duration::from_secs(cfg.channel_batch_interval_secs);

        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        // 懒绑定：无生产侧则从会话取（正常路径在 ensure_session/apply_channel_key 已绑定）
        if ctx.producer.load_full().is_none() {
            self.bind_batch(channel_id, session).await;
        }
        let Some(producer) = (*ctx.producer.load_full()).clone() else {
            warn!("enqueue_batch: channel {} 无合批生产侧", channel_id);
            return;
        };
        let _ = producer.tx.send(event);                                // 数据入队（队列累积，不逐条消费）
        let at = Instant::now() + interval;                             // 单次计算：deadline 与触发时间同源
        producer.set_deadline(at);                                      // 更新截止（防抖，后推覆盖）
        let _ = producer.trigger_tx.send(crate::batching::Trigger::At(at));  // 发送触发时间（绝对）
    }
```

- [ ] **Step 5: 构建并跑全量测试**

Run: `cd kissbot-agent && cargo build 2>&1 | grep -c warning; cargo test 2>&1 | grep "test result"`
Expected: build 0 warnings；test result: ok. 111 passed

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): ChannelContext 用 ArcSwapOption 无锁持 BatchProducer（替代 batch_tx/trigger_tx 双 Mutex 槽）——BatchProducer 字段全 Clone/Arc 无需锁；ensure_session 每次调用幂等绑定 producer（多 channel 共享同一 session 共用同一 producer 对）；enqueue_batch 经 ctx.producer 无锁读取 + set_deadline"
```

### Task 4: 全量回归确认

- [ ] **Step 1: 全量测试 + 0 warnings 复核**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -3 && cargo build 2>&1 | grep -c warning`
Expected: 111 passed、0 failed、0 warnings

- [ ] **Step 2: 引用残留检查**

Run: `cd /home/admin/project/kissbot && grep -rn "weak_self\|OnceLock\|\.batch\.session\|\.batch\.coordinator\|flush_batch\|new_batch()" kissbot-agent/src/`
Expected: 无输出（batching.rs 内注释提及 C1 缺陷除外——检查是否仅为注释措辞；若为注释则按「不删注释、更新措辞」处理：确认注释不再描述已删机制）

- [ ] **Step 3: 工作区清洁确认**

Run: `cd /home/admin/project/kissbot && git status --short`
Expected: 无未提交改动（全部已随前序任务提交）
