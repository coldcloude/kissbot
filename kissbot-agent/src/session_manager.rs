use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use chrono::Local;
use dashmap::DashMap;
use futures_util::StreamExt;
use kissbot_api::channel::IncomingMessageEvent;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Notify};
use tokio_util::time::DelayQueue;
use tracing::warn;

use crate::config_manager::ProviderModel;
use crate::coordinator::{AgentCoordinator, extract_text};
use crate::types::{Error, Message, Mode, Result, SessionKey};

/// 会话上下文：内存消息 + 本地缓存 + 历史归档一体管理（持久化由 SessionContext 自身负责，coordinator 不感知）
///
/// ========== 会话上下文本地缓存 ==========
/// 缓存文件：<data_dir>/context/<session_key编码>.jsonl，每行一条 Message（JSON）
/// 首行 System（如有当前系统消息），其后为消息行
/// 存储时不截断（tokio::fs 追加）；读取时全量回读（LineReader 正序）
///
/// ========== 历史上下文归档 ==========
/// 归档与清空缓存永远配对（archive_and_clear_cache）：把当前内存（含当前系统消息，System 首行）
/// 写成一个历史文件并加上时间戳文件名（无包装格式，不复制缓存文件），然后清空缓存文件，本轮只写不读
/// 历史与当前缓存格式完全一致（都是 <session_key编码>.jsonl 每行一条 Message），
/// 因此内存、缓存、历史三者统一在本结构体实现（会话上下文唯一入口）。
pub struct SessionContext {
    messages: Vec<Message>,
    /// 当前系统消息（模型上下文首条；含从缓存恢复的，直到下次发送前对比替换）
    system_message: Option<String>,
    /// 待定系统消息（set 只写这里，多次 set 只保留最近一次；下次发送前对比应用）
    pending_system: Option<String>,
    /// 数据目录：缓存/历史路径用时按 data_dir + key 编码构造（不冗余存路径）
    data_dir: PathBuf,
    /// session_key 文件名编码（缓存文件与历史文件的 key 编码段）
    key_enc: String,
}

impl SessionContext {
    pub fn new(data_dir: &str, key: &SessionKey) -> Self {
        // session_key → 文件名：{agent_name}-{role_name}（角色模式）或 {agent_name}-{role_name}-{event}（事件模式）
        let key_enc = match &key.mode {
            Mode::Role => format!("{}-{}", key.agent_name, key.role_name),
            Mode::Event(e) => format!("{}-{}-{}", key.agent_name, key.role_name, e),
        };
        Self {
            messages: Vec::new(),
            system_message: None,
            pending_system: None,
            data_dir: PathBuf::from(data_dir),
            key_enc,
        }
    }

    /// 当前缓存文件路径（<data_dir>/context/<session_key编码>.jsonl）
    fn cache_path(&self) -> PathBuf {
        self.data_dir.join("context").join(format!("{}.jsonl", self.key_enc))
    }

    /// 历史归档目录（<data_dir>/context-history）
    fn history_dir(&self) -> PathBuf {
        self.data_dir.join("context-history")
    }

    /// 设置系统消息（会话创建/重置时，配置或 ego 生成后调用）：只保存待定，
    /// 当前系统消息保留到下次发送前对比应用（多次 set 只保留最近一次）
    pub fn set_system_message(&mut self, content: String) {
        self.pending_system = Some(content);
    }

    /// 发送前应用待定系统消息（下次发送时调用）：无待定直接返回；
    /// 发送前应用待定系统消息（下次发送时调用）：无待定直接返回；
    /// 与当前一致 → 仅清空待定；不一致 → 归档并清空旧上下文（用原系统消息）→
    /// 替换为新系统消息 → 从内存写回缓存（System + 消息行）
    pub async fn apply_pending_system(&mut self) -> Result<()> {
        let Some(pending) = self.pending_system.take() else { return Ok(()); };
        if self.system_message.as_deref() == Some(pending.as_str()) {
            return Ok(());   // 一致：无需变更（set 消息已清空）
        }
        self.archive_and_clear_cache().await?;
        self.system_message = Some(pending);
        // 从内存写回缓存（只写消息行；System 首行由 append_cache 对新文件落，避免 System 重复）
        // 先 clone 消息再调用（&mut self 排他，不能同时借用 self.messages）
        let msgs = self.messages.clone();
        self.append_cache(&msgs).await
    }

    /// 追加消息（内存 + 缓存一体，每行一条 Message JSON；不截断）
    /// best-effort：缓存失败仅丢缓存不阻塞流程（内存已装入，调用方按 Result 决定）
    pub async fn append(&mut self, messages: &[Message]) -> Result<()> {
        self.messages.extend(messages.iter().cloned());
        self.append_cache(messages).await
    }

    /// 从缓存恢复上下文（event 模式）：正序全量回读（LineReader 按时间顺序）装入内存；
    /// 首行 System（如有）放当前系统消息；文件不存在清空
    pub async fn recover_from_cache(&mut self) -> Result<()> {
        let path = self.cache_path();
        if !path.exists() {
            self.messages = Vec::new();
            self.system_message = None;
            return Ok(());
        }
        let mut reader = kai_file::LineReader::new(&path, None, None).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        let mut msgs = Vec::new();
        while let Some(line) = reader.next_line().await
            .map_err(|e| Error::IoError(e.to_string()))?
        {
            let s = line.line.trim();
            if s.is_empty() { continue; }
            if let Ok(m) = serde_json::from_str::<Message>(s) {
                msgs.push(m);
            }
        }
        // 首行 System → 当前系统消息（缓存格式：System 行在最前，其后为消息行）
        if let Some(Message::System { content }) = msgs.first() {
            self.system_message = Some(content.as_str().to_string());
            msgs.remove(0);
        }
        // 崩溃一致性清理：恢复应从完整轮次开始。
        // 1) 若末尾是带 tool_calls 的 assistant（崩溃发生在追加 assistant(tool_calls) 后、
        //    工具响应写入前），丢弃这条悬挂的 assistant——否则恢复后 tool_calls 悬空无 Tool 响应。
        // 2) 丢弃开头的 Tool 消息（恢复起点之前的残留）。
        if let Some(Message::Assistant { tool_calls: Some(_), .. }) = msgs.last() {
            msgs.pop();
        }
        while matches!(msgs.first(), Some(Message::Tool { .. })) {
            msgs.remove(0);
        }
        self.messages = msgs;
        Ok(())
    }

    /// 归档当前上下文到历史并清空缓存文件（归档与清空永远配对；重置/重建/系统切换前的持久化清理）：
    /// 1) 把当前内存（含当前系统消息，System 首行）写成一个历史文件（<key编码>-<时间戳>.jsonl，无包装格式），
    ///    不复制缓存文件；内存为空则无可归档内容（仅有系统消息不归档，幂等）
    /// 2) 清空缓存文件（文件不存在幂等）
    ///
    /// &mut self 强制排他：调用方须持独占引用（经 session.context 互斥锁），避免并发归档/清空交错
    pub async fn archive_and_clear_cache(&mut self) -> Result<()> {
        // 1. 归档：当前内存（含当前系统消息）→ 历史
        if !self.messages.is_empty() {
            tokio::fs::create_dir_all(&self.history_dir()).await
                .map_err(|e| Error::IoError(e.to_string()))?;
            let ts = Local::now().format("%Y-%m-%d-%H%M%S").to_string();
            // 历史文件名 = <key编码>-<时间戳>.jsonl（key 编码即缓存文件名段）
            let dest = self.history_dir().join(format!("{}-{}.jsonl", self.key_enc, ts));
            let mut file = tokio::fs::OpenOptions::new()
                .create(true).write(true).truncate(true).open(&dest).await
                .map_err(|e| Error::IoError(e.to_string()))?;
            // 历史格式与缓存一致：System 首行（如有），其后为消息行
            let mut lines = Vec::with_capacity(self.messages.len() + 1);
            if let Some(system) = &self.system_message {
                lines.push(Message::System { content: Arc::new(system.clone()) });
            }
            lines.extend(self.messages.iter().cloned());
            self.write_lines(&mut file, &lines).await?;
        }
        // 2. 清空缓存文件（文件不存在幂等）
        match tokio::fs::remove_file(&self.cache_path()).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::IoError(e.to_string())),
        }
    }

    /// 重建上下文（压缩后）：清空内存（system 保留）→ 装入 messages → 从内存写回缓存
    /// （System + 消息行；不负责清理，调用方保证先 archive_and_clear_cache）
    pub async fn rebuild(&mut self, messages: Vec<Message>) -> Result<()> {
        self.messages.clear();
        // append 复用内存装入 + 缓存写回（不负责清理，调用方保证先 archive_and_clear_cache）
        self.append(&messages).await
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

    /// 检查是否超长（threshold 来自模型 effective 配置的 max_context_messages）
    pub fn is_overflow(&self, max: usize) -> bool {
        self.messages.len() >= max
    }

    // ========== 私有：缓存文件读写 ==========

    /// 缓存追加（每行一条 Message JSON；不截断）
    /// 新缓存文件（未落盘）先写 System 首行（如有当前系统消息），再追加消息行；
    /// 无消息不写（仅有系统消息不落缓存）；&mut self 排他：写入须独占，避免并发交叉写坏行
    async fn append_cache(&mut self, messages: &[Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let path = self.cache_path();
        let is_new = !path.exists();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| Error::IoError(e.to_string()))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true).append(true).open(&path).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        if is_new {
            // 缓存格式：System 行在最前；新文件创建时落当前系统消息
            if let Some(system) = &self.system_message {
                let line = serde_json::to_string(&Message::System { content: Arc::new(system.clone()) })?;
                file.write_all(line.as_bytes()).await
                    .map_err(|e| Error::IoError(e.to_string()))?;
                file.write_all(b"\n").await
                    .map_err(|e| Error::IoError(e.to_string()))?;
            }
        }
        self.write_lines(&mut file, messages).await
    }

    /// 逐行写 Message JSON（每行一条，\n 结尾；缓存/历史共用；&mut self 排他，调用方须持独占引用）
    async fn write_lines(&mut self, file: &mut tokio::fs::File, messages: &[Message]) -> Result<()> {
        for m in messages {
            let line = serde_json::to_string(m)?;
            file.write_all(line.as_bytes()).await
                .map_err(|e| Error::IoError(e.to_string()))?;
            file.write_all(b"\n").await
                .map_err(|e| Error::IoError(e.to_string()))?;
        }
        Ok(())
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
    /// 会话销毁通知（Drop 时 notify_one → trigger 任务退出；与 consumer.notify 同一 Arc）
    pub notify: Arc<Notify>,
}

impl Session {
    /// 合批 flush 入口：trigger 任务经 consumer.session 弱引用升级后调用（原 coordinator.flush_batch 职责迁至会话侧）
    /// 模型检查 → coordinator 单例 → out_channel 解析 → agentic loop
    pub(crate) async fn accept_batch(self: &Arc<Self>, content: String) {
        // 无可用模型：静默忽略（与 run_agentic_loop 入口一致）
        if self.model.load().is_none() {
            return;
        }
        let coordinator = AgentCoordinator::instance();
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
    /// 数据目录（会话上下文缓存/历史归档路径基准）
    data_dir: String,
}

impl SessionManager {
    pub fn new(data_dir: &str) -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
            data_dir: data_dir.to_string(),
        })
    }

    /// 按 key 取会话
    pub fn get(&self, key: &SessionKey) -> Option<Arc<Session>> {
        self.sessions.get(key).map(|e| e.value().clone())
    }

    /// 定位会话，不存在则创建（model 为初始模型，None = 无模型；agent_id 为会话状态保存的解析结果）；
    /// 返回 (会话, 是否新建)
    /// 创建时依赖序组装（内联 new_producer/BatchConsumer::new）：notify → 2 mpsc → producer → session → consumer → spawn
    /// （channel 均从 session.batch_producer 取 clone；任务持 consumer，consumer 持 session 弱引用与 notify，
    ///  anchor/deadline/notify 均为独立 Arc——producer 与 consumer 共享同一份）
    /// 双重锁定：先 get 快速路径（命中直接返回），未命中再走 entry API 原子创建（并发下仅一个创建成功）
    pub fn get_or_create(
        &self,
        key: &SessionKey,
        model: Option<ProviderModel>,
        agent_id: Arc<String>,
    ) -> (Arc<Session>, bool) {
        if let Some(s) = self.get(key) {
            return (s, false);
        }
        match self.sessions.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(e) => (e.get().clone(), false),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                // 创建部分抽出（create_session）：依赖序组装 + spawn 触发任务
                let session = Self::create_session(key, model, agent_id, &self.data_dir);
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
        data_dir: &str,
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
        // 3. 用 producer 构造 session（字面量，无 new 函数；Session 全字段在同文件内可见）
        let session = Arc::new(Session {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new(data_dir, key)),
            batch_producer: producer,
            model: ArcSwap::from_pointee(model),
            agent_id,
            notify: notify.clone(),
        });
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
        let dir = tempfile::tempdir().unwrap();
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
        // 3. 用 producer 构造 session（字面量，与 create_session 同构）
        let session = Arc::new(Session {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new(dir.path().to_str().unwrap(), &key)),
            batch_producer: producer.clone(),
            model: ArcSwap::from_pointee(None),
            agent_id: Arc::new("aid".into()),
            notify: notify.clone(),
        });
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

    /// 测试用 SessionManager（data_dir 指向临时目录；会话持久化路径仅构造不落盘）
    fn mgr() -> Arc<SessionManager> {
        let dir = tempfile::tempdir().unwrap();
        SessionManager::new(dir.path().to_str().unwrap())
    }

    #[tokio::test]
    async fn get_or_create_dedupes() {
        let mgr = mgr();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k = key("a1", "r1");
        let (s1, created1) = mgr.get_or_create(&k, Some(model.clone()), Arc::new("a1".into()));
        assert!(created1, "首次创建");
        let (s2, created2) = mgr.get_or_create(&k, Some(model.clone()), Arc::new("a1".into()));
        assert!(!created2, "同 key 复用");
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同 mode 是不同会话
        let k_event = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let (_s3, created3) = mgr.get_or_create(&k_event, Some(model), Arc::new("a1".into()));
        assert!(created3, "事件模式是独立会话");
    }

    #[tokio::test]
    async fn retain_prunes_unbound() {
        let mgr = mgr();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k1 = key("a1", "r1");
        let k2 = key("a2", "r2");
        mgr.get_or_create(&k1, Some(model.clone()), Arc::new("a1".into()));
        mgr.get_or_create(&k2, Some(model), Arc::new("a2".into()));
        let mut keep = HashSet::new();
        keep.insert(k1.clone());
        mgr.retain(&keep);
        assert!(mgr.get(&k1).is_some(), "仍在绑定集合的会话保留");
        assert!(mgr.get(&k2).is_none(), "无绑定会话销毁");
    }

    #[tokio::test]
    async fn get_or_create_with_none_model() {
        let mgr = mgr();
        let key = SessionKey { agent_name: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let (s, created) = mgr.get_or_create(&key, None, Arc::new("a".into()));
        assert!(created);
        assert!(s.model.load().is_none());
    }

    #[tokio::test]
    async fn session_copies_role_name_and_mode_from_key() {
        let dir = tempfile::tempdir().unwrap();
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
        // 3. 用 producer 构造 session（字面量，与 create_session 同构）
        let session = Session {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new(dir.path().to_str().unwrap(), &key)),
            batch_producer: producer,
            model: ArcSwap::from_pointee(model),
            agent_id,
            notify,
        };
        assert_eq!(session.role_name.as_str(), "r1");
        assert_eq!(*session.mode, Mode::Event("e1".into()));
    }

    // ===== 会话上下文（内存 + 缓存 + 历史一体）测试 =====

    fn cache_key() -> SessionKey {
        SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) }
    }

    fn role_key() -> SessionKey {
        SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role }
    }

    fn sample_msgs() -> Vec<Message> {
        vec![
            Message::User { content: Arc::new("你好".into()) },
            Message::Assistant { content: Arc::new("在的".into()), reasoning_content: Some(Arc::new("思考".into())), tool_calls: None },
        ]
    }

    #[tokio::test]
    async fn append_persists_and_recover_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let k = cache_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        assert!(ctx.build().is_empty(), "初始为空");
        assert!(!dir.path().join("context").exists(), "未写入前不落盘");
        ctx.append(&sample_msgs()).await.unwrap();
        // 同 key 新上下文从缓存恢复（内存为空）：append 已同时写内存 + 缓存
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        let back = recovered.build();
        assert_eq!(back.len(), 2);
        assert!(matches!(&back[0], Message::User { content } if content.as_str() == "你好"));
        assert!(matches!(&back[1], Message::Assistant { reasoning_content: Some(r), .. } if r.as_str() == "思考"), "reasoning_content 应保留");
    }

    #[tokio::test]
    async fn append_twice_accumulates_and_rebuild_clears() {
        let dir = tempfile::tempdir().unwrap();
        let k = cache_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        ctx.append(&sample_msgs()).await.unwrap();
        ctx.append(&[Message::User { content: Arc::new("再问".into()) }]).await.unwrap();
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        assert_eq!(recovered.build().len(), 3, "追加不截断");
        // 重建（role 构建路径：archive_and_clear_cache → rebuild，替代 reset 语义）：清空内存，缓存不残留
        ctx.archive_and_clear_cache().await.unwrap();
        ctx.rebuild(vec![]).await.unwrap();
        assert!(ctx.build().is_empty(), "重建后内存为空");
        let mut after = SessionContext::new(dir.path().to_str().unwrap(), &k);
        after.recover_from_cache().await.unwrap();
        assert!(after.build().is_empty(), "重建后缓存为空");
        // 无内容时重复重建幂等
        ctx.archive_and_clear_cache().await.unwrap();
        ctx.rebuild(vec![]).await.unwrap();
    }

    #[tokio::test]
    async fn recover_sanitizes_dangling_tool_turn() {
        let dir = tempfile::tempdir().unwrap();
        // 用例 A：完整轮次 user → assistant(tool_calls) → tool，末尾再追一条悬挂 assistant(tool_calls)
        // （崩溃发生在追加 assistant(tool_calls) 后、Tool 响应写入前）→ 回读丢弃悬挂尾巴，保留完整轮次
        let k_a = cache_key();
        let mut ctx_a = SessionContext::new(dir.path().to_str().unwrap(), &k_a);
        ctx_a.append(&[
            Message::User { content: Arc::new("查一下".into()) },
            Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: Some(vec![]) },
            Message::Tool { tool_call_id: Arc::new("c1".into()), name: Arc::new("read".into()), content: Arc::new("内容".into()) },
        ]).await.unwrap();
        ctx_a.append(&[Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: Some(vec![]) }]).await.unwrap();
        let mut rec_a = SessionContext::new(dir.path().to_str().unwrap(), &k_a);
        rec_a.recover_from_cache().await.unwrap();
        let back = rec_a.build();
        assert_eq!(back.len(), 3, "悬挂的 assistant(tool_calls) 应被丢弃，保留完整轮次");
        assert!(matches!(&back[1], Message::Assistant { tool_calls: Some(_), .. }), "完整轮次的 assistant(tool_calls) 保留");
        assert!(matches!(&back[2], Message::Tool { .. }), "tool 响应保留");

        // 用例 B：仅一条悬挂 assistant(tool_calls) → 恢复为空
        let k_b = SessionKey { agent_name: "a1".into(), role_name: "r2".into(), mode: Mode::Role };
        let mut ctx_b = SessionContext::new(dir.path().to_str().unwrap(), &k_b);
        ctx_b.append(&[Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: Some(vec![]) }]).await.unwrap();
        let mut rec_b = SessionContext::new(dir.path().to_str().unwrap(), &k_b);
        rec_b.recover_from_cache().await.unwrap();
        assert!(rec_b.build().is_empty(), "仅悬挂 assistant 时恢复为空");

        // 用例 C：开头的 Tool 残留（恢复起点之前的半条轮次）被丢弃
        let k_c = SessionKey { agent_name: "a1".into(), role_name: "r3".into(), mode: Mode::Role };
        let mut ctx_c = SessionContext::new(dir.path().to_str().unwrap(), &k_c);
        ctx_c.append(&[
            Message::Tool { tool_call_id: Arc::new("c9".into()), name: Arc::new("read".into()), content: Arc::new("残留".into()) },
            Message::User { content: Arc::new("继续".into()) },
        ]).await.unwrap();
        let mut rec_c = SessionContext::new(dir.path().to_str().unwrap(), &k_c);
        rec_c.recover_from_cache().await.unwrap();
        let back_c = rec_c.build();
        assert_eq!(back_c.len(), 1, "开头 Tool 残留被丢弃，保留后续完整消息");
        assert!(matches!(&back_c[0], Message::User { content } if content.as_str() == "继续"));
    }

    #[tokio::test]
    async fn archive_and_clear_writes_history_and_removes_cache() {
        let dir = tempfile::tempdir().unwrap();
        let k = role_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        ctx.append(&sample_msgs()).await.unwrap();
        let cache_path = dir.path().join("context").join("a1-r1.jsonl");
        assert!(cache_path.exists(), "append 已建缓存");
        // 归档 + 清空一体：历史生成，缓存移除
        ctx.archive_and_clear_cache().await.unwrap();
        // 目标文件名 = <key编码>-<时间戳>.jsonl（历史目录内恰有一个文件；role_key = a1-r1）
        let history_dir = dir.path().join("context-history");
        let mut files = Vec::new();
        let mut rd = tokio::fs::read_dir(&history_dir).await.unwrap();
        while let Some(entry) = rd.next_entry().await.unwrap() {
            files.push(entry);
        }
        assert_eq!(files.len(), 1, "归档生成一个历史文件");
        let dest = files.remove(0).path();
        let fname = dest.file_name().unwrap().to_str().unwrap().to_string();
        assert!(fname.starts_with("a1-r1"), "文件名以 key 编码开头: {}", fname);
        assert!(fname.ends_with(".jsonl"));
        // 内容与内存一致（逐行 Message JSON；不复制缓存）
        let expect = sample_msgs().iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>().join("\n") + "\n";
        assert_eq!(tokio::fs::read_to_string(&dest).await.unwrap(), expect);
        // 归档同时清空缓存（重写/重建前旧缓存不残留）
        assert!(!cache_path.exists(), "归档并清空后缓存不存在");
    }

    #[tokio::test]
    async fn archive_and_clear_with_empty_memory_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let k = role_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        // 内存为空：无归档、无报错、不创建历史目录
        ctx.archive_and_clear_cache().await.unwrap();
        assert!(!dir.path().join("context-history").exists(), "不创建历史目录");
        // 仅有系统消息同样不归档（历史只记录有消息的上下文）
        ctx.set_system_message("仅系统".into());
        ctx.apply_pending_system().await.unwrap();
        ctx.archive_and_clear_cache().await.unwrap();
        assert!(!dir.path().join("context-history").exists(), "系统消息不产生历史");
    }

    // ===== 系统消息：缓存/历史持久化 + 待定懒切换 =====

    #[tokio::test]
    async fn set_system_message_is_lazy_and_applies_on_send() {
        let dir = tempfile::tempdir().unwrap();
        let k = role_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        // set 只保存待定（多次 set 只保留最近一次），build 不立即带 system
        ctx.set_system_message("旧系统".into());
        ctx.set_system_message("新系统".into());
        assert!(ctx.build().iter().all(|m| !matches!(m, Message::System { .. })), "set 不立即生效");
        // 发送前应用：当前系统为空 → 不一致 → 替换（仅有系统消息不落缓存）
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"), "取最近一次 set");
        assert!(!dir.path().join("context").exists(), "仅有系统消息不写缓存");
        // 追加消息后缓存含 System 首行：同 key 新上下文恢复得到系统消息
        ctx.append(&[Message::User { content: Arc::new("你好".into()) }]).await.unwrap();
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        assert!(matches!(&recovered.build()[0], Message::System { content } if content.as_str() == "新系统"), "缓存读出的系统消息放当前");
        let back = recovered.build();
        assert_eq!(back.len(), 2, "系统 + 消息");
        assert!(matches!(&back[1], Message::User { content } if content.as_str() == "你好"));
        // 再次 set 同值：一致 → 无变更（set 消息清空）
        ctx.set_system_message("新系统".into());
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"));
        assert!(!dir.path().join("context-history").exists(), "一致时不产生归档");
    }

    #[tokio::test]
    async fn system_change_archives_old_context_with_old_system() {
        let dir = tempfile::tempdir().unwrap();
        let k = role_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        ctx.set_system_message("旧系统".into());
        ctx.apply_pending_system().await.unwrap();
        ctx.append(&[Message::User { content: Arc::new("你好".into()) }]).await.unwrap();
        // 变更系统：不一致 → 旧上下文（含原系统消息）写成历史 → 替换 → 重建缓存
        ctx.set_system_message("新系统".into());
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"));
        // 历史：System 旧 + User 你好
        let history_dir = dir.path().join("context-history");
        let mut files = Vec::new();
        let mut rd = tokio::fs::read_dir(&history_dir).await.unwrap();
        while let Some(entry) = rd.next_entry().await.unwrap() {
            files.push(entry);
        }
        assert_eq!(files.len(), 1, "系统变更时旧上下文归档一次");
        let dest_text = tokio::fs::read_to_string(files.remove(0).path()).await.unwrap();
        assert!(dest_text.contains("旧系统"), "历史含原系统消息");
        assert!(dest_text.contains("你好"), "历史含原内存消息");
        // 缓存：System 新 + User 你好（消息保留）
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        assert!(matches!(&recovered.build()[0], Message::System { content } if content.as_str() == "新系统"));
        let back = recovered.build();
        assert_eq!(back.len(), 2);
        assert!(matches!(&back[1], Message::User { content } if content.as_str() == "你好"), "消息保留");
    }
}
