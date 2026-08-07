# Channel 合批引用重构设计（无 OnceLock / 依赖序构造）

日期：2026-08-07
状态：已确认（brainstorming 交互定稿）

## 背景与目标

当前合批的 session/coordinator/producer 引用关系存在两处「先创建后设置」的 OnceLock 延迟注入：

1. `BatchProducer` 持两个升级槽 `OnceLock<Weak<Session>>` / `OnceLock<Weak<AgentCoordinator>>`，由 `ensure_session` 事后设置；
2. `AgentCoordinator` 持 `weak_self: OnceLock<Weak<Self>>`，供槽使用。

根因：`Session::new` 在 coordinator 还不存在时 spawn 合批任务，coordinator/session 引用只能后补。

本次重构改为**依赖序构造**：先建 notify + 2 个 mpsc（无依赖）→ 用 tx 构造 Producer → 用 Producer 构造 Session → 用 rx + session 构造 Consumer → Consumer 去 spawn → 返回 Session。全程无 OnceLock、无后置设置。

与最初「producer 从 session 拆出」的想法不同，最终定案：**Session 持有 Producer**（强引用）。Channel 均从 session 取 producer。

## 对象与引用（谁持谁）

- `BatchProducer { tx, trigger_tx, anchor, deadline }`
  - **删除**：两个 `OnceLock` 升级槽（`coordinator` / `session`）、`notify`
  - 字段全为 Clone/Arc（`UnboundedSender` ×2、`Arc<Instant>`、`Arc<AtomicU64>`），可自由 Clone
- `BatchConsumer { rx, trigger_rx, delay, session: Weak<Session>, notify }`
  - 任务独占（move 进 spawn）；**全流程唯一持 Session 弱引用处**
  - `notify` 供任务 select 等待退出（从 session 取会强持 session 造成环，故由 consumer 直接持有）
- `Session { coordinator: Weak<AgentCoordinator>, batch: BatchProducer, notify: Arc<Notify>, agent_name, role_name, mode, context, model, agent_id }`
  - 持 producer（强）；coordinator 弱引用（破环：coordinator → session_manager → session → coordinator）
  - `Drop`：`self.notify.notify_one()`（会话销毁 → 任务退出，语义与旧版一致）
- `ChannelContext { producer: ArcSwapOption<BatchProducer>, ... }`
  - **替代**原 `batch_tx` / `trigger_tx` 两个 `Mutex<Option<...>>` 槽
  - `BatchProducer` 内部全 Arc/原子，无需锁；`ArcSwapOption` 与同结构 `agent_id` 模式一致
  - 绑定：`ctx.producer.store(Some(session.batch.clone()))`；读取：`(*ctx.producer.load_full()).clone()`
- `AgentCoordinator`：**删除** `weak_self` 字段

## 构建顺序（get_or_create，全依赖序）

```
① notify = Arc::new(Notify::new())
② (tx, rx) 数据 mpsc + (trigger_tx, trigger_rx) 触发 mpsc        ← 无额外依赖
③ producer = BatchProducer { tx, trigger_tx, anchor, deadline } ← 用 tx 构造
④ session = Session::new(key, model, agent_id, coordinator_weak,
                          producer, notify.clone())             ← 用 producer 构造
⑤ consumer = BatchConsumer { rx, trigger_rx, delay,
                              session: Arc::downgrade(&session), notify } ← 用 rx + session 构造
⑥ spawn_trigger(session.batch.clone(), consumer)                ← consumer 去 spawn
⑦ 返回 session；channel 绑定 ctx.producer = Some(session.batch.clone())
```

- 构造在 `session_manager.get_or_create` 内完成（新增参数 `coordinator: Weak<AgentCoordinator>`）；`get_or_create` 保持同步（tokio::spawn 无需 async）
- `Session::new` 签名：`(key, model, agent_id, coordinator: Weak<AgentCoordinator>, batch: BatchProducer, notify: Arc<Notify>)`
- `ensure_session` 每次调用都绑定 channel（created 与非 created 皆幂等绑定同一 session 的 producer；多 channel 共享 session 时共享同一 producer 对）；created 分支另做 `build_initial_context`
- 懒绑定保留：enqueue 发现 `ctx.producer` 为空时从 session.batch 补绑

## flush 链（任务 → session.accept_batch）

```
任务 select（consumer.notify 退出 / trigger_rx 收 At/Forced / delay 到期）
  → try_flush(producer, consumer, force)
  → flush_ready(producer, force) 判定
  → producer.clear_deadline() → drain consumer.rx
  → consumer.session.upgrade()（失败则数据已 drain，丢弃）
  → pack_events → session.accept_batch(content)
  → session.model 检查 → coordinator 弱引用 upgrade
  → coordinator.resolve_out_channel_for_session → coordinator.run_agentic_loop
```

- **删除** `coordinator.flush_batch`，改由 `Session::accept_batch` 承担
- `run_agentic_loop` / `resolve_out_channel_for_session` 改 `pub(crate)`（session_manager 调用）

## reset 路径（S 定案）

`reset_context` 内 `session.batch.trigger_tx.send(Forced)`：
- admin `/reset`（`reset_session_for(channel_id)`，任务外）与 role 溢出（`run_agentic_loop` 内，任务内）两路径统一
- 行为与旧版一致（Forced 入队 → 任务下一轮 select 立即 flush）

## Arc<Self> 链（A 定案）

仅必需链改 `self: Arc<Self>`（内部需 `Arc::downgrade(&self)` 传 session）：
- `ensure_session`、`handle_incoming`、`relocate_channel`、`apply_channel_key`
- `connect_channels` 已是 `self: &Arc<Self>`，不变
- 其余方法保持 `&self`（`Arc<Self>` 可自动解引用调用，无需全改）

## 测试适配

- `BatchProducer` 可独立构造（无 session 依赖）→ flush_ready 类用例直接 `BatchProducer { ... }`
- 需 session 的用例（consumer 构造、spawn_trigger 系列、try_flush 升级）改走 `session_manager.get_or_create`，coordinator 传 `Weak::new()`
- `session_manager` 测试同步新签名（get_or_create 新增 coordinator 参数；Session::new 新增 producer/notify 参数）
- 回归网保留：C1（空队列守卫）、deadline 语义、notify 退出
