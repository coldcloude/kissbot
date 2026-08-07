use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use futures_util::StreamExt;
use kissbot_api::channel::IncomingMessageEvent;
use tokio::sync::{mpsc, Notify};
use tokio_util::time::DelayQueue;
use tracing::warn;

use crate::config_manager::ProviderModel;
use crate::coordinator::{AgentCoordinator, extract_text};
use crate::types::{Message, Mode, SessionKey};

/// 会话上下文：纯内存消息序列 + system 消息（缓存/历史持久化由 coordinator 负责）
pub struct SessionContext {
    messages: Vec<Message>,
    system_message: Option<String>,
}

impl SessionContext {
    pub fn new() -> Self {
        Self { messages: Vec::new(), system_message: None }
    }

    /// 设置系统消息（会话创建或重置时）
    pub fn set_system_message(&mut self, content: String) {
        self.system_message = Some(content);
    }

    /// 取系统消息（压缩/恢复用；当前调用方经 build() 内部读取，保留供后续使用）
    #[allow(dead_code)]
    pub fn system_message(&self) -> Option<&str> {
        self.system_message.as_deref()
    }

    /// 从缓存/记忆加载历史消息重建上下文（system 之外的部分）
    pub fn load_messages(&mut self, messages: Vec<Message>) {
        self.messages.clear();
        self.messages = messages;
    }

    /// 追加一条消息
    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// 构建模型消息列表（system 在最前）
    pub fn build(&self) -> Vec<Message> {
        let mut items = Vec::new();
        if let Some(system) = &self.system_message {
            items.push(Message::System { content: Arc::new(system.clone()) });
        }
        items.extend(self.messages.iter().cloned());
        items
    }

    /// 消息条数（不含 system；is_overflow 内部直接算，保留供调用方读取）
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// 检查是否超长（threshold 来自模型 effective 配置的 max_context_messages）
    pub fn is_overflow(&self, max: usize) -> bool {
        self.messages.len() >= max
    }

    /// 清空上下文（重置时调用；system 保留）
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

/// 单个会话：独立上下文、模型与模式状态
pub struct Session {
    pub agent_name: Arc<String>,    // 运行态：从 key 复制（context 配置查找用）
    pub role_name: Arc<String>,     // 运行态：从 key 复制（身份读取源；SessionKey 仅作去重，不存于 Session）
    pub mode: Arc<Mode>,            // 运行态：从 key 复制
    pub context: tokio::sync::Mutex<SessionContext>,
    /// 合批生产侧（依赖序构造时经 create_session 传入；channel 均从本字段取 clone 绑定）
    pub batch_producer: BatchProducer,
    /// 会话级模型（创建时取 default_model，/model 调整）；None = 无模型（普通消息静默忽略）
    pub model: ArcSwap<Option<ProviderModel>>,
    /// 会话状态保存的 agent_id（UUID；创建时取自触发 channel 的运行态绑定，之后不变）
    /// 取记忆/ego 一律用本字段（agent_name 仅作 context 配置查找，不参与记忆/ego 定位）
    pub agent_id: Arc<String>,
    /// coordinator 弱引用（accept_batch 升级调用 run_agentic_loop/out_channel 解析；弱引用破环：
    /// coordinator → session_manager → session → coordinator 会形成强环）
    coordinator: Weak<AgentCoordinator>,
    /// 会话销毁通知（Drop 时 notify_one → trigger 任务退出；与 consumer.notify 同一 Arc）
    pub notify: Arc<Notify>,
}

impl Session {
    pub fn new(
        key: &SessionKey,
        model: Option<ProviderModel>,
        agent_id: Arc<String>,
        coordinator: Weak<AgentCoordinator>,
        batch_producer: BatchProducer,
        notify: Arc<Notify>,
    ) -> Self {
        Self {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new()),
            batch_producer,
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

// ===== 新合批（mpsc×2 + DelayQueue，spec 2026-08-07-channel-batching-mpsc-design；原 batching.rs 迁入，属 session 功能）=====

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

/// 触发 flush（BatchConsumer 成员函数）：判定（force 或 deadline 已过；内联 now_millis/deadline_passed）→
/// deadline 置 0 → drain（&mut self.rx 零锁）→ 打包（内联 pack_events）→ 经 session 弱引用升级进 agentic loop
/// 升级失败（session 已销毁）：数据仍被 drain 清走，仅丢弃打包内容（会话已不存在，无消费者）
impl BatchConsumer {
    /// 触发任务主循环（原 spawn_trigger 的 spawn 内部分，改 consumer 成员函数；get_or_create 经 tokio::spawn 启动）
    /// 唯一消费者（独占 &mut self 零锁）；不持 producer（anchor/deadline 经 self 内共享 Arc 访问；
    /// 退出靠 notify + trigger channel 关闭兜底）——不阻止 session drop
    async fn run(mut self) {
        loop {
            tokio::select! {
                _ = self.notify.notified() => break,  // 会话销毁（session.notify notify_one）→ 退出
                t = self.trigger_rx.recv() => {
                    match t {
                        // 按剩余时长插入（DelayQueue::insert 收 Duration；at 为 std::time::Instant）
                        Some(Trigger::At(at)) => {
                            self.delay.insert(Trigger::At(at), at.saturating_duration_since(Instant::now()));
                        }
                        // 强制：立即到期
                        Some(Trigger::Forced) => {
                            self.delay.insert(Trigger::Forced, Duration::ZERO);
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
                        Some(expired) => match expired.get_ref() {
                            Trigger::Forced => self.try_flush(true).await,
                            Trigger::At(_) => self.try_flush(false).await,
                        },
                        None => break,                      // 仅防御（队列非空时 poll_next 不返回 None）
                    }
                }
            }
        }
    }

    pub async fn try_flush(&mut self, force: bool) {
        // 触发判定（内联 deadline_passed：0 = 无待 flush → false；now_millis = 相对 anchor 的 u64 毫秒）
        let deadline = self.deadline.load(Ordering::Relaxed);
        let now_millis = Instant::now().duration_since(*self.anchor).as_millis() as u64;
        if !(force || (deadline != 0 && now_millis >= deadline)) {
            return;   // 非强制且未超 deadline：空转（等下一个到期触发）
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
        // 任务持 session 弱引用升级会话（失败 = 会话已销毁，数据已 drain 清走，仅丢弃打包内容）
        let Some(session) = self.session.upgrade() else {
            return;
        };
        // 打包为一条 user 消息的 content（内联 pack_events）：逐行 "name: text"（name 为空只留 text）
        let content = items.iter().map(|e| {
            let name = e.incoming_message.user_name.as_str();
            let text = extract_text(&e.incoming_message.content);
            if name.is_empty() { text } else { format!("{}: {}", name, text) }
        }).collect::<Vec<_>>().join("\n");
        session.accept_batch(content).await;
    }
}

/// 会话管理器：汇总所有绑定 channel 的 (agent_name, role_name, mode) 去重维护会话集合
/// （session_key 仅用于去重；agent_id 解析结果由各 channel 运行态绑定保存，不在此提取）
pub struct SessionManager {
    sessions: DashMap<SessionKey, Arc<Session>>,
}

impl SessionManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
        })
    }

    /// 按 key 取会话
    pub fn get(&self, key: &SessionKey) -> Option<Arc<Session>> {
        self.sessions.get(key).map(|e| e.value().clone())
    }

    /// 定位会话，不存在则创建（model 为初始模型，None = 无模型；agent_id 为会话状态保存的解析结果；
    /// coordinator 弱引用由 Arc 链调用方降级传入）；返回 (会话, 是否新建)
    /// 创建时依赖序组装（内联 new_producer/BatchConsumer::new）：notify → 2 mpsc → producer → session → consumer → spawn
    /// （channel 均从 session.batch_producer 取 clone；任务持 consumer，consumer 持 session 弱引用与 notify，
    ///  anchor/deadline/notify 均为独立 Arc——producer 与 consumer 共享同一份）
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
                // 创建部分抽出（create_session）：依赖序组装 + spawn 触发任务
                let session = Self::create_session(key, model, agent_id, coordinator);
                e.insert(session.clone());
                (session, true)
            }
        }
    }

    /// 创建会话（get_or_create 的 created 分支抽出）：依赖序组装（内联 new_producer/BatchConsumer::new）+
    /// spawn 触发任务（内联 spawn_trigger：tokio::spawn(consumer.run())）；返回新建会话
    /// （channel 均从 session.batch_producer 取 clone；任务持 consumer，consumer 持 session 弱引用与 notify，
    ///  anchor/deadline/notify 均为独立 Arc——producer 与 consumer 共享同一份）
    fn create_session(
        key: &SessionKey,
        model: Option<ProviderModel>,
        agent_id: Arc<String>,
        coordinator: Weak<AgentCoordinator>,
    ) -> Arc<Session> {
        // 1. notify + anchor + deadline + 2 mpsc（无依赖；各 Arc 单独建立，复制给 producer/consumer）
        let notify = Arc::new(Notify::new());
        let anchor = Arc::new(Instant::now());
        let deadline = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::unbounded_channel();
        let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        // 2. 用 tx 构造 producer（anchor/deadline 复制自独立 Arc）
        let producer = BatchProducer {
            tx,
            trigger_tx,
            anchor: anchor.clone(),
            deadline: deadline.clone(),
        };
        // 3. 用 producer 构造 session
        let session = Arc::new(Session::new(key, model, agent_id, coordinator, producer, notify.clone()));
        // 4. 用 rx 和 session 构造 consumer（anchor/deadline/notify 均与 producer 共享同一 Arc）
        let consumer = BatchConsumer {
            rx,
            trigger_rx,
            delay: DelayQueue::new(),
            session: Arc::downgrade(&session),
            notify,
            anchor,
            deadline,
        };
        // 5. consumer 去 spawn（内联 spawn_trigger）
        tokio::spawn(consumer.run());
        session
    }

    /// 只保留仍在绑定集合中的会话（绑定信息变化后清理无绑定会话）
    pub fn retain(&self, keys: &HashSet<SessionKey>) {
        self.sessions.retain(|k, _| keys.contains(k));
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;

    fn key(agent: &str, role: &str) -> SessionKey {
        SessionKey { agent_name: agent.into(), role_name: role.into(), mode: Mode::Role }
    }

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
    /// 与 get_or_create 内联构造同构：2 mpsc → producer → session → consumer
    fn test_pair() -> (BatchProducer, BatchConsumer, Arc<Session>) {
        let key = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let notify = Arc::new(Notify::new());
        let (tx, rx) = mpsc::unbounded_channel();
        let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        let producer = BatchProducer {
            tx,
            trigger_tx,
            anchor: Arc::new(Instant::now()),
            deadline: Arc::new(AtomicU64::new(0)),
        };
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into()), Weak::new(), producer.clone(), notify.clone()));
        let consumer = BatchConsumer {
            rx,
            trigger_rx,
            delay: DelayQueue::new(),
            session: Arc::downgrade(&session),
            notify,
            anchor: producer.anchor.clone(),
            deadline: producer.deadline.clone(),
        };
        (producer, consumer, session)
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
        consumer.try_flush(false).await;
        assert!(consumer.rx.try_recv().is_err(), "已 drain");
        // 未超 deadline：不 drain
        let (p2, mut c2, _) = test_pair();
        p2.tx.send(ev("u1", "x")).unwrap();
        p2.set_deadline(Instant::now() + Duration::from_secs(10));
        c2.try_flush(false).await;
        assert!(c2.rx.try_recv().is_ok(), "未超时不应 drain");
    }

    #[tokio::test]
    async fn spawn_trigger_exits_on_notify() {
        let (producer, consumer, session) = test_pair();
        // 与 get_or_create 的 spawn 同构：tokio::spawn(consumer.run())
        tokio::spawn(consumer.run());
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

    #[tokio::test]
    async fn get_or_create_dedupes() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k = key("a1", "r1");
        let (s1, created1) = mgr.get_or_create(&k, Some(model.clone()), Arc::new("a1".into()), Weak::new());
        assert!(created1, "首次创建");
        let (s2, created2) = mgr.get_or_create(&k, Some(model.clone()), Arc::new("a1".into()), Weak::new());
        assert!(!created2, "同 key 复用");
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同 mode 是不同会话
        let k_event = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let (_s3, created3) = mgr.get_or_create(&k_event, Some(model), Arc::new("a1".into()), Weak::new());
        assert!(created3, "事件模式是独立会话");
    }

    #[tokio::test]
    async fn retain_prunes_unbound() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k1 = key("a1", "r1");
        let k2 = key("a2", "r2");
        mgr.get_or_create(&k1, Some(model.clone()), Arc::new("a1".into()), Weak::new());
        mgr.get_or_create(&k2, Some(model), Arc::new("a2".into()), Weak::new());
        let mut keep = HashSet::new();
        keep.insert(k1.clone());
        mgr.retain(&keep);
        assert!(mgr.get(&k1).is_some(), "仍在绑定集合的会话保留");
        assert!(mgr.get(&k2).is_none(), "无绑定会话销毁");
    }

    #[tokio::test]
    async fn get_or_create_with_none_model() {
        let mgr = SessionManager::new();
        let key = SessionKey { agent_name: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let (s, created) = mgr.get_or_create(&key, None, Arc::new("a".into()), Weak::new());
        assert!(created);
        assert!(s.model.load().is_none());
    }

    #[tokio::test]
    async fn session_copies_role_name_and_mode_from_key() {
        let key = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let model = Some(ProviderModel { provider: "p".into(), model: "m".into() });
        let agent_id = Arc::new("uuid".to_string());
        let notify = Arc::new(Notify::new());
        // 与 get_or_create 内联构造同构：2 mpsc → producer（测试丢弃接收端）
        let (tx, _rx) = mpsc::unbounded_channel();
        let (trigger_tx, _trigger_rx) = mpsc::unbounded_channel();
        let producer = BatchProducer {
            tx,
            trigger_tx,
            anchor: Arc::new(Instant::now()),
            deadline: Arc::new(AtomicU64::new(0)),
        };
        let session = Session::new(&key, model, agent_id, Weak::new(), producer, notify);
        assert_eq!(session.role_name.as_str(), "r1");
        assert_eq!(*session.mode, Mode::Event("e1".into()));
    }
}
