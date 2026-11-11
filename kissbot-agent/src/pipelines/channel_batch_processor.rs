use std::{sync::{Arc, Weak, atomic::{AtomicU64, Ordering}}, time::{Duration, Instant}};

use futures_util::StreamExt;
use kissbot_api::IncomingMessageEvent;
use tokio::sync::mpsc;
use tokio_util::time::DelayQueue;

use crate::pipeline::IncomingMessageBatchProcessor;

// ===== 新合批（mpsc×2 + DelayQueue，spec 2026-08-07-channel-batching-mpsc-design）=====

/// 生产侧
pub struct BatchProducer {
    tx: mpsc::UnboundedSender<Arc<IncomingMessageEvent>>,
    trigger_tx: mpsc::UnboundedSender<Instant>,
    /// 编码基准：固定 Instant（所有 clone 共享）；u64 毫秒 = 相对此基准（参照 kai-ws WsHeartbeatHandler 的 anchor 方法）
    anchor: Instant,
    /// 截止时间（u64 毫秒，相对 anchor；0 = 无待 flush 哨兵）——Arc<AtomicU64> 无锁共享
    deadline: Arc<AtomicU64>,
}

impl BatchProducer {
    pub fn send(&self, event: Arc<IncomingMessageEvent>, interval: u64) {
        // push message
        let _ = self.tx.send(event);
        // 设置截止时间（Instant → u64 毫秒，相对 anchor；enqueue 推数据后调用，后推覆盖）
        // 0 是「无截止」哨兵：合法截止钳到 ≥1ms（过去时间饱和为 0 时不会与哨兵碰撞，判定时必已过）
        // CAS-max：只抬不降——并发后推（deadline 更大）不被较早写入覆盖
        let at = Instant::now() + Duration::from_secs(interval);
        let duration = at.saturating_duration_since(self.anchor);
        let new = (duration.as_millis() as u64).max(1);
        let mut cur = self.deadline.load(Ordering::Relaxed);
        loop {
            if cur != 0 && new <= cur {
                break;   // 已有更晚截止（并发后推）：保持，防覆盖
            }
            match self.deadline.compare_exchange(cur, new, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(actual) => cur = actual,   // 竞争：用实际值重试
            }
        }
        // push trigger
        let _ = self.trigger_tx.send(at);
    }

}

/// 消费侧：trigger 任务独占（随 spawn move 进任务，任务内 mut 访问，零锁）
/// 持 anchor/deadline（与 producer 共享同一份：enqueue 侧 send 写 deadline，任务侧 run 读/清）；
/// 退出靠 producer 被丢弃后 trigger channel 关闭（tx/trigger_tx 均只在 producer 里）。
pub struct BatchConsumer {
    rx: mpsc::UnboundedReceiver<Arc<IncomingMessageEvent>>,
    trigger_rx: mpsc::UnboundedReceiver<Instant>,
    delay: DelayQueue<Instant>,
    /// 编码基准（与 producer 共享的 Instant；deadline 判定用，参照 kai-ws WsHeartbeatHandler 的 anchor 方法）
    anchor: Instant,
    /// 截止时间（与 producer 共享同一 Arc<AtomicU64>；0 = 无待 flush 哨兵）
    deadline: Arc<AtomicU64>,
}

/// 触发 flush（BatchConsumer 成员函数）：deadline 置 0 → drain（&mut self.rx 零锁）→
/// 交 processor.incoming_message_batch 进 agentic loop
impl BatchConsumer {
    /// 处理队列中已到达的消息（到期触发、退出前共用）
    async fn flush(&mut self) {
    }

    /// 触发任务主循环（consumer 成员函数；构造方经 tokio::spawn 启动）
    /// 唯一消费者（独占 &mut self 零锁）；不持 producer（anchor/deadline 经 self 内共享 Arc 访问）。
    /// 退出：producer 被丢弃 → trigger channel 关闭 → `recv()` 返回 None；未到期的待处理消息一并丢弃。
    pub async fn run<P: IncomingMessageBatchProcessor>(mut self, processor: Weak<P>) {
        loop {
            tokio::select! {
                t = self.trigger_rx.recv() => {
                    match t {
                        // 按剩余时长插入（DelayQueue::insert 收 Duration；at 为 std::time::Instant）
                        Some(at) => {
                            self.delay.insert(at, at.saturating_duration_since(Instant::now()));
                        }
                        None => break, // trigger channel 关闭
                    }
                }
                // DelayQueue 实现 futures_core::Stream（poll_next 委托 poll_expired）；next() 来自 StreamExt。
                // 守卫：队列空时禁用该分支——空队列时 poll_next 返回 Poll::Ready(None)（而非 Pending），
                // 守卫安全（插入必然伴随唤醒）：队列数据唯一入口是 trigger_rx 分支（唯一的 delay.insert 调用点），
                // 该分支完成即任务已醒来，下一轮 select 重新评估守卫便启用 delay 分支——不存在「队列有数据但
                // 任务 park 着、delay 分支没被启用」的状态；到期唤醒走 DelayQueue 内部 sleep 的 waker，与其他
                // 分支是否就绪无关，故 delay 分支不会被饿死，也不会错过已插入的数据。
                item = self.delay.next(), if !self.delay.is_empty() => {
                    match item {
                        Some(_) => {
                            // 触发判定（内联 deadline_passed：0 = 无待 flush → false；now_millis = 相对 anchor 的 u64 毫秒）
                            let deadline = self.deadline.load(Ordering::Relaxed);
                            let now_millis = Instant::now().duration_since(self.anchor).as_millis() as u64;
                            if deadline == 0 || now_millis < deadline {
                                continue;   // 未设deadline或未超 deadline：空转（等下一个到期触发）
                            }
                            // 先清 deadline 再 drain： 并发 enqueue 若在 drain 期间设新截止，不会被后续的 clear 清掉
                            self.deadline.store(0, Ordering::Relaxed);
                            // 处理队列中已到达的消息
                            let mut items = Vec::new();
                            loop {
                                match self.rx.try_recv() {
                                    Ok(item) => items.push(item),
                                    Err(_) => break,   // Empty / Disconnected
                                }
                            }
                            if !items.is_empty() {
                                if let Some(processor) = processor.upgrade() {
                                    processor.incoming_message_batch(items).await;
                                }
                            }
                        },
                        None => break, // 仅防御（队列非空时 poll_next 不返回 None）
                    }
                }
            }
        }
    }
}

pub fn channel_batch_processor() -> (BatchProducer, BatchConsumer) {
    // 1. anchor + deadline + 2 mpsc（无依赖；各 Arc 单独建立，复制给 producer/consumer）
    let anchor = Instant::now();
    let deadline = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::unbounded_channel();
    let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
    // 2. 用 tx 构造 producer（anchor/deadline 复制自独立 Arc）
    let producer = BatchProducer {
        tx,
        trigger_tx,
        anchor,
        deadline: deadline.clone(),
    };
    // 3. 用 rx 和 session 构造 consumer（anchor/deadline 均与 producer 共享同一 Arc）
    let consumer = BatchConsumer {
        rx,
        trigger_rx,
        delay: DelayQueue::new(),
        anchor,
        deadline,
    };
    (producer, consumer)
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use arcstr::ArcStr;
    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;
    use tokio::sync::mpsc;

    /// 记录每次 flush 的条数
    struct Recorder {
        tx: mpsc::UnboundedSender<usize>,
    }

    #[async_trait]
    impl IncomingMessageBatchProcessor for Recorder {
        async fn incoming_message_batch(self: Arc<Self>, events: Vec<Arc<IncomingMessageEvent>>) {
            let _ = self.tx.send(events.len());
        }
    }

    fn event(text: &str) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: ArcStr::from("self"),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: ArcStr::from("m"),
                messenger_id: ArcStr::from("web"),
                user_id: ArcStr::from("u"),
                group_id: ArcStr::from("g"),
                messenger_name: ArcStr::from(""),
                user_name: ArcStr::from("u"),
                group_name: ArcStr::from(""),
                content: Content::Text(ArcStr::from(text)),
                time: ArcStr::from("2026-09-23 10:00:00"),
            }),
        })
    }

    #[tokio::test]
    async fn due_batch_flushes_and_producer_drop_exits_consumer() {
        let (producer, consumer) = channel_batch_processor();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let recorder = Arc::new(Recorder { tx });
        let processor = Arc::downgrade(&recorder);
        let handle = tokio::spawn(consumer.run(processor));

        // 间隔 0：立即到期，消费者 flush 到 processor
        producer.send(event("a"), 0);
        let count = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("应触发 flush")
            .unwrap();
        assert_eq!(count, 1);

        // producer 销毁 → trigger channel 关闭 → 消费者任务退出
        drop(producer);
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("消费者应退出")
            .unwrap();
    }

    #[tokio::test]
    async fn producer_drop_exits_consumer_and_discards_pending() {
        let (producer, consumer) = channel_batch_processor();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let recorder = Arc::new(Recorder { tx });
        let processor = Arc::downgrade(&recorder);
        let handle = tokio::spawn(consumer.run(processor));

        // 消息还没到期就被解绑：丢弃待处理消息并退出（不写入已不属于它的会话）
        producer.send(event("a"), 3600);
        drop(producer);
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("消费者应退出")
            .unwrap();
        assert!(rx.try_recv().is_err(), "解绑/销毁时丢弃待处理消息");
    }
}
