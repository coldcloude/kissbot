// ========== Channel 合批 ==========

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use kissbot_api::channel::IncomingMessageEvent;
use tokio::sync::{mpsc, Notify};
use tokio_util::time::DelayQueue;

use crate::session_manager::Session;

// ===== 过渡 shim（Task 2/3 删除）=====
// 旧的 Mutex<Vec> 合批（BatchBuffer/pack_batch/flush_after_reset）保留到 coordinator/session
// 完成新机制接线（数据/触发双 mpsc + DelayQueue），届时一并移除，避免本任务破坏编译。

/// 每会话待合批缓冲：消息先入缓冲，超时（channel_batch_interval_secs）无新消息才打包为一条 user 消息
/// （过渡 shim：新机制为 BatchState + 数据/触发双 mpsc，Task 2/3 删除）
#[derive(Default)]
pub struct BatchBuffer {
    items: Vec<(String, String)>,  // (user_name, 文本)
}

impl BatchBuffer {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, name: &str, text: &str) {
        self.items.push((name.to_string(), text.to_string()));
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 取出全部并清空（打包用）
    pub fn take(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.items)
    }

    /// 清空缓冲（保留 API：当前重置流程不清空——重置期间消息统一并入重置后打包，见 flush_after_reset）
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// 打包为一条 user 消息的 content：逐行 "name: text"（name 为空只留 text）
/// （过渡 shim：新机制为 pack_events(&[BatchItem])，Task 2/3 删除）
pub fn pack_batch(items: &[(String, String)]) -> String {
    items.iter().map(|(name, text)| {
        if name.is_empty() { text.clone() } else { format!("{}: {}", name, text) }
    }).collect::<Vec<_>>().join("\n")
}

/// 延时打包：等待 interval 后，若会话正在重置则继续等待（重置期间不触发超时），
/// 重置完成后立即打包一次；期间到达的消息统一合并（缓冲不清空）。缓冲为空返回 None
/// （过渡 shim：新机制为 spawn_trigger + DelayQueue，Task 3 删除）
pub async fn flush_after_reset(session: &Arc<Session>, interval: Duration) -> Option<String> {
    tokio::time::sleep(interval).await;
    loop {
        let mut b = session.batch.lock().await;
        // 重置期间等待（轮询，重置通常毫秒级）
        if session.resetting.load(std::sync::atomic::Ordering::SeqCst) {
            drop(b);
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
        if b.is_empty() {
            return None;
        }
        let items = b.take();
        drop(b);
        return Some(pack_batch(&items));
    }
}

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
                item = std::future::poll_fn(|cx| delay.poll_expired(cx)) => {
                    match item {
                        Some(expired) => match expired.get_ref() {
                            Trigger::Forced => {
                                if let (Some(s), Some(c)) = (session.upgrade(), coordinator.upgrade()) {
                                    flush_events_to_loop(&batch, &s, &c, true).await;
                                }
                            }
                            Trigger::At(_) => {
                                if let (Some(s), Some(c)) = (session.upgrade(), coordinator.upgrade()) {
                                    flush_events_to_loop(&batch, &s, &c, false).await;
                                }
                            }
                        },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_batch_formats_name_content_lines() {
        let items = vec![
            ("u1".to_string(), "你好".to_string()),
            ("u2".to_string(), "在吗".to_string()),
            (String::new(), "无名字".to_string()),
        ];
        assert_eq!(pack_batch(&items), "u1: 你好\nu2: 在吗\n无名字");
    }

    #[test]
    fn batch_buffer_push_take_clear() {
        let mut b = BatchBuffer::new();
        assert!(b.is_empty());
        b.push("u1", "a");
        b.push("u2", "b");
        assert!(!b.is_empty());
        let items = b.take();
        assert_eq!(items.len(), 2);
        assert!(b.is_empty(), "take 后清空");
        b.push("u1", "c");
        b.clear();
        assert!(b.is_empty());
        assert!(b.take().is_empty());
    }

    #[tokio::test]
    async fn flush_after_reset_waits_then_packs() {
        use crate::session_manager::Session;
        use crate::types::{Mode, SessionKey};
        let key = SessionKey { agent_name: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into())));
        session.batch.lock().await.push("u1", "你好");
        // 重置期间：resetting=true，flush 不应打包
        session.resetting.store(true, std::sync::atomic::Ordering::SeqCst);
        let session2 = session.clone();
        let task = tokio::spawn(async move {
            flush_after_reset(&session2, Duration::from_millis(20)).await
        });
        // 重置期间（20ms interval 已过）：缓冲仍保留消息（未被打包）
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!session.batch.lock().await.is_empty(), "重置期间不应打包，缓冲应保留消息");
        // 重置期间到达的新消息并入缓冲
        session.batch.lock().await.push("u2", "在吗");
        // 重置完成：置 false，flush 立即打包（统一合并）
        session.resetting.store(false, std::sync::atomic::Ordering::SeqCst);
        let content = task.await.unwrap().expect("应打包");
        assert_eq!(content, "u1: 你好\nu2: 在吗", "重置期间消息统一合并");
        assert!(session.batch.lock().await.is_empty(), "打包后缓冲清空");
    }

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
}
