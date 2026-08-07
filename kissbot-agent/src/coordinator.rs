use std::collections::HashSet;
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use arc_swap::{ArcSwap, ArcSwapOption};
use bytes::Bytes;
use chrono::Local;
use dashmap::DashMap;
use tracing::{info, warn};
use tokio::sync::mpsc;

use crate::types::{
    Mode, Message, Result, Error, SessionKey, memory_role,
};
use crate::context_cache::ContextCache;
use crate::context_config::EffectiveContextConfig;
use crate::history::HistoryArchive;
use crate::config_manager::{ConfigManager, ProviderModel, OutChannel, ToolConfig};
use crate::command_router::CommandRouter;
use crate::model_client::ModelClient;
use crate::session_manager::{Session, SessionManager};
use crate::memory_reader::{MemoryReader, pack_memory_messages};
use crate::memory_store_client::MemoryStoreClient;
use crate::station::{self, StationRuntime};

use kissbot_api::channel::{IncomingMessageEvent, OutgoingMessage, BindRequest, ChannelUser};
use kissbot_api::memory::{ChannelRequest, ThinkRequest, ToolCallRequest, ToolResultRequest};
use kissbot_api::message::{Content, AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};
/// 保留 agent/role：agent_name 为空 = 保留 agent（建会话但初始上下文用默认系统提示词，见 build_initial_context）；
/// 保留 agent 的 memory-store/ego agent_id 为 RESERVED_AGENT_ID（"0"）。
pub const RESERVED_AGENT_NAME: &str = "";
pub const RESERVED_AGENT_ID: &str = "0";
pub const RESERVED_ROLE_NAME: &str = "";

/// 每 channel 运行时：已发未回显的 outgoing msg_id 集合的 TTL（秒）
const CHANNEL_CONTEXT_TTL_SECS: u64 = 60;
/// 上述 TTL 的 Duration 形式（evict 入参）
const CHANNEL_CONTEXT_TTL: Duration = Duration::from_secs(CHANNEL_CONTEXT_TTL_SECS);

/// Agentic Loop 工具调用轮次上限（防死循环）
const MAX_TOOL_ROUNDS: usize = 10;

// 上下文消息数量上限（溢出触发重置/压缩）已废弃硬编码常量 MAX_CONTEXT_MESSAGES——
// 阈值统一由会话模型 effective.max_context_messages（provider/model 配置合成）决定，见 run_agentic_loop 溢出检查。

/// 每 channel 运行时上下文：维护「已发出但尚未收到回显」的 msg_id 集合 + 运行态 agent_id
/// 运行态 agent_id（UUID）在启动绑定/切换 agent 时确定并自主保存；
/// 解析失败回退保留 agent_id（"0"，等同 agent_name="" 的保留语义）
struct ChannelContext {
    /// 已发出未回显的 msg_id -> 记录时间（DashMap 无锁并发访问）
    pending_outgoing: DashMap<String, Instant>,
    /// 运行态 agent_id（ArcSwapOption 无锁读写；未绑定为 None，channel_agent 懒绑定）
    agent_id: ArcSwapOption<String>,
    /// 运行态模式（ArcSwap 无锁读写；/mode 切换不回写，重启回 Role）
    mode: ArcSwap<Mode>,
    /// 合批数据发送端（绑定会话时从 session.batch 取得；会话重定位后刷新，None 时懒绑定）
    batch_tx: tokio::sync::Mutex<Option<mpsc::UnboundedSender<crate::batching::BatchItem>>>,
    /// 合批触发器发送端（同上）
    trigger_tx: tokio::sync::Mutex<Option<mpsc::UnboundedSender<crate::batching::Trigger>>>,
}

impl ChannelContext {
    fn new() -> Self {
        Self {
            pending_outgoing: DashMap::new(),
            agent_id: ArcSwapOption::new(None),
            mode: ArcSwap::from_pointee(Mode::Role),
            batch_tx: tokio::sync::Mutex::new(None),
            trigger_tx: tokio::sync::Mutex::new(None),
        }
    }

    fn add_pending(&self, msg_id: String) {
        self.evict(CHANNEL_CONTEXT_TTL);
        self.pending_outgoing.insert(msg_id, Instant::now());
    }

    /// 命中则移除并返回 true（回显消费）；未命中再清理过期条目（懒清理）
    fn consume_pending(&self, msg_id: &str) -> bool {
        // 先尝试匹配消费（命中直接返回，避免每次消费都遍历清理）
        if self.pending_outgoing.remove(msg_id).is_some() {
            return true;
        }
        self.evict(CHANNEL_CONTEXT_TTL);
        false
    }

    /// TTL 懒清理：先遍历收集过期条目，再逐个删除（DashMap 迭代期间不可修改，两步防死锁）
    fn evict(&self, ttl: Duration) {
        let expired: Vec<String> = self.pending_outgoing.iter()
            .filter(|e| e.value().elapsed() >= ttl)
            .map(|e| e.key().clone())
            .collect();
        for key in expired {
            self.pending_outgoing.remove(&key);
        }
    }
}

/// agent/role/event 变更任务（mpsc 队列串行处理，避免写-写竞态；读无需外部加锁）
/// 统一为「应用新的会话三元组」：写 config + 运行态 mode + （可选）agent_id + 会话重定位
enum ConfigChange {
    /// 应用新会话三元组（agent/role/mode 任一变化）；agent_id 仅 /agent 切换时 Some
    ApplyKey { channel_id: String, new_key: SessionKey, agent_id: Option<Arc<String>>, done: tokio::sync::oneshot::Sender<Result<()>> },
}

pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    /// 上下文本地缓存（agent-data/context）
    cache: Arc<ContextCache>,
    /// 历史上下文归档（agent-data/context-history）
    history: Arc<HistoryArchive>,
    memory_reader: Arc<MemoryReader>,
    memory_store_client: Arc<MemoryStoreClient>,
    session_manager: Arc<SessionManager>,
    model_client: Arc<tokio::sync::Mutex<ModelClient>>,
    /// 启动校验后的 default_model（从 API 模型列表校验）；None = 无模型（普通消息静默忽略）
    valid_default: ArcSwap<Option<ProviderModel>>,
    /// 按 agent 内部 channel_id 索引的 ChannelClient
    channel_clients: Arc<DashMap<String, Arc<ChannelClient>>>,
    /// 每 channel 运行时上下文（无锁：DashMap pending + ArcSwapOption agent_id）
    channel_contexts: Arc<DashMap<String, Arc<ChannelContext>>>,
    /// 断线通知：channel_id → Notify，closed() 通知重连循环
    disconnect_notify: Arc<DashMap<String, Arc<tokio::sync::Notify>>>,
    /// agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
    command_tx: tokio::sync::mpsc::UnboundedSender<ConfigChange>,
    /// station_id → StationRuntime（启动时按配置构建；base_url 为空的本地 station 注册内置 Read 工具）
    station_runtimes: Arc<DashMap<String, Arc<StationRuntime>>>,
    /// 自引用弱引用（new() 中设置；channel 合批延时任务升级为 Arc<Self> 回调用）
    weak_self: OnceLock<Weak<Self>>,
}

impl AgentCoordinator {
    pub async fn new(
        config: Arc<ConfigManager>,
    ) -> Result<Arc<Self>> {
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let session_manager = SessionManager::new();
        let model_client = ModelClient::new(config.clone());
        let data_dir = config.data_dir().to_string();
        // agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<ConfigChange>();

        let coordinator = Arc::new(Self {
            config: config.clone(),
            cache: Arc::new(ContextCache::new(&data_dir)),
            history: Arc::new(HistoryArchive::new(&data_dir)),
            memory_reader,
            memory_store_client,
            session_manager,
            model_client: Arc::new(tokio::sync::Mutex::new(model_client)),
            channel_clients: Arc::new(DashMap::new()),
            channel_contexts: Arc::new(DashMap::new()),
            disconnect_notify: Arc::new(DashMap::new()),
            valid_default: ArcSwap::from_pointee(None),
            command_tx,
            station_runtimes: Arc::new(DashMap::new()),
            weak_self: OnceLock::new(),
        });

        // 设置自引用弱引用（合批延时任务升级用；弱引用避免引用环）
        let _ = coordinator.weak_self.set(Arc::downgrade(&coordinator));

        // 启动变更消费者：agent/role/event 变更串行处理（避免写-写竞态；读不受影响）
        {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                while let Some(change) = command_rx.recv().await {
                    match change {
                        ConfigChange::ApplyKey { channel_id, new_key, agent_id, done } => {
                            let rst = coordinator.apply_channel_key(&channel_id, &new_key, agent_id).await;
                            let _ = done.send(rst);
                        }
                    }
                }
            });
        }

        // 启动校验 default_model：从 API 拉模型列表，不在列表则无模型（告警）
        let default_model = config.default_model().await;
        let valid_default = match coordinator.model_client.lock().await.list_models(&default_model).await {
            Ok(list) if list.iter().any(|m| m == &default_model.model) => Some(default_model.clone()),
            Ok(_) => { tracing::warn!("default_model {}/{} 不在 API 模型列表", default_model.provider, default_model.model); None }
            Err(e) => { tracing::warn!("校验 default_model 失败（API 不可用?）: {:?}", e); None }
        };
        coordinator.valid_default.store(Arc::new(valid_default));

        // 构建 Station 运行态：base_url 为空的本地 station 注册内置 Read 工具；
        // 远程 station 的 runtime 同样构建（call_tool 走 REST 骨架，本轮不实现）
        {
            let runtimes = coordinator.station_runtimes.clone();
            for (_, sc) in config.stations().await {
                let runtime = Arc::new(StationRuntime::new(sc));
                if runtime.config().base_url.is_empty() {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    runtime.register_local("read", Arc::new(station::ReadTool::new(cwd)));
                }
                runtimes.insert(runtime.station_id().to_string(), runtime);
            }
        }

        // 启动：为全部 channel 绑定运行态 agent（解析失败回退保留 agent），
        // 再按 channel 绑定三元组初始化会话集合（agent_name 为空 = 保留 agent，同样建会话）
        for (_, ch) in config.channels().await {
            coordinator.bind_channel_runtime(&ch.channel_id).await;
            let key = coordinator.session_key_for(&ch);
            coordinator.ensure_session(&key, &ch.channel_id).await;
        }

        // 连接所有 enabled 的 channel
        coordinator.connect_channels().await;

        info!("AgentCoordinator 初始化完成");
        Ok(coordinator)
    }

    // ==================== 会话定位与构建 ====================

    /// 按来源 channel 的绑定配置 + 运行态 mode 计算会话 key（agent/role 取绑定配置，纯函数逻辑见 session_key_of）
    fn session_key_for(&self, ch: &crate::config_manager::ChannelConfig) -> SessionKey {
        // 运行态 mode 参与会话定位（从 ChannelContext 读）；无脱离态，agent_name 为空 = 保留 agent
        let mode = self.channel_mode(&ch.channel_id);
        session_key_of(&ch.agent_name, &ch.role_name, mode)
    }

    /// 记录已发出的 outgoing msg_id 到该 channel 的 pending 集合（回显判定用）
    async fn record_outgoing_msg_id(&self, channel_id: &str, msg_id: &Arc<String>) {
        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        ctx.add_pending(msg_id.as_str().to_string());
    }

    /// 按 msg_id 判定是否为自身发出的回显；命中则消费（移除）并返回 true
    async fn is_self_echo_by_msg_id(&self, channel_id: &str, msg_id: &Arc<String>) -> bool {
        if let Some(ctx) = self.channel_contexts.get(channel_id) {
            ctx.consume_pending(msg_id.as_str())
        } else {
            false
        }
    }

    /// 读取 channel 运行态 agent_id；未绑定（异常路径）时懒绑定
    async fn channel_agent(&self, channel_id: &str) -> Arc<String> {
        let ctx = self.channel_contexts.get(channel_id).map(|c| c.clone());
        if let Some(ctx) = ctx {
            if let Some(agent_id) = ctx.agent_id.load_full() {
                return agent_id;
            }
        }
        // 未绑定/缺失：懒绑定（正常启动路径已在 new() 中绑定全部 channel）
        self.bind_channel_runtime(channel_id).await
    }

    /// 绑定（或重绑）channel 运行态 agent_id：从配置 agent_name 解析，写入 channel 运行状态；
    /// 空 agent_name 直接保留；解析失败回退保留 agent_id（"0"）并告警
    async fn bind_channel_runtime(&self, channel_id: &str) -> Arc<String> {
        let agent_name = self.config.channel(channel_id).await
            .map(|c| c.agent_name.to_string())
            .unwrap_or_default();
        let agent_id = if agent_name.is_empty() {
            Arc::new(RESERVED_AGENT_ID.to_string())
        } else {
            match resolve_agent_id_http(&agent_name, &kissbot_api::ApiConfig::get().memory_ego_url).await {
                Ok(agent_id) => agent_id,
                Err(e) => {
                    warn!("解析 agent {} 失败（{}），回退保留 agent_id（\"0\"）", agent_name, e);
                    Arc::new(RESERVED_AGENT_ID.to_string())
                }
            }
        };
        self.set_channel_runtime(channel_id, agent_id.clone()).await
    }

    /// 写入 channel 运行态 agent_id（切换成功时由命令入口调用）
    pub async fn set_channel_runtime(&self, channel_id: &str, agent_id: Arc<String>) -> Arc<String> {
        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        ctx.agent_id.store(Some(agent_id.clone()));
        agent_id
    }

    /// 绑定会话后刷新合批发送端（从 session.batch 取 clone；会话创建/重定位时调用，None 时 enqueue 懒绑定）
    async fn bind_batch_tx(&self, channel_id: &str, session: &Arc<Session>) {
        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        *ctx.batch_tx.lock().await = Some(session.batch.tx.clone());
        *ctx.trigger_tx.lock().await = Some(session.batch.trigger_tx.clone());
    }

    /// 设置 channel 运行态模式（写 ChannelContext.mode；/mode 切换，不回写，重启回 Role）
    pub fn set_channel_mode(&self, channel_id: &str, mode: Mode) {
        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        ctx.mode.store(Arc::new(mode));
    }

    /// 取 channel 运行态模式（未绑定/缺失回退角色模式）
    pub fn channel_mode(&self, channel_id: &str) -> Mode {
        match self.channel_contexts.get(channel_id) {
            Some(c) => (*c.mode.load_full()).clone(),
            None => Mode::Role,
        }
    }

    /// 解析 agent_name -> agent_id（不缓存）：空 agent_name 返回保留 id；
    /// 解析失败返回 Err（切换 agent 时由调用方决定保持原 agent 不变）
    pub async fn resolve_agent_id_for_bind(&self, agent_name: &str) -> Result<Arc<String>> {
        resolve_agent_id_http(agent_name, &kissbot_api::ApiConfig::get().memory_ego_url).await
    }

    /// 定位会话，新建时构建初始上下文；返回 (会话, 是否新建)
    /// channel_id 为触发会话创建/重置的来源 channel（新建会话的 agent_id 取自该 channel 运行态绑定）
    async fn ensure_session(&self, key: &SessionKey, channel_id: &str) -> (Arc<Session>, bool) {
        // valid_default.load_full() 返回 Arc<Option<ProviderModel>>，解引用克隆得 Option
        let model = (*self.valid_default.load_full()).clone();
        // 会话状态保存 agent_id：新建会话从来源 channel 运行态绑定取得（原子写入 get_or_create）
        let agent_id = self.channel_agent(channel_id).await;
        let (session, created) = self.session_manager.get_or_create(key, model, agent_id);
        if created {
            self.build_initial_context(&session).await;
            // 随会话创建：绑定合批发送端（channel 持 tx/trigger_tx）+ 设置 trigger 任务升级槽
            // （trigger 任务已在 Session::new 随会话 spawn；槽 OnceLock set 一次——coordinator 为进程级
            //  单例、session Weak 每 producer 一次；消息必须先过 ensure_session 才路由进队列，故 flush 时槽必然已设置）
            self.bind_batch_tx(channel_id, &session).await;
            let _ = session.batch.session.set(Arc::downgrade(&session));
            let _ = session.batch.coordinator.set(self.weak_self.get().cloned().unwrap_or_default());
        }
        (session, created)
    }

    /// 会话创建/重置时：加载 ego（保留 agent 用默认提示词）+ 历史记录 + 顶层记忆索引构建初始上下文
    /// 取记忆/ego 一律用会话状态保存的 agent_id（session_key 仅去重，不从 key 提取 agent_name）
    async fn build_initial_context(&self, session: &Arc<Session>) {
        // 保留 agent（agent_id="0"）不调 memory-ego，用 NexusRepo 默认系统提示词；其余走 load_ego_info
        if session.agent_id.as_str() == RESERVED_AGENT_ID {
            let prompt = self.config.default_system_prompt().await;
            session.context.lock().await.set_system_message(prompt);
        } else if let Ok(ego_info) = self.load_ego_info(session.agent_id.as_str(), &session.role_name).await {
            session.context.lock().await.set_system_message(ego_info);
        }
        // 按模式加载上下文：event 从缓存恢复；role 从记忆打包（两查询比较取并集，打包为一条 user 消息）
        let key = self.session_key_of_session(session);
        match &*session.mode {
            Mode::Event(_) => {
                if let Ok(history) = self.cache.read_all(&key).await {
                    session.context.lock().await.load_messages(history);
                }
            }
            Mode::Role => {
                // 重新进入既有 role 会话：旧上下文先归档为历史（重建后缓存将被清空重写）
                // reset_context 已先归档+清空，此处缓存不存在不会重复归档
                let path = self.cache.path_for(&key);
                if path.exists() {
                    let _ = self.history.archive(&key, &path).await;
                    let _ = self.cache.clear(&key).await;
                }
                // 记忆打包：组合查询 + 每组合全史查询 + 并集算法（最后 N 条 ∪ [M, T_N] 同时间组，窗口内早于 T_N 的记录不含），打包为一条 user 消息作为首条内容
                let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
                if let Ok(msgs) = self.memory_reader
                    .read_recent_for_context(session.agent_id.as_str(), session.role_name.as_str(), &cfg)
                    .await
                {
                    if let Some(packed) = pack_memory_messages(&msgs) {
                        session.context.lock().await.push(packed);
                    }
                }
            }
        }
        // 顶层记忆索引（memory-struct 未实现时静默跳过）——保持不变
        let _ = self.memory_reader
            .read_memory_struct_index(&self.config, session.agent_id.as_str(), &session.role_name, &session.mode)
            .await;
    }

    /// 从 Session 运行态构造 SessionKey（缓存/历史定位用）
    fn session_key_of_session(&self, session: &Arc<Session>) -> SessionKey {
        session_key_of(session.agent_name.as_str(), session.role_name.as_str(), (*session.mode).clone())
    }

    /// 来源 channel 绑定信息变化后重定位会话：清理无绑定会话 + 为新三元组创建会话
    async fn relocate_channel(&self, channel_id: &str) {
        // 1. 清理无任何 channel 绑定的会话
        self.prune_sessions().await;
        // 2. 新三元组对应会话不存在则创建并构建初始上下文（agent 标识取该 channel 运行态绑定）
        if let Some(ch) = self.config.channel(channel_id).await {
            let key = self.session_key_for(&ch);
            self.ensure_session(&key, channel_id).await;
        }
    }

    /// 按当前全部 channel 的绑定集合清理无绑定会话
    async fn prune_sessions(&self) {
        let channels = self.config.channels().await;
        let mut keys = HashSet::new();
        for (_, ch) in &channels {
            keys.insert(self.session_key_for(ch));
        }
        self.session_manager.retain(&keys);
    }

    /// 重置来源 channel 所属会话的上下文
    async fn reset_session_for(&self, channel_id: &str) {
        if let Some(ch) = self.config.channel(channel_id).await {
            let key = self.session_key_for(&ch);
            if let Some(session) = self.session_manager.get(&key) {
                self.reset_context(&session).await;
                return;
            }
        }
        warn!("reset: channel {} 无会话可重置", channel_id);
    }

    /// 上下文重置（新 session_key 或超长）：按模式归档当前缓存 → 清空 → 重建
    /// event：归档（超长时调用方先 compress，此处仅归档+清空+重建空白缓存）
    /// role：归档 + 记忆打包重建
    /// 合批：数据留队（新机制不清空），期间消息并入重置后统一打包；
    /// 重置末尾发送 Trigger::Forced 强制 flush（不检查 deadline，重置期间消息即刻并入新上下文）
    async fn reset_context(&self, session: &Arc<Session>) {
        let key = self.session_key_of_session(session);
        let path = self.cache.path_for(&key);
        if path.exists() {
            let _ = self.history.archive(&key, &path).await;
        }
        let _ = self.cache.clear(&key).await;
        session.context.lock().await.clear();
        self.build_initial_context(session).await;
        // 重置完成：强制 flush（不检查 deadline），重置期间到达的消息即刻并入新上下文
        // （reset 可能由 trigger 任务的 flush → run_agentic_loop 溢出路径调用，Forced 入队后由任务串行处理）
        session.batch.trigger_tx.send(crate::batching::Trigger::Forced).ok();
        info!("会话上下文已重置: role={} mode={:?}", session.role_name, session.mode);
    }

    /// event 模式超长压缩：归档当前缓存 → LLM 总结（compress_prompt + 当前上下文）→
    /// 重写缓存为 system + user(压缩指令) + assistant(总结)，等待后续 channel 消息
    async fn compress_context(&self, session: &Arc<Session>) {
        let key = self.session_key_of_session(session);
        // 0. 先校验模型可用（无模型早退，避免留下冗余归档副本）
        let model = session.model.load_full();
        let Some(pm) = model.as_ref() else { return; };
        let path = self.cache.path_for(&key);
        if path.exists() {
            let _ = self.history.archive(&key, &path).await;
        }
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        // 1. 取当前完整上下文（含 system），末尾追加压缩指令 user 消息
        let messages = {
            let ctx = session.context.lock().await;
            let mut msgs = ctx.build();
            msgs.push(Message::User { content: Arc::new(cfg.compress_prompt.clone()) });
            msgs
        };
        // 2. 调会话模型总结（压缩不携带工具定义）
        let summary = {
            let mc = self.model_client.lock().await;
            mc.call(pm, &messages, &[]).await.map(|r| r.content).unwrap_or_default()
        };
        if summary.is_empty() {
            warn!("上下文压缩总结为空，保留原上下文");
            return;
        }
        // 3. 重建：清空内存（system 保留）→ user(压缩指令) + assistant(总结) → 缓存重写
        {
            let mut ctx = session.context.lock().await;
            ctx.clear();
            for m in compressed_messages(&cfg, &summary) {
                ctx.push(m);
            }
        }
        let _ = self.cache.clear(&key).await;
        let msgs = { session.context.lock().await.build() };
        // 缓存不含 system：只存 user+assistant（恢复时 set_system 在前）
        let store: Vec<Message> = msgs.into_iter().filter(|m| !matches!(m, Message::System { .. })).collect();
        let _ = self.cache.append(&key, &store).await;
        info!("会话上下文已压缩: role={} mode={:?}", session.role_name, session.mode);
    }

    /// 读取自我认知（agent 元数据 + 个体识别 + 角色设定）生成系统提示词，agent_id 为解析后的 UUID
    /// 通过 ego_md 模块将 ego 结构转为 markdown，替代手写提示词片段
    async fn load_ego_info(&self, agent_id: &str, role_name: &str) -> Result<String> {
        let ego_url = kissbot_api::ApiConfig::get().memory_ego_url.clone();

        let client = reqwest::Client::new();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();

        let mut system_parts = vec![];

        // agent 自身活跃标识集合：来自各 channel 绑定身份（messenger_id, user_id；群组不限定）
        let mut ids = std::collections::HashSet::new();
        for (_, ch) in self.config.channels().await {
            for bu in &ch.bind_users {
                ids.insert(kissbot_api::ChannelUser {
                    messenger_id: bu.messenger_id.clone(),
                    user_id: bu.user_id.clone(),
                });
            }
        }
        // 匹配的个体名，用于角色设定的 other_roles 过滤
        let mut individual_names = std::collections::HashSet::new();

        // 1. agent 元数据（按 agent_id 查询）-> 身份 markdown
        if let Ok(agent_resp) = client.post(&format!("{}/agent/get", ego_url))
            .header(kissbot_security::HEADER_API_KEY, api_key.as_str())
            .json(&serde_json::json!({
                "agent_id": agent_id,
            }))
            .send()
            .await
        {
            if let Ok(envelope) = agent_resp.json::<kissbot_api::ApiResponse<kissbot_api::AgentMetadata>>().await {
                if let Some(metadata) = envelope.data {
                    system_parts.push(crate::ego_md::build_ego_identity_md(&metadata));
                }
            }
        }

        // 2. 个体识别（按 agent_id 查询）-> 个体识别 markdown，并收集匹配个体名
        if let Ok(individual_resp) = client.post(&format!("{}/individual/get-all", ego_url))
            .header(kissbot_security::HEADER_API_KEY, api_key.as_str())
            .json(&serde_json::json!({
                "agent_id": agent_id,
            }))
            .send()
            .await
        {
            if let Ok(envelope) = individual_resp.json::<kissbot_api::ApiResponse<kissbot_api::IndividualRecognition>>().await {
                if let Some(individuals) = envelope.data {
                    for (name, entry) in individuals.individual_map.iter() {
                        let individual = entry.load();
                        if individual.identifiers.iter().any(|id| ids.contains(id)) {
                            individual_names.insert(name.clone());
                        }
                    }
                    system_parts.push(crate::ego_md::build_ego_individual_recognition_md(&individuals, &ids));
                }
            }
        }

        // 3. 角色设定（按 agent_id + role_name 查询）-> 角色 markdown
        if !role_name.is_empty() {
            if let Ok(role_resp) = client.post(&format!("{}/role/get", ego_url))
                .header(kissbot_security::HEADER_API_KEY, api_key.as_str())
                .json(&serde_json::json!({
                    "agent_id": agent_id,
                    "role_name": role_name,
                }))
                .send()
                .await
            {
                if let Ok(envelope) = role_resp.json::<kissbot_api::ApiResponse<kissbot_api::RolePlay>>().await {
                    if let Some(role) = envelope.data {
                        system_parts.push(crate::ego_md::build_role_play_md(&role, &individual_names));
                    }
                }
            }
        }

        if system_parts.is_empty() {
            system_parts.push("你是 kissbot 智能助手".to_string());
        }

        Ok(system_parts.join("\n"))
    }

    // ==================== 运行状态修改（管理命令入口） ====================

    /// agent/role/mode 变更统一入口：应用新会话三元组（写 config agent_name/role_name + 运行态 mode + 可选 agent_id + 会话重定位），
    /// 走串行队列，返回时已生效
    pub async fn change_channel_key(&self, channel_id: &str, new_key: SessionKey, agent_id: Option<Arc<String>>) -> Result<()> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        self.command_tx.send(ConfigChange::ApplyKey {
            channel_id: channel_id.to_string(),
            new_key,
            agent_id,
            done: done_tx,
        }).map_err(|_| Error::InternalError("变更队列已关闭".to_string()))?;
        done_rx.await.map_err(|_| Error::InternalError("变更处理中断".to_string()))?
    }

    /// 取 channel 当前会话三元组（config 的 agent_name/role_name + 运行态 mode），命令构造新三元组用
    pub async fn channel_session_key(&self, channel_id: &str) -> Option<SessionKey> {
        let ch = self.config.channel(channel_id).await?;
        Some(SessionKey {
            agent_name: ch.agent_name.to_string(),
            role_name: ch.role_name.to_string(),
            mode: self.channel_mode(channel_id),
        })
    }

    // ---- 变更消费者（队列内串行执行，不对外） ----

    async fn apply_channel_key(&self, channel_id: &str, new_key: &SessionKey, agent_id: Option<Arc<String>>) -> Result<()> {
        self.config.update_channel(channel_id, |c| {
            c.agent_name = Arc::new(new_key.agent_name.clone());
            c.role_name = Arc::new(new_key.role_name.clone());
        }).await?;
        self.set_channel_mode(channel_id, new_key.mode.clone());
        if let Some(agent_id) = agent_id {
            self.set_channel_runtime(channel_id, agent_id).await;
        }
        self.relocate_channel(channel_id).await;
        // 会话重定位后刷新合批发送端（channel 绑定新的会话三元组，created 会话已由 ensure_session 绑定）
        if let Some(ch) = self.config.channel(channel_id).await {
            let key = self.session_key_for(&ch);
            if let Some(session) = self.session_manager.get(&key) {
                self.bind_batch_tx(channel_id, &session).await;
            }
        }
        Ok(())
    }

    /// 设置来源 channel 所属会话的模型（运行态，不回写；每次切换都从 API 拉模型列表校验）
    pub async fn set_session_model(&self, channel_id: &str, pm: ProviderModel) -> Result<()> {
        let Some(ch) = self.config.channel(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let key = self.session_key_for(&ch);
        // 每次切换都从 API 拉模型列表校验（失败拒绝，保持原模型）
        let models = self.model_client.lock().await.list_models(&pm).await
            .map_err(|e| Error::ModelApiError(format!("获取模型列表失败: {}", e)))?;
        if !models.iter().any(|m| m == &pm.model) {
            return Err(Error::ModelProviderNotSupported(format!(
                "模型 {} 不在 {} 的 API 模型列表", pm.model, pm.provider)));
        }
        let (session, _) = self.ensure_session(&key, channel_id).await;
        session.model.store(Arc::new(Some(pm)));
        Ok(())
    }

    /// 查询来源 channel 所属会话的事件列表
    pub async fn list_events(&self, channel_id: &str) -> Result<String> {
        let Some(ch) = self.config.channel(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let key = self.session_key_for(&ch);
        // 取记忆用会话保存的 agent_id
        let session = self.session_manager.get(&key)
            .ok_or_else(|| Error::ConfigNotFound(format!("会话不存在: {:?}", key)))?;
        let events = self.memory_reader
            .list_events(&self.config, session.agent_id.as_str(), &key.role_name)
            .await?;
        if events.is_empty() {
            Ok("📋 暂无事件".to_string())
        } else {
            Ok(format!("📋 事件列表:\n{}", events.join("\n")))
        }
    }

    // ==================== 通道连接 ====================

    /// 连接所有 enabled 的 channel（NexusRepo channel 配置为连接来源）
    /// 连接与绑定统一由 ChannelConfig 描述：enabled 控制连接，bind_users 为绑定身份（逐个绑定）
    async fn connect_channels(self: &Arc<Self>) {
        let reconnect_secs = self.config.ws_reconnect_interval_secs();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        let coordinator = self.clone();

        // 遍历 NexusRepo 中所有 channel，enabled 才连接
        for (_, ch) in self.config.channels().await {
            if !ch.enabled {
                continue; // 未启用：不连接
            }
            let channel_id = ch.channel_id.to_string();
            let ws_url = ch.ws_url.to_string();

            let client = ChannelClient::new(
                channel_id.clone(),
                Arc::downgrade(&(coordinator.clone() as Arc<dyn Terminal>)),
            );

            // 断线通知
            let notify = Arc::new(tokio::sync::Notify::new());
            coordinator.disconnect_notify.insert(channel_id.clone(), notify.clone());
            coordinator.channel_clients.insert(channel_id.clone(), client);

            let client_clone = coordinator.channel_clients.get(&channel_id).unwrap().clone();
            let api_key = api_key.clone();
            // 重连循环内实时读取绑定身份（/bind 回写后重连即生效），需持有 coordinator 引用
            let coordinator_clone = coordinator.clone();

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
                        Ok(()) => {
                            info!("已连接 channel: {}", channel_id);
                            // 绑定身份实时读取（bind_users 逐个绑定；BindRequest.messenger_id 用绑定身份的 messenger 标识，如 "web"）
                            let bind_users = coordinator_clone.config.channel(&channel_id).await
                                .map(|c| c.bind_users.clone());
                            if let Some(bus) = bind_users {
                                for bu in bus {
                                    let _ = client_clone.bind(BindRequest {
                                        messenger_id: Arc::new(bu.messenger_id.clone()),
                                        user_id: Arc::new(bu.user_id.clone()),
                                    }).await;
                                }
                            }
                            // 等待断线通知（closed() 回调中 notify_one）
                            notify.notified().await;
                        }
                        Err(e) => {
                            warn!("连接 channel {} 失败: {:?}，{}秒后重连", channel_id, e, reconnect_secs);
                            tokio::time::sleep(Duration::from_secs(reconnect_secs)).await;
                        }
                    }
                }
            });
        }
    }

    /// 启动主循环（保持进程运行）
    pub async fn run(&self) {
        info!("AgentCoordinator 启动，等待外部输入...");
        // channel-client 通过 Terminal 回调驱动，此处保持进程不退出
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
}

// ==================== Terminal trait 实现 ====================

#[async_trait]
impl Terminal for AgentCoordinator {
    /// 收到上行消息（event 含接收方 recipient_user_id）
    async fn incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        // 1. 来源 channel 必须在配置中
        let Some(ch) = self.config.channel(channel_id).await else { return; };

        // 2. msg_id 回显判定：命中（已发未回显）则跳过，不存 record、不进 agentic loop
        if self.is_self_echo_by_msg_id(channel_id, &event.incoming_message.msg_id).await {
            return;
        }

        // 3. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 取来源 channel 运行态绑定，事件模式编码）
        let key = self.session_key_for(&ch);
        let role_name = memory_role(&key.role_name, &key.mode);
        let agent_id = self.channel_agent(channel_id).await;
        self.memory_store_client.push_channel_record(ChannelRequest {
            agent_id,
            role_name: Arc::new(role_name),
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.incoming_message.user_id.clone(),
            // 接收方身份 = event.recipient_user_id（agent 视角的 self；与 is_self 不同，其他人用绑定用户发消息时 user_id == self_user_id 但 is_self == 0）
            self_user_id: event.recipient_user_id.clone(),
            group_id: event.incoming_message.group_id.clone(),
            is_self: 0,
            messenger_name: event.incoming_message.messenger_name.clone(),
            user_name: event.incoming_message.user_name.clone(),
            group_name: event.incoming_message.group_name.clone(),
            content: event.incoming_message.content.clone(),
            time: event.incoming_message.time.clone(),
        }).await;

        // 4. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
        self.handle_incoming(channel_id, ch, event).await;
    }

    async fn join_group(&self, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组加入事件，当前暂不处理
    }

    async fn leave_group(&self, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组离开事件，当前暂不处理
    }

    async fn user_removed(&self, _id: &str, _notification: Arc<UserRemoveNotification>) {
        // 用户删除事件，当前暂不处理
    }

    async fn download_chunk(&self, _id: &str, _info: Arc<AttachmentInfoResponse>, _pos: u64, _data: Bytes) -> std::result::Result<(), kissbot_channel_client::Error> {
        // 当前未使用附件下载
        Ok(())
    }

    async fn closed(&self, id: &str) {
        info!("channel 连接关闭: {}，准备重连", id);
        // 通知重连循环
        if let Some(notify) = self.disconnect_notify.get(id) {
            notify.notify_one();
        }
    }
}

// ==================== 消息处理 ====================

impl AgentCoordinator {
    async fn handle_incoming(
        &self,
        channel_id: &str,
        ch: Arc<crate::config_manager::ChannelConfig>,
        event: Arc<IncomingMessageEvent>,
    ) {
        let messenger_id = event.incoming_message.messenger_id.to_string();
        let user_id = event.incoming_message.user_id.to_string();
        let content_text = extract_text(&event.incoming_message.content);

        // 1. 系统事件（群组变更/用户移除）不进 agentic loop
        match &event.incoming_message.content {
            Content::GroupJoin(_) | Content::GroupLeave(_) | Content::UserRemove(_) => return,
            _ => {}
        }

        // 2. 管理命令（无论有无 out_channel 都处理；回复发回来源 channel）
        if CommandRouter::is_command(&content_text) {
            if CommandRouter::check_admin(&self.config, channel_id, &messenger_id, &user_id).await {
                self.handle_admin_command(channel_id, &event, &content_text).await;
            }
            // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
            return;
        }

        // 3. 普通消息：无 out_channel 不进 Agentic Loop（ChannelRecord 已存，结束）
        let Some(_out_channel) = self.resolve_out_channel(channel_id).await else {
            return;
        };
        let key = self.session_key_for(&ch);
        let (session, _) = self.ensure_session(&key, channel_id).await;
        self.enqueue_batch(channel_id, &session, event).await;
    }

    /// 合批：数据经 channel 持有的发送端入队（Arc<IncomingMessageEvent>）→ 更新截止时间（防抖）→ 发送触发时间（At）。
    /// 无 sleep、无逐消息任务——触发由 session 的 trigger 任务经 DelayQueue 定时处理。
    async fn enqueue_batch(&self, channel_id: &str, session: &Arc<Session>, event: Arc<IncomingMessageEvent>) {
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        let interval = Duration::from_secs(cfg.channel_batch_interval_secs);

        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(ChannelContext::new()))
            .clone();
        // 懒绑定：无发送端则从会话取（正常路径在 ensure_session 创建/apply_channel_key 已绑定）
        if ctx.batch_tx.lock().await.is_none() {
            self.bind_batch_tx(channel_id, session).await;
        }
        let (btx, ttx) = (ctx.batch_tx.lock().await.clone(), ctx.trigger_tx.lock().await.clone());
        if let (Some(btx), Some(ttx)) = (btx, ttx) {
            let _ = btx.send(event);                                // 数据入队（队列累积，不逐条消费）
            let at = Instant::now() + interval;                     // 单次计算：deadline 与触发时间同源
            session.batch.set_deadline(at);                         // 更新截止（防抖，后推覆盖）
            let _ = ttx.send(crate::batching::Trigger::At(at));     // 发送触发时间（绝对）
        } else {
            warn!("enqueue_batch: channel {} 无合批发送端", channel_id);
        }
    }

    async fn handle_admin_command(
        &self,
        channel_id: &str,
        event: &Arc<IncomingMessageEvent>,
        content: &str,
    ) {
        match CommandRouter::parse(content) {
            Ok(cmd) => {
                match CommandRouter::execute(&cmd, &self.config, self, channel_id).await {
                    Ok((reply, effect)) => {
                        // 回复：系统命令始终发回来源 channel（不走 out_channel）
                        self.send_admin_reply(channel_id, event, reply).await;

                        // 应用命令执行效果
                        match effect {
                            crate::types::CommandEffect::ResetSession => {
                                self.reset_session_for(channel_id).await;
                            }
                            crate::types::CommandEffect::None => {}
                        }
                    }
                    Err(e) => {
                        self.send_admin_reply(channel_id, event,
                            format!("❌ 命令执行失败: {}", e)).await;
                    }
                }
            }
            Err(e) => {
                self.send_admin_reply(channel_id, event,
                    format!("⚠️ {}", e)).await;
            }
        }
    }

    /// 系统命令回复：始终发回来源 channel（不走 out_channel）
    /// 身份：messenger_id = incoming.messenger_id；user_id/self_user_id = event.recipient_user_id（接收方即发声身份，且是群成员）
    async fn send_admin_reply(&self, channel_id: &str, event: &Arc<IncomingMessageEvent>, content: String) {
        let Some(client) = self.channel_clients.get(channel_id) else {
            warn!("send_admin_reply: 未找到 channel client: {}", channel_id);
            return;
        };
        let Some(ch) = self.config.channel(channel_id).await else {
            warn!("send_admin_reply: 未找到 channel 配置: {}", channel_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.recipient_user_id.clone(),
            group_id: event.incoming_message.group_id.clone(),
            content: Content::Text(Arc::new(content.clone())),
        };

        match client.send_message(msg).await {
            Ok(response) => {
                // 下行成功后：先记 msg_id 到 pending（回显判定），再推记忆（is_self=1）
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = self.channel_agent(channel_id).await;
                self.record_outgoing_msg_id(channel_id, &response.msg_id).await;
                self.memory_store_client.push_channel_record(ChannelRequest {
                    agent_id,
                    role_name: Arc::new(role_name),
                    messenger_id: event.incoming_message.messenger_id.clone(),
                    user_id: event.recipient_user_id.clone(),
                    self_user_id: event.recipient_user_id.clone(),
                    group_id: event.incoming_message.group_id.clone(),
                    is_self: 1,
                    messenger_name: response.messenger_name.clone(),
                    user_name: response.user_name.clone(),
                    group_name: response.group_name.clone(),
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;
            }
            Err(e) => {
                warn!("send_admin_reply 失败: {:?}", e);
            }
        }
    }

    /// 取来源 channel 所属 (agent,role) 的 out_channel（跨 channel 找有 outgoing 配置的，至多 1 个）
    /// out_channel 跟 channel 不跟 mode：该 channel 所有 mode 的 session 共用
    async fn resolve_out_channel(&self, channel_id: &str) -> Option<OutChannel> {
        let ch = self.config.channel(channel_id).await?;
        let channels = self.config.channels().await;
        for (_, c) in &channels {
            if c.agent_name == ch.agent_name && c.role_name == ch.role_name {
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

    /// 按会话 (agent, role) 找 out_channel（resolve_out_channel 的会话版，合批 trigger flush 用）
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

    /// 合批 trigger 任务打包后调用：解析 out_channel 并进入 agentic loop
    /// （batching.try_flush 升级槽后调用本方法）
    pub async fn flush_batch(&self, session: &Arc<Session>, content: String) {
        // 无可用模型：静默忽略（与 run_agentic_loop 入口一致）
        if session.model.load().is_none() {
            return;
        }
        let Some(out_channel) = self.resolve_out_channel_for_session(session).await else {
            warn!("flush_batch: 会话无 out_channel，跳过");
            return;
        };
        self.run_agentic_loop("", session, content, &out_channel).await;
    }

    async fn run_agentic_loop(&self, _channel_id: &str, session: &Arc<Session>, content_text: String, out_channel: &OutChannel) {
        // 无可用模型：静默忽略普通消息（仅管理指令可用）
        if session.model.load().is_none() {
            return;
        }

        // 会话 key（缓存定位）
        let key = self.session_key_of_session(session);

        // 1. 追加用户消息到该会话上下文（合批已打包为一条 user 消息，time/messenger 等不保留，只留文本）
        {
            let mut ctx = session.context.lock().await;
            ctx.push(Message::User { content: Arc::new(content_text.clone()) });
        }
        // 1b. 用户消息写缓存（best-effort，失败仅丢缓存不阻塞流程）
        let _ = self.cache.append(&key, &[Message::User { content: Arc::new(content_text.clone()) }]).await;

        // 2. tools 聚合（会话 context 配置的启用 station）
        let tools = self.tools_for_session(session).await;

        // 3. 多轮工具循环：LLM 返回 tool_calls 则执行工具并继续，直到返回最终回复（上限 MAX_TOOL_ROUNDS 防死循环）
        let mut rounds = 0;
        loop {
            rounds += 1;
            let response = {
                let ctx = session.context.lock().await;
                let messages = ctx.build();
                let model = session.model.load_full();
                let Some(pm) = model.as_ref() else { return; };
                let mc = self.model_client.lock().await;
                mc.call(pm, &messages, &tools).await
            };

            match response {
                Ok(model_resp) if !model_resp.tool_calls.is_empty() && rounds <= MAX_TOOL_ROUNDS => {
                    // 4. 追加 assistant(tool_calls) + 写缓存
                    // reasoning_content 必须保留并随请求回传：DeepSeek 带 tools 的请求须完整回传否则 400；
                    // Kimi 单轮工具循环（多步推理）须保留并回传全部思考内容；openai_body 自动序列化
                    {
                        let mut ctx = session.context.lock().await;
                        ctx.push(Message::Assistant {
                            content: Arc::new(String::new()),
                            reasoning_content: model_resp.reasoning_content.clone().map(Arc::new),
                            tool_calls: Some(model_resp.tool_calls.clone()),
                        });
                    }
                    let _ = self.cache.append(&key, &[Message::Assistant {
                        content: Arc::new(String::new()),
                        reasoning_content: model_resp.reasoning_content.clone().map(Arc::new),
                        tool_calls: Some(model_resp.tool_calls.clone()),
                    }]).await;

                    // 5. 逐个执行 tool call → 工具 key + channel 占位记录 + Tool 消息 + 写缓存 + 记忆写入
                    // 5a/5f 占位与 5e 详情共用同一 key（经 channel 时间线关联），仿 think 流程
                    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                    let role_name = memory_role(session.role_name.as_str(), &session.mode);
                    let agent_id = session.agent_id.clone();
                    for call in &model_resp.tool_calls {
                        // 5a. 工具调用 key：UUID（ToolCall/ToolResult 详情与 channel 占位同 key 关联）
                        let tool_key = uuid::Uuid::new_v4().to_string();
                        // 5b. channel 占位记录（仿 think 流程，身份来自 out_channel，is_self=1）
                        let call_placeholder = tool_placeholder_request(session, out_channel, &tool_key, false, &now);
                        self.memory_store_client.push_channel_record(call_placeholder).await;
                        // 5c. 执行工具
                        let result = self.execute_tool_call(session, call).await;
                        let result_text = result.to_string();
                        {
                            let mut ctx = session.context.lock().await;
                            ctx.push(Message::Tool { tool_call_id: call.id.clone(), name: call.name.clone(), content: Arc::new(result_text.clone()) });
                        }
                        let _ = self.cache.append(&key, &[Message::Tool { tool_call_id: call.id.clone(), name: call.name.clone(), content: Arc::new(result_text.clone()) }]).await;
                        // 5e. 记忆写入：ToolCallRequest.key 与 ToolResultRequest.key 用同一 key（agent_id 取会话状态，role_name 含事件编码）
                        self.memory_store_client.push_tool_call(ToolCallRequest {
                            agent_id: agent_id.clone(),
                            role_name: Arc::new(role_name.clone()),
                            tool_name: call.name.clone(),
                            tool_params: call.arguments.clone(),
                            key: Arc::new(tool_key.clone()),
                            time: Arc::new(now.clone()),
                        }).await;
                        self.memory_store_client.push_tool_result(ToolResultRequest {
                            agent_id: agent_id.clone(),
                            role_name: Arc::new(role_name.clone()),
                            tool_result: Arc::new(result.clone()),
                            key: Arc::new(tool_key.clone()),
                            time: Arc::new(now.clone()),
                        }).await;
                        // 5f. tool-result 占位记录（同 key）
                        let result_placeholder = tool_placeholder_request(session, out_channel, &tool_key, true, &now);
                        self.memory_store_client.push_channel_record(result_placeholder).await;
                    }
                    continue;  // 继续下一轮
                }
                Ok(model_resp) => {
                    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

                    // 6. 追加 assistant 回复 + 写缓存
                    // 超限兜底：rounds 超过 MAX_TOOL_ROUNDS 后模型仍返回 tool_calls 时 content 为空，
                    // 用兜底文案作为回复（不把空内容发送给用户）
                    // reasoning_content 保留并回传：带 tools 的请求须完整回传所有 assistant 的思考内容
                    // （DeepSeek 400 规则 / Kimi 保留式思考）；同时思考内容在步骤 7 写 memory-store think 记录
                    let reply_content = if model_resp.tool_calls.is_empty() {
                        model_resp.content.clone()
                    } else {
                        "工具调用轮次已达上限，请稍后再试".to_string()
                    };
                    {
                        let mut ctx = session.context.lock().await;
                        ctx.push(Message::Assistant {
                            content: Arc::new(reply_content.clone()),
                            reasoning_content: model_resp.reasoning_content.clone().map(Arc::new),
                            tool_calls: None,
                        });
                    }
                    let _ = self.cache.append(&key, &[Message::Assistant {
                        content: Arc::new(reply_content.clone()),
                        reasoning_content: model_resp.reasoning_content.clone().map(Arc::new),
                        tool_calls: None,
                    }]).await;

                    // 7. 推送 think 到 memory-store（reasoning_content + thinking 双字段，key 关联 ChannelRecord(Think)）
                    // 身份来自 out_channel；任一有值才写，都 None 跳过
                    if should_write_think(model_resp.reasoning_content.as_deref(), model_resp.thinking.as_deref()) {
                        let key_uuid = uuid::Uuid::new_v4().to_string();
                        let role_name = memory_role(session.role_name.as_str(), &session.mode);
                        let agent_id = session.agent_id.clone();

                        // 7a. ChannelRecord(Think(key)) 写主时间线（身份来自 out_channel，is_self=1）
                        self.memory_store_client.push_channel_record(ChannelRequest {
                            agent_id: agent_id.clone(),
                            role_name: Arc::new(role_name.clone()),
                            messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
                            user_id: Arc::new(out_channel.user.user_id.clone()),
                            self_user_id: Arc::new(out_channel.user.user_id.clone()),
                            group_id: out_channel.group_id.clone(),
                            is_self: 1,
                            messenger_name: Arc::new(String::new()),   // 占位：详情经 key 关联，name 非关键
                            user_name: Arc::new(String::new()),
                            group_name: Arc::new(String::new()),
                            content: Content::Think(Arc::new(key_uuid.clone())),
                            time: Arc::new(now.clone()),
                        }).await;

                        // 7b. ThinkRequest(key, reasoning_content, thinking) 写详情
                        self.memory_store_client.push_think(ThinkRequest {
                            agent_id,
                            role_name: Arc::new(role_name),
                            reasoning_content: Arc::new(model_resp.reasoning_content.clone().unwrap_or_default()),
                            thinking: Arc::new(model_resp.thinking.clone().unwrap_or_default()),
                            key: Arc::new(key_uuid),
                            time: Arc::new(now.clone()),
                        }).await;
                    }

                    // 8. 发送回复到该会话的 out_channel
                    self.send_outgoing(out_channel, reply_content).await;
                    break;
                }
                Err(e) => {
                    warn!("模型调用失败: {:?}", e);
                    self.send_outgoing(out_channel,
                        format!("❌ 模型调用失败: {}", e)).await;
                    break;
                }
            }
        }

        // 9. 检查上下文超长（阈值来自会话模型的 effective.max_context_messages）
        let overflow = {
            let ctx = session.context.lock().await;
            let model = session.model.load_full();
            match model.as_ref() {
                Some(pm) => match self.config.resolve_effective_config(pm).await {
                    Some(eff) => ctx.is_overflow(eff.max_context_messages as usize),
                    None => false,
                },
                None => false,
            }
        };
        if overflow {
            warn!("会话上下文超长，触发重置: role={} mode={:?}", session.role_name, session.mode);
            // 按模式处理：event 超长压缩（LLM 总结归档），role 归档后从记忆重建
            match &*session.mode {
                Mode::Event(_) => self.compress_context(session).await,
                Mode::Role => self.reset_context(session).await,
            }
        }
    }

    /// 会话可用工具：context 配置的启用的 stations ∩ 实际配置的 station → 收集 ToolConfig
    /// tools 聚合为空则请求不携带 tools 字段（兼容无工具场景）
    async fn tools_for_session(&self, session: &Arc<Session>) -> Vec<ToolConfig> {
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        let mut tools = Vec::new();
        for entry in self.station_runtimes.iter() {
            let (station_id, runtime) = entry.pair();
            if cfg.stations.contains(station_id.as_str()) {
                tools.extend(runtime.configured_tools());
            }
        }
        tools
    }

    /// 执行单个 tool call：在启用的 station 中查找并调用；找不到/调用失败返回错误 JSON
    async fn execute_tool_call(&self, session: &Arc<Session>, call: &crate::types::ToolCall) -> serde_json::Value {
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        // 先克隆出 Arc 列表（释放 DashMap 全局读锁），再逐项 await（不跨 await 持锁）
        let runtimes: Vec<(String, Arc<StationRuntime>)> = self.station_runtimes.iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        for (station_id, runtime) in &runtimes {
            if cfg.stations.contains(station_id.as_str()) && runtime.has_tool(call.name.as_str()) {
                match runtime.call_tool(call.name.as_str(), (*call.arguments).clone()).await {
                    Ok(v) => return v,
                    Err(e) => return serde_json::json!({ "error": e.to_string() }),
                }
            }
        }
        serde_json::json!({ "error": format!("工具不存在: {}", call.name) })
    }

    /// Agentic Loop 产出回复：发到 out_channel（channel_id + ChannelUser + group_id）
    async fn send_outgoing(&self, out_channel: &OutChannel, content: String) {
        let Some(client) = self.channel_clients.get(out_channel.channel_id.as_str()) else {
            warn!("send_outgoing: 未找到 channel client: {}", out_channel.channel_id);
            return;
        };
        let Some(ch) = self.config.channel(out_channel.channel_id.as_str()).await else {
            warn!("send_outgoing: 未找到 channel 配置: {}", out_channel.channel_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
            user_id: Arc::new(out_channel.user.user_id.clone()),
            group_id: out_channel.group_id.clone(),
            content: Content::Text(Arc::new(content.clone())),
        };

        match client.send_message(msg).await {
            Ok(response) => {
                // 下行成功后：先记 msg_id 到 pending（回显判定），再推记忆（is_self=1）
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = self.channel_agent(out_channel.channel_id.as_str()).await;
                self.record_outgoing_msg_id(out_channel.channel_id.as_str(), &response.msg_id).await;
                self.memory_store_client.push_channel_record(ChannelRequest {
                    agent_id,
                    role_name: Arc::new(role_name),
                    messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
                    user_id: Arc::new(out_channel.user.user_id.clone()),
                    self_user_id: Arc::new(out_channel.user.user_id.clone()),
                    group_id: out_channel.group_id.clone(),
                    is_self: 1,
                    messenger_name: response.messenger_name.clone(),
                    user_name: response.user_name.clone(),
                    group_name: response.group_name.clone(),
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;
            }
            Err(e) => {
                warn!("send_outgoing 失败: {:?}", e);
            }
        }
    }
}

/// 从 Content 枚举中提取文本（batching.pack_events 复用，pub(crate)）
pub(crate) fn extract_text(content: &Content) -> String {
    match content {
        Content::Text(t) => t.as_str().to_string(),
        Content::Multi(items) => items.iter()
            .filter_map(|c| match c { Content::Text(t) => Some(t.as_str().to_string()), _ => None })
            .collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// think 写入条件：reasoning_content 或 thinking 任一有值才写（都 None 跳过）
fn should_write_think(reasoning: Option<&str>, thinking: Option<&str>) -> bool {
    reasoning.is_some() || thinking.is_some()
}

/// 工具占位记录构造（仿 think 的 ChannelRecord(Think) 流程）：返回 ChannelRequest
/// is_result=false → Content::ToolCall(key)；is_result=true → Content::ToolResult(key)
/// 身份来自 out_channel（is_self=1，self_user=绑定用户）；role_name 含事件编码；详情经 key 关联
/// 自由函数（不依赖 coordinator 状态），便于单测
fn tool_placeholder_request(
    session: &Arc<Session>,
    out_channel: &OutChannel,
    key: &str,
    is_result: bool,
    now: &str,
) -> ChannelRequest {
    let role_name = memory_role(session.role_name.as_str(), &session.mode);
    let content = if is_result { Content::ToolResult(Arc::new(key.to_string())) }
                  else { Content::ToolCall(Arc::new(key.to_string())) };
    ChannelRequest {
        agent_id: session.agent_id.clone(),
        role_name: Arc::new(role_name),
        messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
        user_id: Arc::new(out_channel.user.user_id.clone()),
        self_user_id: Arc::new(out_channel.user.user_id.clone()),
        group_id: out_channel.group_id.clone(),
        is_self: 1,
        messenger_name: Arc::new(String::new()),   // 占位：详情经 key 关联，name 非关键
        user_name: Arc::new(String::new()),
        group_name: Arc::new(String::new()),
        content,
        time: Arc::new(now.to_string()),
    }
}

/// 压缩后上下文（不含 system）：user(压缩指令) + assistant(总结)
/// 由 compress_context 重建内存上下文用；抽为纯函数便于测试
fn compressed_messages(cfg: &EffectiveContextConfig, summary: &str) -> Vec<Message> {
    vec![
        Message::User { content: Arc::new(cfg.compress_prompt.clone()) },
        Message::Assistant { content: Arc::new(summary.to_string()), reasoning_content: None, tool_calls: None },
    ]
}

/// 按 (agent_name, role_name, mode) 三元组计算会话 key（session_key_for 的纯函数版，便于测试）；
/// 无脱离态：agent_name 为空 = 保留 agent
fn session_key_of(agent_name: &str, role_name: &str, mode: Mode) -> SessionKey {
    SessionKey {
        agent_name: agent_name.to_string(),
        role_name: role_name.to_string(),
        mode,
    }
}

/// resolve_agent_id 的纯函数实现（便于单测）：不缓存（agent_name->agent_id 关联只在启动绑定/切换 agent 时确定）；
/// 空 agent_name（保留 agent）返回 Ok(保留 id)；ego 未配置/HTTP 失败/无匹配返回 Err（调用方决定回退策略）
async fn resolve_agent_id_http(agent_name: &str, ego_url: &str) -> Result<Arc<String>> {
    if agent_name.is_empty() {
        return Ok(Arc::new(RESERVED_AGENT_ID.to_string()));
    }
    if ego_url.is_empty() {
        return Err(Error::MemoryEgoError("ego 未配置（memory_ego_url 为空）".to_string()));
    }
    let client = reqwest::Client::new();
    let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
    let resp = client.post(format!("{}/agent/search-name", ego_url))
        .header(kissbot_security::HEADER_API_KEY, api_key.as_str())
        .json(&serde_json::json!({ "keyword": agent_name }))
        .send()
        .await
        .map_err(|e| Error::MemoryEgoError(format!("search-name 请求失败: {}", e)))?;
    let data: serde_json::Value = resp.json().await
        .map_err(|e| Error::MemoryEgoError(format!("search-name 响应解析失败: {}", e)))?;
    match data["data"].as_str() {
        Some(id) if !id.is_empty() => Ok(Arc::new(id.to_string())),
        _ => Err(Error::MemoryEgoError(format!("search-name 未找到 agent: {}", agent_name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_context_msg_id_consume() {
        let ctx = ChannelContext::new();
        // 加入后命中且消费移除
        ctx.add_pending("msg1".to_string());
        assert!(ctx.consume_pending("msg1"));
        // 已消费，再次查询为 false
        assert!(!ctx.consume_pending("msg1"));
        // 未加入的 msg_id
        assert!(!ctx.consume_pending("nonexistent"));
    }

    #[test]
    fn channel_context_ttl_evict() {
        let ctx = ChannelContext::new();
        // TTL=0：插入即过期（走真实 add_pending 路径），下次操作即被淘汰
        ctx.add_pending("expired".to_string());
        ctx.evict(Duration::from_secs(0));
        assert!(!ctx.consume_pending("expired"), "TTL=0 插入即过期，应被淘汰");
        // 正常 TTL：未过期条目保留
        ctx.add_pending("fresh".to_string());
        ctx.evict(Duration::from_secs(CHANNEL_CONTEXT_TTL_SECS));
        assert!(ctx.consume_pending("fresh"));
    }

    #[test]
    fn session_key_of_always_builds_key() {
        // 无脱离态：agent_name 为空 = 保留 agent（建会话，agent_id="0"）
        let key = session_key_of("", "0", Mode::Role);
        assert_eq!(key.agent_name, "");
        assert_eq!(key.role_name, "0");
        assert_eq!(key.mode, Mode::Role);
        // 保留 role 为空串
        let key = session_key_of("a1", "", Mode::Role);
        assert_eq!(key.agent_name, "a1");
        assert_eq!(key.role_name, "");
        // 普通 agent（含事件模式）
        let key = session_key_of("a1", "r1", Mode::Event("e1".into()));
        assert_eq!(key.agent_name, "a1");
        assert_eq!(key.role_name, "r1");
        assert_eq!(key.mode, Mode::Event("e1".into()));
    }

    #[tokio::test]
    async fn resolve_agent_id_http_empty_returns_reserved() {
        // 空 agent_name（保留 agent）-> Ok("0")，无需 ego
        let r = resolve_agent_id_http("", "http://127.0.0.1:1").await;
        assert_eq!(r.unwrap().as_str(), RESERVED_AGENT_ID);
    }

    #[tokio::test]
    async fn resolve_agent_id_http_ego_unconfigured_errors() {
        // ego_url 为空（ego 未配置）-> Err（启动绑定回退保留 agent）
        let r = resolve_agent_id_http("alice", "").await;
        assert!(r.is_err(), "ego 未配置应 Err");
    }

    #[tokio::test]
    async fn resolve_agent_id_http_unreachable_errors() {
        // ego_url 指向不可达端口 -> 连接失败 Err（启动绑定回退保留 agent）
        let r = resolve_agent_id_http("carol", "http://127.0.0.1:1").await;
        assert!(r.is_err(), "不可达应 Err");
    }

    #[test]
    fn think_write_condition_any_non_empty() {
        // 任一有值则写
        assert!(should_write_think(Some("r".into()), None));
        assert!(should_write_think(None, Some("t".into())));
        assert!(should_write_think(Some("r".into()), Some("t".into())));
        // 都 None 不写
        assert!(!should_write_think(None, None));
    }

    #[test]
    fn compress_builds_prompt_summary_sequence() {
        let cfg = EffectiveContextConfig {
            channel_batch_interval_secs: 3,
            memory_time_secs: 3600,
            memory_count: 50,
            compress_prompt: "总结以上对话".into(),
            stations: std::collections::HashSet::new(),
        };
        let msgs = compressed_messages(&cfg, "总结内容");
        assert_eq!(msgs.len(), 2);
        assert!(matches!(&msgs[0], Message::User { content } if content.as_str() == "总结以上对话"));
        assert!(matches!(&msgs[1], Message::Assistant { content, .. } if content.as_str() == "总结内容"));
    }

    #[tokio::test]
    async fn tool_placeholder_uses_same_key_for_call_and_result() {
        // 构造最小 Session + OutChannel（参照既有测试模式）
        let key = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into())));
        let out_channel = OutChannel {
            channel_id: Arc::new("c1".into()),
            user: ChannelUser { messenger_id: "web".into(), user_id: "u1".into() },
            group_id: Arc::new("g1".into()),
        };

        let tool_key = uuid::Uuid::new_v4().to_string();
        let call = tool_placeholder_request(&session, &out_channel, &tool_key, false, "2026-08-05 10:00:00");
        let result = tool_placeholder_request(&session, &out_channel, &tool_key, true, "2026-08-05 10:00:01");
        // 占位内容携带同一 key（call=ToolCall、result=ToolResult）
        assert!(matches!(&call.content, Content::ToolCall(k) if k.as_str() == tool_key));
        assert!(matches!(&result.content, Content::ToolResult(k) if k.as_str() == tool_key));
        // 身份来自 out_channel（is_self=1，self_user=绑定用户）
        assert_eq!(call.is_self, 1);
        assert_eq!(call.self_user_id.as_str(), "u1");
        assert_eq!(result.group_id.as_str(), "g1");
    }
}
