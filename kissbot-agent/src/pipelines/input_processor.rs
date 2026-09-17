use std::{sync::{Arc, atomic::{AtomicU64, Ordering}}, time::{Duration, Instant}};

use async_trait::async_trait;
use futures_util::StreamExt;
use kissbot_api::IncomingMessageEvent;
use tokio::sync::{Notify, mpsc};
use tokio_util::time::DelayQueue;

use crate::{config_manager::ConfigManager, configs::ChannelBatchConfig, message::pack_batch, nexus::Nexus, pipeline::AgentInputProcessor, types::SessionKey};

// ===== 新合批（mpsc×2 + DelayQueue，spec 2026-08-07-channel-batching-mpsc-design）=====

/// 生产侧
struct BatchProducer {
    tx: mpsc::UnboundedSender<Arc<IncomingMessageEvent>>,
    trigger_tx: mpsc::UnboundedSender<Instant>,
    /// 编码基准：固定 Instant（所有 clone 共享）；u64 毫秒 = 相对此基准（参照 kai-ws WsHeartbeatHandler 的 anchor 方法）
    anchor: Instant,
    /// 截止时间（u64 毫秒，相对 anchor；0 = 无待 flush 哨兵）——Arc<AtomicU64> 无锁共享
    deadline: Arc<AtomicU64>,
}

impl BatchProducer {
    pub fn send(&self, event: Arc<IncomingMessageEvent>, interval: u64){
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
/// 持 session 弱引用（flush 升级用；弱引用不强持会话——会话销毁由 session_manager/channel 决定）
/// 持 notify（任务 select 等待会话销毁通知；与 Session.notify 同一 Arc，见 get_or_create 组装）
/// 持 anchor/deadline（与 producer 共享同一 Arc：enqueue 侧 set_deadline 写、任务侧 try_flush 读/清，见 get_or_create 组装）
pub struct BatchConsumer {
    rx: mpsc::UnboundedReceiver<Arc<IncomingMessageEvent>>,
    trigger_rx: mpsc::UnboundedReceiver<Instant>,
    delay: DelayQueue<Instant>,
    notify: Arc<Notify>,
    /// 编码基准（与 producer 共享同一 Arc<Instant>；try_flush 判定用，参照 kai-ws WsHeartbeatHandler 的 anchor 方法）
    anchor: Instant,
    /// 截止时间（与 producer 共享同一 Arc<AtomicU64>；0 = 无待 flush 哨兵）
    deadline: Arc<AtomicU64>,
    /// 绑定的session
    session_key: Arc<SessionKey>,
}

/// 触发 flush（BatchConsumer 成员函数）：判定（force 或 deadline 已过；内联 now_millis/deadline_passed）→
/// deadline 置 0 → drain（&mut self.rx 零锁）→ 打包（内联 pack_events）→ 经 session 弱引用升级进 agentic loop
/// 升级失败（session 已销毁）：数据仍被 drain 清走，仅丢弃打包内容（会话已不存在，无消费者）
impl BatchConsumer {
    /// 触发任务主循环（consumer 成员函数；get_or_create 经 tokio::spawn 启动）
    /// 唯一消费者（独占 &mut self 零锁）；不持 producer（anchor/deadline 经 self 内共享 Arc 访问；
    /// 退出靠 notify + trigger channel 关闭兜底）——不阻止 session drop
    async fn run(mut self) {
        loop {
            tokio::select! {
                _ = self.notify.notified() => break,  // 会话销毁（session.notify notify_one）→ 退出
                t = self.trigger_rx.recv() => {
                    match t {
                        // 按剩余时长插入（DelayQueue::insert 收 Duration；at 为 std::time::Instant）
                        Some(at) => {
                            self.delay.insert(at, at.saturating_duration_since(Instant::now()));
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
                item = self.delay.next(), if !self.delay.is_empty() => {
                    match item {
                        Some(_) => {
                            // 触发判定（内联 deadline_passed：0 = 无待 flush → false；now_millis = 相对 anchor 的 u64 毫秒）
                            let deadline = self.deadline.load(Ordering::Relaxed);
                            let now_millis = Instant::now().duration_since(self.anchor).as_millis() as u64;
                            if deadline == 0 || now_millis < deadline {
                                return;   // 未设deadline或未超 deadline：空转（等下一个到期触发）
                            }
                            // 先清 deadline 再 drain（内联 clear_deadline：store 0 = 无待 flush 哨兵）：
                            // 并发 enqueue 若在 drain 期间设新截止，不会被后续的 clear 清掉
                            // （触发判定与 clear 之间无 await，不插队）；drain 期间到达的消息并入本次 flush，
                            // 其 At 触发稍后空转——语义可接受
                            self.deadline.store(0, Ordering::Relaxed);
                            let mut items = Vec::new();
                            loop {
                                match self.rx.try_recv() {
                                    Ok(item) => items.push(item),
                                    Err(_) => break,   // Empty / Disconnected
                                }
                            }
                            if items.is_empty() {
                                return;
                            }
                            // 打包为一条 user 消息的 content（复用 message::pack_batch：extract_content + user_line + 空 content 跳过）
                            let content = pack_batch(&items);
                            // 在对应session启动流水线
                            Nexus::get().run_pipeline(self.session_key.clone(), content).await
                        },
                        None => break, // 仅防御（队列非空时 poll_next 不返回 None）
                    }
                }
            }
        }
    }
}

pub struct BatchAgentInputProcessor {
    /// 绑定的session
    session_key: Arc<SessionKey>,
    /// 合批生产侧（依赖序构造时经 create_session 传入；channel 均从本字段取 clone 绑定）
    producer: BatchProducer,
    /// 会话销毁通知（Drop 时 notify_one → trigger 任务退出；与 consumer.notify 同一 Arc）
    notify: Arc<Notify>,
}

impl BatchAgentInputProcessor {
    pub async fn new(session_key: Arc<SessionKey>) -> Self {
        // 1. notify + anchor + deadline + 2 mpsc（无依赖；各 Arc 单独建立，复制给 producer/consumer）
        let notify = Arc::new(Notify::new());
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
        // 3. 用 rx 和 session 构造 consumer（anchor/deadline/notify 均与 producer 共享同一 Arc）
        let consumer = BatchConsumer {
            rx,
            trigger_rx,
            delay: DelayQueue::new(),
            notify: notify.clone(),
            anchor,
            deadline,
            session_key: session_key.clone(),
        };
        // 4. consumer 去 spawn（内联 spawn_trigger）
        tokio::spawn(consumer.run());
        // 5. 构造 processor 实例
        Self {
            producer,
            notify,
            session_key,
        }
    }
}

#[async_trait]
impl AgentInputProcessor for BatchAgentInputProcessor {
    async fn accept(&self, event: Arc<IncomingMessageEvent>) {
        let cfg = ConfigManager::get().session_config::<ChannelBatchConfig,ChannelBatchConfig>(&self.session_key).await;
        self.producer.send(event, cfg.channel_batch_interval_secs)
    }
}

impl Drop for BatchAgentInputProcessor {
    fn drop(&mut self) {
        // 会话销毁：通知 trigger 任务退出（notify_one permit 语义：任务错过唤醒后下一轮 notified 立即完成）
        self.notify.notify_one();
    }
}
