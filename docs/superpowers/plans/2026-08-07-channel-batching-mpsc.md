# Channel 合批触发器重构实现计划（mpsc×2 + DelayQueue）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 spec（docs/superpowers/specs/2026-08-07-channel-batching-mpsc-design.md）重构 channel 合批：数据队列（mpsc，元素 `Arc<IncomingMessageEvent>`）+ 触发器队列（mpsc，`Trigger::At(Instant)`/`Forced`）+ DelayQueue（归 trigger 任务所有）+ 随 session 生命周期的 trigger 任务；flush 天然串行，无 armed/CAS/resetting。

**Architecture:** `BatchState`（Session 的 Arc 字段）持数据/触发两对 mpsc + DelayQueue + `ArcSwapOption<Instant>` deadline + `Notify`；channel 持 tx/trigger_tx（绑定会话时 clone）；trigger 任务随 session 创建 spawn（持 `Arc<BatchState>` + `Weak<Session>` + `Weak<Coordinator>`），`select!` 并行等待 `trigger_rx.recv()`/`delay.next()`/`notify.notified()`；`try_flush` 按 deadline 判断（非强制）或直接（强制），drain 全部打包进 agentic loop；`Session::drop` → `notify.notify_one()` 使任务退出。

**Tech Stack:** Rust 2024、tokio（mpsc/select/Notify）、tokio-util（DelayQueue，新增依赖）、arc-swap（ArcSwapOption，已有）、kai-rs/kissbot-api（IncomingMessageEvent/Content）。

## Global Constraints

- 遵守 `.claude/rules/coding-standards.md`：时间格式 `yyyy-MM-dd HH:mm:ss`；非枚举/非 Map Key/非 Vec 字段用 `Arc<T>` 包裹
- 不要删除代码中的注释（CLAUDE.md）；读写文件用 Read/Edit/Write 工具（禁止 sed/python 修改文件）
- 测试运行：`cd kissbot-agent && cargo test`（无根 workspace）
- 提交 comment 中文，覆盖本次改动全部内容
- 新增依赖 `tokio-util = { version = "0.7", features = ["time"] }`（DelayQueue）
- 数据队列元素 = `Arc<IncomingMessageEvent>`（**不新建 BatchItem 结构体**）；打包时取 `user_name` + `extract_text(content)`
- 删除旧机制：`BatchBuffer`、`pack_batch(Vec<(String,String)>)`、`flush_after_reset`、`Session.resetting`（Rework Task 1 已删 batch_gen）

---

### Task 1: batching.rs 重构（BatchState/Trigger/trigger_loop/try_flush）+ tokio-util

**Files:**
- Modify: `kissbot-agent/Cargo.toml`（+tokio-util）
- Rewrite: `kissbot-agent/src/batching.rs`

**Interfaces:**
- Produces: `BatchItem = Arc<IncomingMessageEvent>`；`enum Trigger { At(Instant), Forced }`；`struct BatchState { tx, rx(Mutex<Option<..>>), trigger_tx, trigger_rx, delay, deadline: ArcSwapOption<Instant>, notify: Arc<Notify> }`；`BatchState::new() -> Arc<Self>`；`drain_batch(&BatchState) -> Vec<BatchItem>`；`pack_events(&[BatchItem]) -> String`；`try_flush(batch, session: &Arc<Session>, coordinator: &Arc<AgentCoordinator>, force)`；`spawn_trigger(batch: Arc<BatchState>, session: Weak<Session>, coordinator: Weak<AgentCoordinator>)`
- Consumes: `kissbot_api::channel::IncomingMessageEvent`、`Content` 文本提取（复用 coordinator 的 extract_text——需 pub(crate) 化）

- [ ] **Step 1: 写失败测试（batching.rs tests 重写）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::AgentCoordinator;  // 若测试需协调器则用轻量替代（见 Step 3 说明）
    use crate::session_manager::Session;
    use crate::types::{Mode, SessionKey};
    use kissbot_api::channel::IncomingMessageEvent;
    use kissbot_api::message::Content;

    fn ev(name: &str, text: &str) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: Arc::new("self".into()),
            incoming_message: Arc::new(kissbot_api::channel::IncomingMessage {
                msg_id: Arc::new("m".into()),
                messenger_id: Arc::new("web".into()),
                user_id: Arc::new("u".into()),
                group_id: Arc::new("g".into()),
                messenger_name: Arc::new("".into()),
                user_name: Arc::new(name.into()),
                group_name: Arc::new("".into()),
                content: Content::Text(Arc::new(text.into())),
                time: Arc::new("2026-08-07 10:00:00".into()),
            }),
        })
    }

    #[test]
    fn pack_events_builds_name_content_lines() {
        let events = vec![ev("u1", "你好"), ev("u2", "在吗")];
        assert_eq!(pack_events(&events), "u1: 你好\nu2: 在吗");
    }

    #[tokio::test]
    async fn drain_accumulates_and_channel_stays_open() {
        let batch = BatchState::new();
        batch.tx.send(ev("u1", "a")).unwrap();
        batch.tx.send(ev("u2", "b")).unwrap();
        let items = drain_batch(&batch);
        assert_eq!(items.len(), 2);
        // channel 未关闭：再次 send 可消费
        batch.tx.send(ev("u3", "c")).unwrap();
        assert_eq!(drain_batch(&batch).len(), 1);
    }

    #[tokio::test]
    async fn try_flush_respects_deadline() {
        let batch = BatchState::new();
        batch.tx.send(ev("u1", "a")).unwrap();
        batch.deadline.store(Some(Arc::new(Instant::now() + Duration::from_secs(10))));
        // 未超 deadline：非强制不 flush
        try_flush(&batch, None, false).await;
        assert_eq!(drain_batch(&batch).len(), 1, "未超时不应 drain");
        // 强制：无视 deadline 立即 drain
        try_flush(&batch, None, true).await;
        assert!(drain_batch(&batch).is_empty(), "强制 flush 应 drain 全部");
    }
}
```

> `try_flush` 的签名以能测试为准：测试用 `None` 替代 coordinator/session（`try_flush(batch, None, force)` 仅测 drain/打包部分；带会话的完整 flush 在 Task 3 接线后由集成路径覆盖）。若签名不便，抽 `fn flush_ready(batch, force) -> bool` 纯判定 + `drain_batch` 组合测。

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test pack_events_builds_name_content_lines`
Expected: 编译失败——`pack_events`/`BatchState` 未定义

- [ ] **Step 3: 实现 batching.rs**

`Cargo.toml` 增加：

```toml
tokio-util = { version = "0.7", features = ["time"] }
```

`batching.rs` 整体重写：

```rust
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use kissbot_api::channel::IncomingMessageEvent;
use tokio::sync::{mpsc, Notify};
use tokio_util::time::DelayQueue;

use crate::session_manager::Session;

/// 合批数据：直接用 IncomingMessageEvent（不新建 BatchItem）
pub type BatchItem = Arc<IncomingMessageEvent>;

/// 触发器消息：channel 发送触发时间；reset 发送强制
pub enum Trigger {
    /// 非强制：到期后按当前 deadline 判断（可能被后续消息延长而空转）
    At(Instant),
    /// 强制：立即 flush（上下文重置用）
    Forced,
}

/// 会话合批状态（Session 的 Arc 字段；trigger 任务持此 Arc，不持 Arc<Session>）
pub struct BatchState {
    pub tx: mpsc::UnboundedSender<BatchItem>,
    rx: tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<BatchItem>>>,
    pub trigger_tx: mpsc::UnboundedSender<Trigger>,
    trigger_rx: mpsc::UnboundedReceiver<Trigger>,
    delay: DelayQueue<Trigger>,
    /// 截止时间（无锁：推数据时 store，try_flush 时 load）
    pub deadline: ArcSwapOption<Instant>,
    /// 任务退出唤醒（session 销毁 notify_one；permit 语义错过唤醒后仍可退出）
    pub notify: Arc<Notify>,
}

impl BatchState {
    pub fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            tx,
            rx: tokio::sync::Mutex::new(Some(rx)),
            trigger_tx,
            trigger_rx,
            delay: DelayQueue::new(),
            deadline: ArcSwapOption::from(None),
            notify: Arc::new(Notify::new()),
        })
    }

    /// 一次性读出全部待合批数据（channel 不关闭，跨 flush 复用）
    pub async fn drain(&self) -> Vec<BatchItem> {
        let mut rx = self.rx.lock().await;
        let mut items = Vec::new();
        if let Some(rx) = rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(item) => items.push(item),
                    Err(_) => break,   // Empty / Disconnected
                }
            }
        }
        items
    }
}

/// 打包为一条 user 消息的 content：逐行 "name: text"（name 为空只留 text）
pub fn pack_events(events: &[BatchItem]) -> String {
    events.iter().map(|e| {
        let name = e.incoming_message.user_name.as_str();
        let text = crate::coordinator::extract_text(&e.incoming_message.content);
        if name.is_empty() { text } else { format!("{}: {}", name, text) }
    }).collect::<Vec<_>>().join("\n")
}

/// 触发判定：非强制且未超 deadline → 不 flush；强制 → flush
pub fn flush_ready(batch: &BatchState, force: bool) -> bool {
    if force {
        return true;
    }
    match batch.deadline.load_full() {
        None => false,
        Some(d) => Instant::now() >= **d,
    }
}

/// 触发任务：随 session 创建 spawn；唯一消费者（trigger_rx + DelayQueue 所有权）
/// 持 Arc<BatchState> + Weak<Session> + Weak<Coordinator>——不阻止 session drop
pub fn spawn_trigger(
    batch: Arc<BatchState>,
    session: Weak<Session>,
    coordinator: Weak<crate::coordinator::AgentCoordinator>,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = batch.notify.notified() => break,      // session 销毁（notify_one）→ 退出
                t = batch.trigger_rx.recv() => {
                    match t {
                        Some(Trigger::At(at)) => batch.delay.insert(Trigger::At(at), at.saturating_duration_since(Instant::now())),
                        Some(Trigger::Forced) => batch.delay.insert(Trigger::Forced, Duration::ZERO),
                        None => break,                      // trigger channel 关闭
                    }
                }
                item = batch.delay.next() => {
                    match item {
                        Some(Trigger::Forced) => {
                            if let (Some(s), Some(c)) = (session.upgrade(), coordinator.upgrade()) {
                                flush_events_to_loop(&batch, &s, &c, true).await;
                            }
                        }
                        Some(Trigger::At(_)) => {
                            if let (Some(s), Some(c)) = (session.upgrade(), coordinator.upgrade()) {
                                flush_events_to_loop(&batch, &s, &c, false).await;
                            }
                        }
                        None => break,                      // delay 关闭
                    }
                }
            }
        }
    });
}

/// 触发 flush：判定 → drain 全部 → 打包 → 交协调器进 agentic loop
/// （命名 flush_events_to_loop 与 coordinator 的 flush_batch 方法区分）
pub async fn flush_events_to_loop(
    batch: &Arc<BatchState>,
    session: &Arc<Session>,
    coordinator: &Arc<crate::coordinator::AgentCoordinator>,
    force: bool,
) {
    if !flush_ready(batch, force) {
        return;   // 非强制且未超 deadline：空转（等下一个到期触发）
    }
    let items = batch.drain().await;
    batch.deadline.store(None);
    if items.is_empty() {
        return;
    }
    let content = pack_events(&items);
    coordinator.flush_batch(session, content).await;
}
```

> `crate::coordinator::extract_text` 与 `AgentCoordinator::flush_batch` 在 Task 3 提供（本任务先加 pub(crate) 或占位以便编译——按实际编译调整）。若跨模块引用导致循环，`extract_text` 移到 batching.rs 或 kissbot-api（`Content` 文本提取），coordinator 复用。

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-agent && cargo test batching`
Expected: 新测试 PASS（旧 BatchBuffer/pack_batch/flush_after_reset 测试随重写删除）

- [ ] **Step 5: 全量确认**

Run: `cd kissbot-agent && cargo test`
Expected: 除 coordinator/session 引用待 Task 2/3 接线的编译错误外，先确认 batching 模块本身；若 coordinator 引用阻塞，Task 2/3 合并处理编译（顺序执行时 Task 1 允许临时 `#[allow(dead_code)]` 或最小桩）

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/Cargo.toml kissbot-agent/src/batching.rs
git commit -m "feat(agent): 合批重构之 batching.rs——BatchState（数据/触发双 mpsc + DelayQueue + ArcSwapOption deadline + Notify）、Trigger::At/Forced、spawn_trigger 任务（select! 并行 recv/next/notified）、flush_batch（判定+drain+打包）；新增 tokio-util 依赖；删除旧 BatchBuffer/pack_batch/flush_after_reset"
```

---

### Task 2: session_manager.rs（Session.batch: Arc<BatchState>、Drop 通知）

**Files:**
- Modify: `kissbot-agent/src/session_manager.rs`

**Interfaces:**
- Produces: `Session.batch: Arc<BatchState>`（替换 `Mutex<BatchBuffer>`）；`Session::new` 创建 BatchState（`BatchState::new()`）；`Session` 实现 `Drop` → `self.batch.notify.notify_one()`
- Removes: `Session.resetting: Arc<AtomicBool>`、`BatchBuffer` 引用、batching 旧 API

- [ ] **Step 1: 改 Session 结构与 Drop**

```rust
use std::sync::atomic::AtomicBool;  // 若不再使用则删除该 import

pub struct Session {
    pub agent_name: Arc<String>,
    pub role_name: Arc<String>,
    pub mode: Arc<Mode>,
    pub context: tokio::sync::Mutex<SessionContext>,
    /// 合批状态（数据/触发双 mpsc + DelayQueue + deadline + notify；trigger 任务持此 Arc）
    pub batch: Arc<crate::batching::BatchState>,
    pub model: ArcSwap<Option<ProviderModel>>,
    pub agent_id: Arc<String>,
}

impl Session {
    pub fn new(key: &SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> Self {
        Self {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new()),
            batch: crate::batching::BatchState::new(),
            model: ArcSwap::from_pointee(model),
            agent_id,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // 会话销毁：通知 trigger 任务退出（notify_one permit 语义：任务错过唤醒后下一轮 notified 立即完成）
        self.batch.notify.notify_one();
    }
}
```

> 注意：trigger 任务持 `Arc<BatchState>` 而非 `Arc<Session>`——Session 可正常 drop，`Drop` 必然执行并唤醒任务；任务退出后释放 BatchState。若任务（Task 1 实现）持 `Weak<Session>`，此处 Drop 时 Weak 升级已失败（session 正在销毁），无问题。

- [ ] **Step 2: 删除旧 batch/resetting 字段与相关代码**

删除 `Session.batch: Mutex<BatchBuffer>`、`Session.resetting`、`Session::new` 中对应初始化；删除 `use crate::batching::BatchBuffer` 引用（改为 BatchState）。`BatchBuffer` 类型本身在 Task 1 已从 batching.rs 删除。

- [ ] **Step 3: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: session_manager 测试适配（`Session::new` 构造不变，仅内部字段变化）；coordinator 中引用 `session.batch.lock()`/`session.resetting` 处编译错误由 Task 3 修复——若本任务单独编译不过，与 Task 3 合并提交（见 Task 3 说明）。

- [ ] **Step 4: Commit**

```bash
git add kissbot-agent/src/session_manager.rs
git commit -m "refactor(agent): Session.batch 改 Arc<BatchState>（合批状态随 session 生命周期），Session 实现 Drop 通知 trigger 任务退出（notify_one）；删除 resetting 标志与旧 Mutex<BatchBuffer>"
```

---

### Task 3: coordinator.rs（enqueue 改造 + spawn 触发器 + reset 强制 flush）

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: `BatchState::new/spawn_trigger/flush_batch`、`Session.batch`（Task 1/2）
- Produces: `ChannelContext.batch_tx/trigger_tx`（Option，绑定会话时设置）；`AgentCoordinator::flush_batch(&self, session, content)`（resolve out_channel + run_agentic_loop）；`enqueue_batch` 改造（send 数据 + store deadline + send At）；`ensure_session` created=true 时 spawn_trigger；`reset_context` 末尾 send Forced

- [ ] **Step 1: ChannelContext 持 tx/trigger_tx + 绑定刷新**

`ChannelContext` 增加：

```rust
    /// 合批数据发送端（绑定会话时从 session.batch 取得；解绑清空）
    batch_tx: tokio::sync::Mutex<Option<mpsc::UnboundedSender<crate::batching::BatchItem>>>,
    /// 合批触发器发送端（同上）
    trigger_tx: tokio::sync::Mutex<Option<mpsc::UnboundedSender<crate::batching::Trigger>>>,
```

新增方法（在 `bind_channel_runtime`/`apply_channel_key` 等绑定路径调用，绑定后刷新）：

```rust
    /// 绑定会话后刷新合批发送端（从 session.batch 取 clone；解绑/重定位时调用可置 None）
    async fn bind_batch_tx(&self, channel_id: &str, session: &Arc<Session>) {
        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        *ctx.batch_tx.lock().await = Some(session.batch.tx.clone());
        *ctx.trigger_tx.lock().await = Some(session.batch.trigger_tx.clone());
    }
```

调用点：`ensure_session`（created 时）+ `apply_channel_key`（会话重定位后）——channel 的会话三元组变化处刷新；`enqueue_batch` 内若为 None 则懒刷新。

- [ ] **Step 2: enqueue_batch 改造（channel 侧）**

`enqueue_batch`（原 push+was_empty+spawn 逻辑）替换为：

```rust
    /// 合批：数据入队 + 更新 deadline + 发送触发时间（At）。无 sleep、无逐消息任务。
    async fn enqueue_batch(&self, channel_id: &str, session: &Arc<Session>, event: Arc<IncomingMessageEvent>) {
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        let interval = Duration::from_secs(cfg.channel_batch_interval_secs);

        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        // 懒绑定：无发送端则从 session 取
        if ctx.batch_tx.lock().await.is_none() {
            self.bind_batch_tx(channel_id, session).await;
        }
        let (btx, ttx) = (ctx.batch_tx.lock().await.clone(), ctx.trigger_tx.lock().await.clone());
        if let (Some(btx), Some(ttx)) = (btx, ttx) {
            let _ = btx.send(event);                                   // 数据入队
            session.batch.deadline.store(Some(Arc::new(Instant::now() + interval)));  // 更新截止（防抖）
            let _ = ttx.send(crate::batching::Trigger::At(Instant::now() + interval)); // 发送触发时间
        } else {
            warn!("enqueue_batch: channel {} 无合批发送端", channel_id);
        }
    }
```

`handle_incoming` 调用处：`self.enqueue_batch(channel_id, &session, event.clone()).await`（不再传 user_name/content_text）。

- [ ] **Step 3: ensure_session spawn 触发器**

`ensure_session` 中 `created == true` 分支，`build_initial_context` 之后：

```rust
        if created {
            self.build_initial_context(&session).await;
            // 随 session 创建 spawn trigger 任务（持 Weak<Session>/Weak<Self>，不阻止销毁）
            crate::batching::spawn_trigger(
                session.batch.clone(),
                Arc::downgrade(&session),
                self.weak_self.get().cloned().unwrap_or_default(),
            );
        }
```

（`weak_self: OnceLock<Weak<Self>>` 已有——Task 7 建立的；若 get 为 None 则任务持空 Weak，flush 时升级失败跳过。）

- [ ] **Step 4: flush_batch 方法（coordinator）**

```rust
    /// trigger 任务打包后调用：解析 out_channel 并进入 agentic loop
    pub async fn flush_batch(&self, session: &Arc<Session>, content: String) {
        // 无可用模型：静默忽略（与 run_agentic_loop 入口一致）
        if session.model.load().is_none() {
            return;
        }
        // 按会话 (agent, role) 解析 out_channel（扫描匹配 channel 的 outgoing）
        let Some(out_channel) = self.resolve_out_channel_for_session(session).await else {
            warn!("flush_batch: 会话无 out_channel，跳过");
            return;
        };
        self.run_agentic_loop("", session, content, &out_channel).await;
    }

    /// 按会话 (agent, role) 找 out_channel（resolve_out_channel 的会话版）
    async fn resolve_out_channel_for_session(&self, session: &Arc<Session>) -> Option<OutChannel> {
        let channels = self.config.channels().await;
        for (_, c) in &channels {
            if c.agent_name.as_str() == session.agent_name.as_str()
                && c.role_name.as_str() == session.role_name.as_str()
            {
                if let Some(out) = &c.outgoing {
                    return Some(OutChannel {
                        channel_id: c.channel_id.clone(),
                        user: ChannelUser {
                            messenger_id: out.messenger_id.to_string(),
                            user_id: out.user_id.to_string(),
                        },
                        group_id: out.group_id.clone(),
                    });
                }
            }
        }
        None
    }
```

（`run_agentic_loop` 的 `_channel_id` 参数用 `""` 占位——该参数当前未使用（Task 7 重构后）；`extract_text` 改为 `pub(crate)` 供 batching.rs 复用。）

- [ ] **Step 5: reset_context 末尾强制 flush**

`reset_context` 删除 `resetting` 置位与 `batch.clear()`（Task 1 已删 BatchBuffer，确认无残留），末尾增加：

```rust
    // 重置完成：强制 flush（不检查 deadline），重置期间到达的消息即刻并入新上下文
    session.batch.trigger_tx.send(crate::batching::Trigger::Forced).ok();
```

> 注意：`reset_context` 可能由 trigger 任务的 flush → run_agentic_loop 溢出路径调用（任务内部），此时 send Forced 入队，任务下次 select 取到后强制 flush——串行保证无并发。

- [ ] **Step 6: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过；grep 确认 `session.batch.lock()`/`session.resetting`/`flush_after_reset`/`pack_batch(` 旧引用零残留

- [ ] **Step 7: Commit**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "refactor(agent): 合批接入——ChannelContext 持 batch_tx/trigger_tx（绑定会话时刷新/懒绑定）；enqueue_batch 改 send 数据+store deadline+send At（无 sleep 任务）；ensure_session 创建时 spawn trigger 任务；flush_batch/resolve_out_channel_for_session 供任务调用；reset_context 末尾 send Forced 强制 flush；删除 resetting/旧 flush_after_reset 逻辑"
```

---

### Task 4: 收尾（文档 + 全量验证）

**Files:**
- Modify: `docs/design/components-design/kissbot-agent-nexus.md`（合批机制描述）
- Modify: `script/README.md`（手动验证清单合批描述）

- [ ] **Step 1: 全量构建/测试**

Run: `cd kissbot-agent && cargo build && cargo test`；`cd kissbot-memory && cargo test`；`cd kissbot-api && cargo test`
Expected: 全绿、0 warnings（tokio-util 依赖解析成功）

- [ ] **Step 2: 文档更新（只说明现状）**

`kissbot-agent-nexus.md` 合批相关节：数据队列（mpsc，`Arc<IncomingMessageEvent>`）+ 触发器队列（`Trigger::At(触发时间)`/`Forced`）+ DelayQueue 定时触发；trigger 任务随会话创建、`select!` 并行等待、按 deadline 判断（非强制）/直接（强制）；重置末尾强制 flush；channel 持发送端、会话持接收端与触发器。

`script/README.md`：验证点描述同步（合批窗口 3s、重置后强制合并）。

- [ ] **Step 3: Commit**

```bash
git add docs/design/components-design/ script/README.md
git commit -m "docs(agent): 合批机制文档同步——数据/触发双 mpsc + DelayQueue、trigger 任务随会话、deadline/强制 flush、channel 持发送端"
```

---

## Self-Review 记录

（writing-plans 自审，修正项已同步进正文）

**1. Spec 覆盖检查**（rework spec 各节 → 任务）：

| Spec 节 | 任务 |
|---|---|
| 生命周期与所有权（双 mpsc + DelayQueue 随 session、任务持 BatchState、Notify 退出） | T1、T2、T3 |
| 结构（BatchState/Trigger/deadline ArcSwapOption/notify） | T1、T2 |
| ① channel 收消息（send+store+send At） | T3 |
| ② trigger 任务（select! recv/next/notified） | T1 |
| ③ try_flush（deadline 判定/drain/打包） | T1、T3（flush_batch 接线） |
| ④ reset 强制 flush（send Forced） | T3 |
| 关键性质（串行/所有权/无逐消息任务/deadline 无锁/单机制退出） | T1、T2、T3 |
| 测试范围 | T1、T3 |
| 受影响文件 | 各任务 Files |

**2. 占位符扫描**：无 TBD/TODO；`extract_text` pub(crate) 化、`weak_self` 空弱引用降级等均有明确处理说明。

**3. 类型一致性**：`BatchState::new() -> Arc<Self>`；`spawn_trigger(batch, Weak<Session>, Weak<AgentCoordinator>)`；`flush_batch(batch, session, coordinator, force)`（batching.rs 自由函数）与 `AgentCoordinator::flush_batch(session, content)`（coordinator 方法）同名但签名不同——batching 的自由函数命名改为 `flush_session_batch` 避免混淆（正文已区分：batching 的 `flush_batch` 内部调用 coordinator 的 `flush_batch`——**改为 batching 侧命名 `flush_events_to_loop`** 更清晰，正文按此调整）。
