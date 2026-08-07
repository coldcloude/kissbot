# Channel 合批归属拆分实现计划（BatchProducer/BatchConsumer）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将合批 `BatchState`（rx/trigger_runtime 用 Mutex）拆分为 **BatchProducer**（生产侧：Session 持 Arc，全无锁共享类型）+ **BatchConsumer**（消费侧：trigger 任务独占、随 Session::new spawn move 进任务、任务内 mut 访问零锁）。全结构唯一同步 = 升级槽（`OnceLock<Weak<Session>>`/`OnceLock<Weak<Coordinator>>`）设置一次。

**Architecture:** `BatchProducer` 持 `tx/trigger_tx/deadline(ArcSwapOption)/notify/coordinator 槽/session 槽`；`BatchConsumer` 持 `rx/trigger_rx/delay`（任务独占）；`Session::new` 用 `BatchProducer::new()` 建两半，consumer 立即 move 进 `spawn_trigger` 的任务；任务 select! 直接 mut 访问 consumer（零锁）；flush 时升级 producer 槽（ensure_session 设置）→ coordinator.flush_batch；`Session::drop → notify.notify_one()` 退出不变。

**Tech Stack:** Rust 2024、tokio（mpsc/select/Notify）、tokio-util（DelayQueue）、arc-swap（ArcSwapOption）、std OnceLock/Weak。

## Global Constraints

- 遵守 `.claude/rules/coding-standards.md`：非枚举/非 Map Key/非 Vec 字段用 `Arc<T>` 包裹
- 不要删除代码中的注释（CLAUDE.md）；读写文件用 Read/Edit/Write 工具（禁止 sed/python）
- 测试运行：`cd kissbot-agent && cargo test`
- 提交 comment 中文，覆盖本次改动全部内容
- `OnceLock`/`Weak` 用 std；`OnceLock::set` 失败（已设置）忽略（coordinator 为进程级单例、session 槽每 producer 一次）
- 保持 110 测试基线全绿；0 warnings（消除原 rx/trigger_runtime 的 Mutex）

---

### Task 1: batching.rs 拆分（BatchProducer/BatchConsumer + spawn_trigger/try_flush 改造）

**Files:**
- Modify: `kissbot-agent/src/batching.rs`
- Modify: `kissbot-agent/src/session_manager.rs`

**Interfaces:**
- Produces: `BatchProducer { tx, trigger_tx, deadline: ArcSwapOption<Instant>, notify: Arc<Notify>, coordinator: Arc<OnceLock<Weak<AgentCoordinator>>>, session: Arc<OnceLock<Weak<Session>>> }` + `new() -> (Arc<Self>, BatchConsumer)`；`BatchConsumer { rx, trigger_rx, delay }`；`spawn_trigger(producer: Arc<BatchProducer>, consumer: BatchConsumer)`（move consumer 进任务）；`try_flush(producer, consumer: &mut BatchConsumer, force)`（零锁 drain）；`flush_ready(producer, force)`；`pack_events`
- Consumes: `Session`（Weak 槽）、`AgentCoordinator::flush_batch`（已有）

- [ ] **Step 1: 写失败测试（batching.rs tests 更新/新增）**

```rust
    #[tokio::test]
    async fn producer_consumer_new_pairs_channels() {
        let (producer, mut consumer) = BatchProducer::new();
        producer.tx.send(ev("u1", "a")).unwrap();
        // 消费侧直接 &mut 取（零锁）
        let item = consumer.rx.try_recv().unwrap();
        assert!(matches!(&*item, ...));  // ev("u1","a") 还原断言
        assert!(consumer.rx.try_recv().is_err(), "仅一条");
    }

    #[tokio::test]
    async fn try_flush_drains_consumer_without_lock() {
        let (producer, mut consumer) = BatchProducer::new();
        producer.tx.send(ev("u1", "a")).unwrap();
        producer.tx.send(ev("u2", "b")).unwrap();
        producer.deadline.store(Some(Arc::new(Instant::now() - Duration::from_secs(1))));
        // 槽未设置：升级失败 → 数据被 drain（丢弃），可观测
        try_flush(&producer, &mut consumer, false).await;
        assert!(consumer.rx.try_recv().is_err(), "已 drain");
        // 未超 deadline：不 drain
        let (p2, mut c2) = BatchProducer::new();
        p2.tx.send(ev("u1", "x")).unwrap();
        p2.deadline.store(Some(Arc::new(Instant::now() + Duration::from_secs(10))));
        try_flush(&p2, &mut c2, false).await;
        assert!(c2.rx.try_recv().is_ok(), "未超时不应 drain");
    }
```

（`ev` 辅助沿用现有 tests；`try_flush` 签名以实际为准——需 &mut consumer。）

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test producer_consumer_new_pairs_channels`
Expected: 编译失败——`BatchProducer` 未定义

- [ ] **Step 3: 实现 batching.rs**

```rust
use std::sync::{Arc, OnceLock, Weak};

/// 生产侧：Session 持有（Arc）；全无锁共享类型
pub struct BatchProducer {
    pub tx: mpsc::UnboundedSender<BatchItem>,
    pub trigger_tx: mpsc::UnboundedSender<Trigger>,
    pub deadline: ArcSwapOption<Instant>,
    pub notify: Arc<Notify>,
    /// 任务升级槽（ensure_session 创建会话后设置一次；coordinator 为进程级单例）
    pub coordinator: Arc<OnceLock<Weak<crate::coordinator::AgentCoordinator>>>,
    /// 任务升级槽（ensure_session 设置一次：Arc::downgrade(&session)）
    pub session: Arc<OnceLock<Weak<Session>>>,
}

/// 消费侧：trigger 任务独占（随 spawn move 进任务，任务内 mut 访问，零锁）
pub struct BatchConsumer {
    pub rx: mpsc::UnboundedReceiver<BatchItem>,
    pub trigger_rx: mpsc::UnboundedReceiver<Trigger>,
    pub delay: DelayQueue<Trigger>,
}

impl BatchProducer {
    /// 创建 channel 对 + DelayQueue，返回 (生产侧, 消费侧)
    pub fn new() -> (Arc<Self>, BatchConsumer) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        let producer = Arc::new(Self {
            tx,
            trigger_tx,
            deadline: ArcSwapOption::from(None),
            notify: Arc::new(Notify::new()),
            coordinator: Arc::new(OnceLock::new()),
            session: Arc::new(OnceLock::new()),
        });
        let consumer = BatchConsumer {
            rx,
            trigger_rx,
            delay: DelayQueue::new(),
        };
        (producer, consumer)
    }
}

/// 触发任务：随 Session::new 调用（consumer 创建后立即移入）；唯一消费者（独占 &mut consumer，零锁）
pub fn spawn_trigger(producer: Arc<BatchProducer>, mut consumer: BatchConsumer) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = producer.notify.notified() => break,          // session 销毁（notify_one）→ 退出
                t = consumer.trigger_rx.recv() => {
                    match t {
                        Some(Trigger::At(at)) => consumer.delay.insert(Trigger::At(at), at.saturating_duration_since(Instant::now())),
                        Some(Trigger::Forced) => consumer.delay.insert(Trigger::Forced, Duration::ZERO),
                        None => break,
                    }
                }
                item = std::future::poll_fn(|cx| consumer.delay.poll_expired(cx)), if !consumer.delay.is_empty() => {
                    // 守卫：队列空时禁用该分支——poll_expired 空队列返回 Ready(None)，无守卫 spawn 首轮即退出
                    match item {
                        Some(Trigger::Forced) => try_flush(&producer, &mut consumer, true).await,
                        Some(Trigger::At(_))  => try_flush(&producer, &mut consumer, false).await,
                        None => break,                             // 仅防御
                    }
                }
            }
        }
    });
}

/// 触发 flush：判定 → deadline 置 None → drain（&mut consumer.rx 零锁）→ 打包 → 升级槽进 agentic loop
pub async fn try_flush(
    producer: &Arc<BatchProducer>,
    consumer: &mut BatchConsumer,
    force: bool,
) {
    if !flush_ready(producer, force) {
        return;
    }
    producer.deadline.store(None);
    let mut items = Vec::new();
    loop {
        match consumer.rx.try_recv() {
            Ok(item) => items.push(item),
            Err(_) => break,
        }
    }
    if items.is_empty() {
        return;
    }
    let content = pack_events(&items);
    let (Some(s), Some(c)) = (
        producer.session.get().and_then(|w| w.upgrade()),
        producer.coordinator.get().and_then(|w| w.upgrade()),
    ) else {
        return;   // 会话/协调器已销毁：数据已 drain（丢弃）
    };
    c.flush_batch(&s, content).await;
}

/// 触发判定：非强制且未超 deadline → 不 flush；强制 → flush
pub fn flush_ready(producer: &BatchProducer, force: bool) -> bool {
    if force {
        return true;
    }
    match producer.deadline.load_full() {
        None => false,
        Some(d) => Instant::now() >= **d,
    }
}
```

（`pack_events`/`BatchItem`/`Trigger` 不变；删除 `BatchState` 与 `rx`/`trigger_runtime` Mutex；`drain` 方法删除——改为任务内直接 `consumer.rx.try_recv()`。）

- [ ] **Step 4: session_manager.rs——Session::new 建两半并 spawn**

```rust
pub struct Session {
    pub agent_name: Arc<String>,
    pub role_name: Arc<String>,
    pub mode: Arc<Mode>,
    pub context: tokio::sync::Mutex<SessionContext>,
    /// 合批生产侧（数据/触发发送端 + deadline + notify + 升级槽；trigger 任务持消费侧）
    pub batch: Arc<crate::batching::BatchProducer>,
    pub model: ArcSwap<Option<ProviderModel>>,
    pub agent_id: Arc<String>,
}

impl Session {
    pub fn new(key: &SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> Self {
        let (batch, consumer) = crate::batching::BatchProducer::new();
        // 消费侧立即移入 trigger 任务（随 session 创建；升级槽由 ensure_session 设置）
        crate::batching::spawn_trigger(batch.clone(), consumer);
        Self {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new()),
            batch,
            model: ArcSwap::from_pointee(model),
            agent_id,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.batch.notify.notify_one();   // 会话销毁通知 trigger 任务退出（permit 语义）
    }
}
```

> `Session::new` 在 async 上下文（ensure_session 调用链）内执行，`tokio::spawn` 可用。

- [ ] **Step 5: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: batching/session 新测试 PASS；coordinator 引用 `session.batch` 字段名不变（tx/trigger_tx/deadline/notify 同生产侧）——编译若因 `spawn_trigger` 签名变化或 ensure_session 旧调用点破坏，Task 2 处理；本任务先保证 batching/session 模块内正确（coordinator 的 ensure_session 旧 `spawn_trigger` 调用点需临时适配或与 Task 2 合并提交——见 Task 2 说明）。

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/batching.rs kissbot-agent/src/session_manager.rs
git commit -m "refactor(agent): 合批归属拆分——BatchState 拆 BatchProducer（生产侧无锁共享 + 升级槽 OnceLock）/ BatchConsumer（消费侧任务独占 move 进任务零锁）；Session::new 建两半并 spawn trigger 任务；try_flush 直接 &mut consumer drain（删 rx/trigger_runtime Mutex）"
```

---

### Task 2: coordinator.rs（升级槽设置 + 删除旧 spawn 调用点）+ 收尾

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`
- Modify: 文档（nexus.md / script/README.md 若需）

**Interfaces:**
- Consumes: `BatchProducer.session/coordinator` 槽（Task 1）
- Produces: `ensure_session`（created=true）设置 `session.batch.session.set(Arc::downgrade(&session))` + `session.batch.coordinator.set(weak_self)`；删除 ensure_session 内旧 `spawn_trigger` 调用（移至 Session::new）

- [ ] **Step 1: ensure_session 设置升级槽 + 删旧 spawn**

`ensure_session` created 分支（原 `spawn_trigger(...)` 调用替换为槽设置）：

```rust
        if created {
            self.build_initial_context(&session).await;
            // trigger 任务已在 Session::new 随会话 spawn；此处设置其升级槽（OnceLock set 一次；
            // coordinator 为进程级单例，session Weak 每 producer 一次——消息必须先过 ensure_session
            // 才路由进队列，故 flush 时槽必然已设置）
            let _ = session.batch.session.set(Arc::downgrade(&session));
            let _ = session.batch.coordinator.set(self.weak_self.get().cloned().unwrap_or_default());
        }
```

- [ ] **Step 2: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过；grep `spawn_trigger(` 仅 batching.rs 定义 + session_manager 调用，coordinator 零调用；grep `BatchState`/`trigger_runtime`/`Mutex<Option<` 零残留（除必要注释）

- [ ] **Step 3: 文档同步（若字段/机制表述过时）**

`docs/design/components-design/kissbot-agent-nexus.md` 合批节：BatchProducer（生产侧共享）/BatchConsumer（任务独占零锁）表述；`script/README.md` 无机制细节则不动。

- [ ] **Step 4: 全量验证 + Commit**

Run: `cd kissbot-agent && cargo test`；`cd kissbot-memory && cargo test`；`cd kissbot-api && cargo test`；`cargo build` 0 warnings

```bash
git add kissbot-agent/src/coordinator.rs docs/design/components-design/
git commit -m "refactor(agent): ensure_session 设置升级槽（session/coordinator OnceLock），删除旧 spawn_trigger 调用点（移至 Session::new）；文档同步归属拆分"
```

---

## Self-Review 记录

（writing-plans 自审，修正项已同步进正文）

**1. Spec 覆盖检查**（归属拆分 spec → 任务）：

| Spec 节 | 任务 |
|---|---|
| 生命周期与所有权（归属拆分、任务随 Session::new spawn、升级槽、Notify 退出） | T1、T2 |
| 结构（BatchProducer/BatchConsumer/new() 建两半） | T1 |
| ② trigger 任务（独占 consumer 零锁、守卫） | T1 |
| ③ try_flush（producer/consumer、零锁 drain、升级槽） | T1 |
| ④ 重置强制 flush | 不变（enqueue/reset 用 producer 字段，字段名不变） |
| 关键性质（消费侧零锁/生产侧零锁/唯一同步=槽设置一次） | T1、T2 |
| 测试范围 | T1 |
| 受影响文件 | 各任务 Files |

**2. 占位符扫描**：无 TBD/TODO。

**3. 类型一致性**：`BatchProducer::new() -> (Arc<Self>, BatchConsumer)`；`spawn_trigger(Arc<BatchProducer>, BatchConsumer)`；`try_flush(&Arc<BatchProducer>, &mut BatchConsumer, bool)`；`BatchProducer.session/coordinator: Arc<OnceLock<Weak<..>>>`——enqueue/reset 继续用 `session.batch.tx/trigger_tx/deadline/notify`（字段名未变，仅类型名变化）。旧 `flush_events_to_loop`/`drain`/`flush_ready(batch,..)` 改为 `try_flush`/`flush_ready(producer,..)`——T1 内一致更新。
