# Channel 合批触发器重构设计（mpsc×2 + DelayQueue）

## 目标

将现有合批机制（`BatchBuffer: Mutex<Vec>` + 每批次 spawn sleep 任务 + `resetting` 标志）重构为：**数据队列（mpsc，元素为 `Arc<IncomingMessageEvent>`）+ 触发器队列（mpsc，携带触发时间/强制标记）+ DelayQueue（定时触发）+ 随 session 生命周期的 trigger 任务**。flush 天然串行，无 armed/CAS/resetting 等额外协调。

## 生命周期与所有权

- 两个 mpsc（数据、触发）与 DelayQueue **随 session 创建而创建、随 session 销毁而销毁**
- **trigger 任务随 session spawn**（创建时启动），持 `Arc<BatchState>`（Session 的 Arc 字段）而非 `Arc<Session>`——session 可正常 drop
- **Notify 控制任务退出**：`Session::drop` 调 `notify.notify_one()`（permit 语义：无等待者时存 permit，任务错过唤醒后下一轮 `notified()` 立即完成）→ 任务收到通知**直接 break 退出**（无需 alive 标志）

## 结构

```rust
// batching.rs —— 数据直接用 IncomingMessageEvent，不新建 BatchItem
pub type BatchItem = Arc<IncomingMessageEvent>;

/// 触发器消息：channel 发送触发时间；reset 发送强制
pub enum Trigger {
    At(Instant),   // 非强制：到期后按当前 deadline 判断（可能被后续消息延长而空转）
    Forced,        // 强制：立即 flush（上下文重置用）
}

/// 会话合批状态（Session 的一个 Arc 字段；trigger 任务持此 Arc）
pub struct BatchState {
    pub tx: mpsc::UnboundedSender<BatchItem>,                     // 数据发送端（channel 绑定会话时 clone）
    rx: tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<BatchItem>>>,   // 数据接收端（消费侧）
    pub trigger_tx: mpsc::UnboundedSender<Trigger>,               // 触发器发送端（channel/coordinator clone）
    trigger_rx: mpsc::UnboundedReceiver<Trigger>,                 // 触发器接收端（trigger 任务持有）
    delay: DelayQueue<Trigger>,                                   // 定时触发队列（trigger 任务持有）
    pub deadline: ArcSwapOption<Instant>,                         // 截止时间（无锁：推数据时 store，try_flush 时 load）
    notify: Arc<Notify>,                                          // 任务退出唤醒（session 销毁 notify_one）
}

// Session: batch: Arc<BatchState>；Session::new 里创建 BatchState 并 spawn trigger 任务
// Session Drop: batch.notify.notify_one()
// ChannelContext: batch.tx / batch.trigger_tx（绑定会话时取 clone，解绑清空）
```

依赖：新增 `tokio-util = { version = "0.7", features = ["time"] }`（DelayQueue）。

## 流程

### ① channel 收消息（enqueue，无 sleep、无逐消息任务）

```
batch.tx.send(event)                                     // 数据入队（Arc<IncomingMessageEvent>）
batch.deadline.store(Some(Arc::new(now + interval)))      // 更新截止（防抖；ArcSwapOption 无锁）
batch.trigger_tx.send(Trigger::At(now + interval))        // 直接发送触发时间（绝对），send 非阻塞
```

### ② trigger 任务（随 session 创建，唯一消费者；拥有 trigger_rx + DelayQueue 所有权）

```
loop {
    tokio::select! {
        _ = notify.notified() => break,                    // session 销毁（notify_one）→ 直接退出
        t = trigger_rx.recv() => {
            match t {
                Some(At(at)) => delay.insert(Trigger::At(at), at - now),
                Some(Forced) => delay.insert(Trigger::Forced, ZERO),
                None         => break,                     // trigger channel 关闭
            }
        }
        item = delay.next() => {
            match item {
                Some(Forced) => try_flush(force=true),
                Some(At(_))  => try_flush(force=false),
                None         => break,                     // delay 关闭
            }
        }
    }
}
```

### ③ try_flush(force)（唯一调用方 = trigger 任务 → flush 天然串行）

```
d = deadline.load_full();                                  // Option<Arc<Instant>>
if !force && (d.is_none() || now < **d) { return; }        // 非强制且未超 deadline：空转（等下一个到期触发）
items = drain rx（try_recv 循环全部；channel 不关闭，跨 flush 复用）;
deadline.store(None);
if items.is_empty() { return; }
打包：逐条 IncomingMessageEvent → user_name + extract_text(content) → "name: text" 行 → run_agentic_loop
```

### ④ 上下文重置 → 强制 flush

`reset_context` 末尾 `batch.trigger_tx.send(Trigger::Forced)` → 任务插入 delay(ZERO) → 立即弹出 → `try_flush(force=true)` 直接 drain（不检查 deadline），重置期间消息即刻并入新上下文。

## 关键性质

- **串行**：唯一 trigger 任务逐项处理（insert / try_flush），无 armed/CAS/resetting
- **所有权清晰**：trigger_rx + DelayQueue 归任务（`delay.next()` 需 &mut，无 Arc<Mutex> 包裹）；生产者（channel）只碰两个 tx（send 非阻塞）
- **无逐消息任务、无 re-arm**：触发时间即消息（At）；早期触发（被后续消息延长）弹出时 `now < deadline` 空转，延长后的新触发已由 channel 的 send 进 delay
- **deadline 无锁**：ArcSwapOption<Instant>——多写（各 channel 推数据）+ 每次触发读，原子指针交换无锁竞争
- **退出单机制**：Session::drop → notify_one()（permit 语义）→ 任务 notified() 直接 break；无 alive 标志
- **生命周期无环**：任务持 Arc<BatchState> 不持 Session；任务退出后 BatchState 随引用释放；trigger_tx 随 BatchState drop，recv() 返回 None 双保险
- **多 channel 共享会话**：同源 tx/trigger_tx clone，任一推数据/触发都进同一队列；任一推数据重置 session 级 deadline

## 边界

- session 销毁：Drop → notify_one() → 任务 break；channel 残留 send 失败忽略（warn）
- session 重建（同 key 重新绑定）：新 session → 新 BatchState + 新任务；旧任务经 notify 退出
- reset 期间的 push：数据留队 + deadline 推未来（任务不中途触发）；重置末尾 Forced 立即 flush 合并
- 无消息空闲：任务挂起在 select（轻量，随 session 生命周期）
- 多 channel 同时推数据：数据进同一队列；多个 At 触发进 delay，早期触发空转

## 测试范围（batching.rs tests）

- 数据/触发分离：tx.send 数据 + At 触发 → 任务 insert → 到期 try_flush drain 全部；channel 不关闭可复用
- 防抖：连续 send 延长 deadline（ArcSwapOption 更新），早期触发空转，最后到期 flush
- 强制：Forced → 立即 flush（无视 deadline）
- select! 并行：触发到达与到期交错均正确处理
- 退出：notify_one() 后任务 break
- 多 channel：同源 tx 推入同一队列合并消费
- deadline 无锁读写（store/load_full 语义）

## 受影响文件

- `kissbot-agent/src/batching.rs`（重构：BatchState/Trigger/try_flush/trigger 任务）
- `kissbot-agent/src/session_manager.rs`（Session.batch: Arc<BatchState>；Session::new spawn 任务；Drop 通知；删除 batch_gen/resetting 残留）
- `kissbot-agent/src/coordinator.rs`（enqueue_batch 改为 send+store+send；ChannelContext 持 tx/trigger_tx；reset_context 末尾 send Forced；删除 flush_after_reset/resetting 相关）
- `kissbot-agent/Cargo.toml`（+tokio-util）
- 相关测试、组件设计文档、`script/README.md` 验证清单同步
