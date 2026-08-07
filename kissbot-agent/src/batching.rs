// ========== Channel 合批 ==========

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
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

impl BatchProducer {
    /// 设置截止时间（Instant → u64 毫秒，相对 anchor；enqueue 推数据后调用，后推覆盖）
    /// 0 是「无截止」哨兵：合法截止钳到 ≥1ms（过去时间饱和为 0 时不会与哨兵碰撞，判定时必已过）
    /// CAS-max：只抬不降——并发后推（deadline 更大）不被较早写入覆盖
    pub fn set_deadline(&self, at: Instant) {
        let new = at.saturating_duration_since(*self.anchor).as_millis() as u64;
        let new = new.max(1);
        let mut cur = self.deadline.load(Ordering::Relaxed);
        loop {
            if cur != 0 && new <= cur {
                return;   // 已有更晚截止（并发后推）：保持，防覆盖
            }
            match self.deadline.compare_exchange(cur, new, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return,
                Err(actual) => cur = actual,   // 竞争：用实际值重试
            }
        }
    }

    /// 截止时间是否已清（测试观测 flush 执行用）
    #[cfg(test)]
    pub fn deadline_cleared(&self) -> bool {
        self.deadline.load(Ordering::Relaxed) == 0
    }
}

/// 消费侧：trigger 任务独占（随 spawn move 进任务，任务内 mut 访问，零锁）
/// 持 session 弱引用（flush 升级用；弱引用不强持会话——会话销毁由 session_manager/channel 决定）
/// 持 notify（任务 select 等待会话销毁通知；与 Session.notify 同一 Arc，见 get_or_create 组装）
/// 持 anchor/deadline（与 producer 共享同一 Arc：enqueue 侧 set_deadline 写、任务侧 try_flush 读/清，见 get_or_create 组装）
pub struct BatchConsumer {
    rx: mpsc::UnboundedReceiver<BatchItem>,
    trigger_rx: mpsc::UnboundedReceiver<Trigger>,
    delay: DelayQueue<Trigger>,
    session: Weak<Session>,
    notify: Arc<Notify>,
    /// 编码基准（与 producer 共享同一 Arc<Instant>；try_flush 判定用，参照 kai-ws WsHeartbeatHandler 的 anchor 方法）
    anchor: Arc<Instant>,
    /// 截止时间（与 producer 共享同一 Arc<AtomicU64>；0 = 无待 flush（原 None）哨兵）
    deadline: Arc<AtomicU64>,
}

impl BatchConsumer {
    /// 组装消费侧（依赖序：session 已建）；仅 session_manager.get_or_create 调用
    /// anchor/deadline 从 producer 复制（同一 Arc，无锁共享状态）
    pub fn new(
        rx: mpsc::UnboundedReceiver<BatchItem>,
        trigger_rx: mpsc::UnboundedReceiver<Trigger>,
        session: &Arc<Session>,
        notify: Arc<Notify>,
        producer: &BatchProducer,
    ) -> Self {
        Self {
            rx,
            trigger_rx,
            delay: DelayQueue::new(),
            session: Arc::downgrade(session),
            notify,
            anchor: producer.anchor.clone(),
            deadline: producer.deadline.clone(),
        }
    }
}

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

/// 打包为一条 user 消息的 content：逐行 "name: text"（name 为空只留 text）
pub fn pack_events(events: &[BatchItem]) -> String {
    events.iter().map(|e| {
        let name = e.incoming_message.user_name.as_str();
        let text = crate::coordinator::extract_text(&e.incoming_message.content);
        if name.is_empty() { text } else { format!("{}: {}", name, text) }
    }).collect::<Vec<_>>().join("\n")
}

/// 触发任务：随 session_manager.get_or_create 调用（consumer 创建后立即移入）；唯一消费者（独占 &mut consumer，零锁）
/// 不持 producer（anchor/deadline 经 consumer 内共享 Arc 访问；退出靠 notify + trigger channel 关闭兜底）——不阻止 session drop
pub fn spawn_trigger(mut consumer: BatchConsumer) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = consumer.notify.notified() => break,  // 会话销毁（session.notify notify_one）→ 退出
                t = consumer.trigger_rx.recv() => {
                    match t {
                        // 按剩余时长插入（DelayQueue::insert 收 Duration；at 为 std::time::Instant）
                        Some(Trigger::At(at)) => {
                            consumer.delay.insert(Trigger::At(at), at.saturating_duration_since(Instant::now()));
                        }
                        // 强制：立即到期
                        Some(Trigger::Forced) => {
                            consumer.delay.insert(Trigger::Forced, Duration::ZERO);
                        }
                        None => break,                      // trigger channel 关闭
                    }
                }
                // DelayQueue 实现 futures_core::Stream（poll_next 委托 poll_expired）；next() 来自 StreamExt。
                // 守卫：队列空时禁用该分支——空队列时 poll_next 返回 Poll::Ready(None)（而非 Pending），
                // 守卫安全（插入必然伴随唤醒）：队列数据唯一入口是 trigger_rx 分支（唯一的 delay.insert 调用点），
                // 该分支完成即任务已醒来，下一轮 select 重新评估守卫便启用 delay 分支——不存在「队列有数据但
                // 任务 park 着、delay 分支没被启用」的状态；到期唤醒走 DelayQueue 内部 sleep 的 waker，与其他
                // 分支是否就绪无关，故 delay 分支不会被饿死，也不会错过已插入的数据。
                item = consumer.delay.next(), if !consumer.delay.is_empty() => {
                    match item {
                        Some(expired) => match expired.get_ref() {
                            Trigger::Forced => try_flush(&mut consumer, true).await,
                            Trigger::At(_) => try_flush(&mut consumer, false).await,
                        },
                        None => break,                      // 仅防御（队列非空时 poll_next 不返回 None）
                    }
                }
            }
        }
    });
}

/// 触发 flush：判定（force 或 deadline 已过；内联 now_millis/deadline_passed）→ deadline 置 0 → drain
/// （&mut consumer.rx 零锁）→ 打包 → 经 consumer.session 弱引用升级进 agentic loop
/// 升级失败（session 已销毁）：数据仍被 drain 清走，仅丢弃打包内容（会话已不存在，无消费者）
pub async fn try_flush(consumer: &mut BatchConsumer, force: bool) {
    // 触发判定（内联 deadline_passed：0 = 无待 flush → false；now_millis = 相对 anchor 的 u64 毫秒）
    let deadline = consumer.deadline.load(Ordering::Relaxed);
    let now_millis = Instant::now().duration_since(*consumer.anchor).as_millis() as u64;
    if !(force || (deadline != 0 && now_millis >= deadline)) {
        return;   // 非强制且未超 deadline：空转（等下一个到期触发）
    }
    // 先清 deadline 再 drain（内联 clear_deadline：store 0 = 无待 flush 哨兵）：
    // 并发 enqueue 若在 drain 期间设新截止，不会被后续的 clear 清掉
    // （触发判定与 clear 之间无 await，不插队）；drain 期间到达的消息并入本次 flush，
    // 其 At 触发稍后空转——语义可接受
    consumer.deadline.store(0, Ordering::Relaxed);
    let mut items = Vec::new();
    loop {
        match consumer.rx.try_recv() {
            Ok(item) => items.push(item),
            Err(_) => break,   // Empty / Disconnected
        }
    }
    if items.is_empty() {
        return;
    }
    // 任务持 consumer.session 弱引用升级会话（失败 = 会话已销毁，数据已 drain 清走，仅丢弃打包内容）
    let Some(session) = consumer.session.upgrade() else {
        return;
    };
    let content = pack_events(&items);
    session.accept_batch(content).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== 新合批（mpsc×2 + DelayQueue，归属拆分：生产侧共享无锁 / 消费侧任务独占零锁）=====

    use kissbot_api::channel::{IncomingMessage, IncomingMessageEvent};
    use kissbot_api::message::Content;
    use crate::types::{Mode, SessionKey};

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

    /// 测试 producer/consumer 对（未 spawn；consumer 持测试会话弱引用 + 会话 notify）
    fn test_pair() -> (BatchProducer, BatchConsumer, Arc<Session>) {
        let key = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let notify = Arc::new(Notify::new());
        let (producer, rx, trigger_rx) = new_producer();
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into()), Weak::new(), producer.clone(), notify.clone()));
        let consumer = BatchConsumer::new(rx, trigger_rx, &session, notify, &producer);
        (producer, consumer, session)
    }

    #[test]
    fn pack_events_builds_name_content_lines() {
        let events = vec![ev("u1", "你好"), ev("u2", "在吗")];
        assert_eq!(pack_events(&events), "u1: 你好\nu2: 在吗");
    }

    #[tokio::test]
    async fn producer_consumer_pairs_channels() {
        // 生产侧/消费侧同源成对；消费侧直接 &mut 取（零锁），channel 不关闭跨 flush 复用
        let (producer, mut consumer, _session) = test_pair();
        producer.tx.send(ev("u1", "a")).unwrap();
        let item = consumer.rx.try_recv().unwrap();
        assert!(item.incoming_message.user_name.as_str() == "u1");
        assert!(consumer.rx.try_recv().is_err(), "仅一条");
        // channel 未关闭：再次 send 可消费
        producer.tx.send(ev("u2", "b")).unwrap();
        assert!(consumer.rx.try_recv().is_ok(), "再次 send 仍可消费");
    }

    #[tokio::test]
    async fn try_flush_drains_consumer_without_lock() {
        // 已超 deadline：非强制 flush → drain 全部（会话弱引用升级失败或 accept_batch 返回 → 丢弃，drain 可观测）
        let (producer, mut consumer, _session) = test_pair();
        producer.tx.send(ev("u1", "a")).unwrap();
        producer.tx.send(ev("u2", "b")).unwrap();
        // 过去 deadline：anchor 编码下饱和为 1ms，sleep 保证 now_millis > 1（判定必已过）
        producer.set_deadline(Instant::now() - Duration::from_secs(1));
        tokio::time::sleep(Duration::from_millis(10)).await;
        try_flush(&mut consumer, false).await;
        assert!(consumer.rx.try_recv().is_err(), "已 drain");
        // 未超 deadline：不 drain
        let (p2, mut c2, _) = test_pair();
        p2.tx.send(ev("u1", "x")).unwrap();
        p2.set_deadline(Instant::now() + Duration::from_secs(10));
        try_flush(&mut c2, false).await;
        assert!(c2.rx.try_recv().is_ok(), "未超时不应 drain");
    }


    // ===== trigger 循环测试（回归网：拦住空队列立即退出缺陷）=====

    /// 等待任务执行 flush：try_flush 判定通过后先 clear_deadline 再 drain，
    /// 观测 deadline 变 0 即确认 flush 路径已执行（consumer 已移入任务，不做二次 drain）
    async fn wait_flushed(producer: &BatchProducer) -> bool {
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if producer.deadline_cleared() {
                tokio::time::sleep(Duration::from_millis(30)).await;
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn spawn_trigger_flushes_on_at_trigger() {
        let (producer, consumer, _session) = test_pair();
        producer.tx.send(ev("u1", "a")).unwrap();
        producer.tx.send(ev("u2", "b")).unwrap();
        // 与真实 enqueue 一致：deadline 与 At 触发时间同源（此处设为过去，到期立即弹出）
        let at = Instant::now() - Duration::from_secs(1);
        producer.set_deadline(at);
        producer.trigger_tx.send(Trigger::At(at)).unwrap();
        spawn_trigger(consumer);
        assert!(wait_flushed(&producer).await, "At 触发后任务应执行 flush（deadline 置 None）");
    }

    #[tokio::test]
    async fn spawn_trigger_flushes_on_forced() {
        let (producer, consumer, _session) = test_pair();
        producer.tx.send(ev("u1", "a")).unwrap();
        // 设过去 deadline 作为 flush 执行观测点（force 路径同样先 clear_deadline 再 drain）
        producer.set_deadline(Instant::now() - Duration::from_secs(1));
        producer.trigger_tx.send(Trigger::Forced).unwrap();
        spawn_trigger(consumer);
        assert!(wait_flushed(&producer).await, "Forced 触发后任务应执行 flush（deadline 置 None）");
    }

    #[tokio::test]
    async fn spawn_trigger_exits_on_notify() {
        let (producer, consumer, session) = test_pair();
        spawn_trigger(consumer);
        // 确保任务已启动并持有 trigger_rx
        tokio::time::sleep(Duration::from_millis(20)).await;
        session.notify.notify_one();
        // 任务退出后 trigger_rx 已 drop → channel 关闭 → send 返回 Err
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if producer.trigger_tx.send(Trigger::Forced).is_err() {
                break;
            }
            assert!(Instant::now() < deadline, "任务应在 notify 后退出");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
