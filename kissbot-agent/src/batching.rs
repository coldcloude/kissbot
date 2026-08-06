// ========== Channel 合批 ==========

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use kissbot_api::channel::IncomingMessageEvent;
use tokio::sync::{mpsc, Notify};
use tokio_util::time::DelayQueue;

use crate::session_manager::Session;

// ===== 新合批（mpsc×2 + DelayQueue，spec 2026-08-07-channel-batching-mpsc-design）=====

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
    /// 触发器运行态（trigger_rx + DelayQueue）：随 spawn_trigger 取走移入任务（std Mutex 短暂持有，无 await）
    trigger_runtime: std::sync::Mutex<Option<(mpsc::UnboundedReceiver<Trigger>, DelayQueue<Trigger>)>>,
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
            trigger_runtime: std::sync::Mutex::new(Some((trigger_rx, DelayQueue::new()))),
            deadline: ArcSwapOption::new(None),
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

    /// 取走触发器运行态（trigger_rx + DelayQueue），随 spawn_trigger 移入任务；仅调用一次
    pub fn take_trigger_runtime(&self) -> Option<(mpsc::UnboundedReceiver<Trigger>, DelayQueue<Trigger>)> {
        self.trigger_runtime.lock().unwrap().take()
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
        Some(d) => Instant::now() >= *d,
    }
}

/// 触发任务：随 session 创建 spawn；唯一消费者（trigger_rx + DelayQueue 所有权）
/// 持 Arc<BatchState> + Weak<Session> + Weak<Coordinator>——不阻止 session drop
pub fn spawn_trigger(
    batch: Arc<BatchState>,
    session: Weak<Session>,
    coordinator: Weak<crate::coordinator::AgentCoordinator>,
) {
    let Some((mut trigger_rx, mut delay)) = batch.take_trigger_runtime() else {
        tracing::warn!("spawn_trigger: 触发器运行态已被取走，跳过");
        return;
    };
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = batch.notify.notified() => break,      // session 销毁（notify_one）→ 退出
                t = trigger_rx.recv() => {
                    match t {
                        // 按剩余时长插入（DelayQueue::insert 收 Duration；at 为 std::time::Instant）
                        Some(Trigger::At(at)) => {
                            delay.insert(Trigger::At(at), at.saturating_duration_since(Instant::now()));
                        }
                        // 强制：立即到期
                        Some(Trigger::Forced) => {
                            delay.insert(Trigger::Forced, Duration::ZERO);
                        }
                        None => break,                      // trigger channel 关闭
                    }
                }
                // DelayQueue 无固有 next()（next 来自 StreamExt）；用 poll_fn + poll_expired
                // 守卫：队列空时禁用该分支——poll_expired 在空队列返回 Poll::Ready(None)（而非 Pending），
                // 无守卫会在 spawn 首轮（队列空）命中 None => break 使任务立即退出（合批失效）
                item = std::future::poll_fn(|cx| delay.poll_expired(cx)), if !delay.is_empty() => {
                    match item {
                        Some(expired) => match expired.get_ref() {
                            Trigger::Forced => {
                                flush_events_to_loop(&batch, session.clone(), coordinator.clone(), true).await;
                            }
                            Trigger::At(_) => {
                                flush_events_to_loop(&batch, session.clone(), coordinator.clone(), false).await;
                            }
                        },
                        None => break,                      // 仅防御（队列非空时 poll_expired 不返回 None）
                    }
                }
            }
        }
    });
}

/// 触发 flush：判定 → drain 全部 → 打包 → 升级 Weak 后交协调器进 agentic loop
/// （命名 flush_events_to_loop 与 coordinator 的 flush_batch 方法区分）
/// 升级失败（session/coordinator 已销毁）：数据仍被 drain 清走，仅丢弃打包内容（会话已不存在，无消费者）
pub async fn flush_events_to_loop(
    batch: &Arc<BatchState>,
    session: Weak<Session>,
    coordinator: Weak<crate::coordinator::AgentCoordinator>,
    force: bool,
) {
    if !flush_ready(batch, force) {
        return;   // 非强制且未超 deadline：空转（等下一个到期触发）
    }
    // 先清 deadline 再 drain：并发 enqueue 若在 drain 期间设新截止，不会被后续的 store(None)
    // 清掉（flush_ready 与 store(None) 之间无 await，不插队）；drain 期间到达的消息并入本次 flush，
    // 其 At 触发稍后空转——语义可接受
    batch.deadline.store(None);
    let items = batch.drain().await;
    if items.is_empty() {
        return;
    }
    // 升级 Weak：失败则数据已 drain（清走），打包内容丢弃
    let (Some(s), Some(c)) = (session.upgrade(), coordinator.upgrade()) else {
        return;
    };
    let content = pack_events(&items);
    c.flush_batch(&s, content).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== 新合批（mpsc×2 + DelayQueue）=====

    use kissbot_api::channel::{IncomingMessage, IncomingMessageEvent};
    use kissbot_api::message::Content;

    fn ev(name: &str, text: &str) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: Arc::new("self".into()),
            incoming_message: Arc::new(IncomingMessage {
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
        let items = batch.drain().await;
        assert_eq!(items.len(), 2);
        // channel 未关闭：再次 send 可消费（跨 flush 复用）
        batch.tx.send(ev("u3", "c")).unwrap();
        assert_eq!(batch.drain().await.len(), 1);
    }

    #[test]
    fn flush_ready_respects_deadline_and_force() {
        let batch = BatchState::new();
        // 无 deadline：非强制不 flush
        assert!(!flush_ready(&batch, false));
        // 未超 deadline：非强制不 flush；强制无视 deadline
        batch.deadline.store(Some(Arc::new(Instant::now() + Duration::from_secs(10))));
        assert!(!flush_ready(&batch, false), "未超时非强制不应 flush");
        assert!(flush_ready(&batch, true), "强制应 flush");
        // 已超 deadline：非强制 flush
        batch.deadline.store(Some(Arc::new(Instant::now() - Duration::from_secs(1))));
        assert!(flush_ready(&batch, false), "超时非强制应 flush");
    }

    // ===== trigger 循环测试（回归网：拦住空队列立即退出缺陷）=====

    /// 等待任务执行 flush：flush_events_to_loop 的 flush_ready 通过后先 store(None) 再 drain，
    /// 观测 deadline 变 None 即确认 flush 路径已执行（不自行 drain，避免把未处理数据误判为已处理）
    async fn wait_flushed(batch: &Arc<BatchState>) -> bool {
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if batch.deadline.load_full().is_none() {
                // 任务已执行 store(None)；其 drain 可能仍在进行，稍候让数据被清走
                tokio::time::sleep(Duration::from_millis(30)).await;
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn spawn_trigger_flushes_on_at_trigger() {
        let batch = BatchState::new();
        batch.tx.send(ev("u1", "a")).unwrap();
        batch.tx.send(ev("u2", "b")).unwrap();
        // 与真实 enqueue 一致：deadline 与 At 触发时间同源（此处设为过去，到期立即弹出）
        let at = Instant::now() - Duration::from_secs(1);
        batch.deadline.store(Some(Arc::new(at)));
        batch.trigger_tx.send(Trigger::At(at)).unwrap();
        spawn_trigger(batch.clone(), Weak::new(), Weak::new());
        assert!(wait_flushed(&batch).await, "At 触发后任务应执行 flush（deadline 置 None）");
        assert!(batch.drain().await.is_empty(), "At 触发后任务应 drain 全部数据");
    }

    #[tokio::test]
    async fn spawn_trigger_flushes_on_forced() {
        let batch = BatchState::new();
        batch.tx.send(ev("u1", "a")).unwrap();
        // 设过去 deadline 作为 flush 执行观测点（force 路径同样先 store(None) 再 drain）
        batch.deadline.store(Some(Arc::new(Instant::now() - Duration::from_secs(1))));
        batch.trigger_tx.send(Trigger::Forced).unwrap();
        spawn_trigger(batch.clone(), Weak::new(), Weak::new());
        assert!(wait_flushed(&batch).await, "Forced 触发后任务应执行 flush（deadline 置 None）");
        assert!(batch.drain().await.is_empty(), "Forced 触发后任务应 drain 全部数据");
    }

    #[tokio::test]
    async fn spawn_trigger_exits_on_notify() {
        let batch = BatchState::new();
        spawn_trigger(batch.clone(), Weak::new(), Weak::new());
        // 确保任务已启动并持有 trigger_rx
        tokio::time::sleep(Duration::from_millis(20)).await;
        batch.notify.notify_one();
        // 任务退出后 trigger_rx 已 drop → channel 关闭 → send 返回 Err
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if batch.trigger_tx.send(Trigger::Forced).is_err() {
                break;
            }
            assert!(Instant::now() < deadline, "任务应在 notify 后退出");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
