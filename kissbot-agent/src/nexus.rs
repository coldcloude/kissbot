use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use arc_swap::ArcSwap;
use tracing::{info, warn};

use crate::channel_manager::ChannelManager;
use crate::types::{
    ChannelCommand, Error, Message, Mode, ModelResponse, RESERVED_AGENT_ID, Result,
    SessionKey, ToolCall, memory_role,
};
use crate::session_manager::{Session, SessionManager};
use crate::station::Station;
use crate::config_manager::{ConfigManager, ProviderModel, OutChannel, ToolConfig};
use crate::command_router::CommandRouter;
use crate::model_client::ModelClient;
use crate::message::pack_memory_messages;
use crate::memory_ego_client::MemoryEgoClient;
use crate::memory_store_client::MemoryStoreClient;

use kissbot_api::channel::{IncomingMessageEvent, OutgoingMessage, ChannelUser};
use kissbot_api::memory::{ChannelRequest, ThinkRequest, ToolCallRequest, ToolResultRequest};
use kissbot_api::message::Content;
/// 保留 role：空串 = 保留 role
pub const RESERVED_ROLE_NAME: &str = "";

// 上下文消息数量上限（溢出触发重置/压缩）已废弃硬编码常量 MAX_CONTEXT_MESSAGES——
// 阈值统一由会话模型 effective.max_context_messages（provider/model 配置合成）决定，见 run_agentic_loop 溢出检查。

/// agent/role/event 变更任务（mpsc 队列串行处理，避免写-写竞态；读无需外部加锁）
/// 统一为「应用新的会话三元组」：写 config + 运行态 mode + 会话重定位
enum ConfigChange {
    /// 应用新会话三元组（agent/role/mode 独立 Option，None = 保持当前值；队列内结合当前状态合成新三元组）
    ApplyKey { channel_id: String, agent_id: Option<String>, role_name: Option<String>, mode: Option<Mode>, done: tokio::sync::oneshot::Sender<Result<()>> },
}

/// channel 配置变更任务（排队调 ChannelManager 方法执行；与 ConfigChange 同消费者串行，写-写无竞态）
struct ChannelTask {
    cmd: ChannelCommand,
    done: tokio::sync::oneshot::Sender<Result<()>>,
}

/// Nexus 全局单例（进程内唯一；new() 完成时注册，此后 get() 可用）。
/// 所有使用 coordinator 的位置一律不传参数、从单例获取（Session/Channel 不保存引用）。
static SINGLETON: OnceLock<Nexus> = OnceLock::new();

pub struct Nexus {
    memory_store_client: Arc<MemoryStoreClient>,
    /// ego 服务 REST 客户端（共享连接池；system_prompt_for_agent / verify_agent_exists 经它发请求）
    memory_ego_client: Arc<MemoryEgoClient>,
    session_manager: Arc<SessionManager>,
    model_client: Arc<ModelClient>,
    /// 启动校验后的 default_model（从 API 模型列表校验）；None = 无模型（普通消息静默忽略）
    valid_default: ArcSwap<Option<ProviderModel>>,
    /// 每 channel 运行时管理（ChannelManager：内部 DashMap 无锁并发，含 pending/mode/client）
    channel_manager: Arc<ChannelManager>,
    /// agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
    command_tx: tokio::sync::mpsc::UnboundedSender<ConfigChange>,
    /// channel 配置变更串行队列（bind/unbind/bind-outgoing/clear-outgoing；与 ConfigChange 同一消费者 select! 等待）
    channel_task_tx: tokio::sync::mpsc::UnboundedSender<ChannelTask>,
}

impl Nexus {
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn get() -> &'static Nexus {
        SINGLETON.get().expect("Nexus 未初始化")
    }

    pub async fn new() -> Result<()> {
        let config = ConfigManager::get();
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let memory_ego_client = Arc::new(MemoryEgoClient::new());
        let data_dir = config.data_dir().to_string();
        let session_manager = SessionManager::new(&data_dir);
        let model_client = ModelClient::new();
        // agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<ConfigChange>();
        // channel 配置变更串行队列（bind/unbind/bind-outgoing/clear-outgoing；与 ConfigChange 同一消费者 select! 等待）
        let (channel_task_tx, mut channel_task_rx) = tokio::sync::mpsc::unbounded_channel::<ChannelTask>();

        let coordinator = Self {
            memory_store_client,
            memory_ego_client,
            session_manager,
            model_client: Arc::new(model_client),
            channel_manager: Arc::new(ChannelManager::new()),
            valid_default: ArcSwap::from_pointee(None),
            command_tx,
            channel_task_tx,
        };

        // 启动校验 default_model：从 API 拉模型列表，不在列表则无模型（告警）
        let default_model = config.default_model().await;
        match coordinator.verify_model(&default_model).await {
            Ok(()) => {
                coordinator.valid_default.store(Arc::new(Some(default_model)));
            },
            Err(e) => {
                warn!("校验 default_model {}/{} 失败: {}", default_model.provider, default_model.model, e);
            },
        };

        // 启动动作（绑定运行态 agent / 初始化会话 / 连接 channel）统一在 run() 中执行

        // 注册全局单例（此后 get() 可用；run() 中启动动作与连接回调均晚于此）
        let _ = SINGLETON.set(coordinator);

        // 启动变更消费者：agent/role/event 变更 + channel 配置变更串行处理（避免写-写竞态；读不受影响）
        // 两队列经 select! 合并到同一消费者，所有 channel 配置写全局串行
        // spawn 晚于 SINGLETON.set，任务内 get() 必然就绪
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    change = command_rx.recv() => {
                        match change {
                            Some(ConfigChange::ApplyKey { channel_id, agent_id, role_name, mode, done }) => {
                                let coordinator = Nexus::get();
                                let rst = coordinator.apply_channel_key(&channel_id, agent_id, role_name, mode).await;
                                let _ = done.send(rst);
                            }
                            // 任一队列关闭则消费者退出（进程内 tx 存于单例不会发生，break 仅防御）
                            None => break,
                        }
                    }
                    task = channel_task_rx.recv() => {
                        match task {
                            Some(ChannelTask { cmd, done }) => {
                                let coordinator = Nexus::get();
                                let rst = coordinator.apply_channel_command(cmd).await;
                                let _ = done.send(rst);
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        info!("Nexus 初始化完成");
        Ok(())
    }

    // ==================== 会话定位与构建 ====================

    /// 校验 agent_id 存在（/agent 切换前调用）：空或保留 id "0" 直接通过；
    /// ego 未配置/HTTP 失败/agent 不存在返回 Err（调用方保持原 agent 不变）
    pub async fn verify_agent_exists(&self, agent_id: &str) -> Result<()> {
        if agent_id.is_empty() || agent_id == RESERVED_AGENT_ID {
            return Ok(());
        }
        if self.memory_ego_client.get_agent(agent_id).await?.is_some() {
            Ok(())
        } else {
            Err(Error::MemoryEgoError(format!("agent 不存在: {}", agent_id)))
        }
    }

    /// 校验 role 存在（apply_channel_key 应用前调用）：显式空串（保留 role）直接通过；
    /// 其余必须 ego 中存在，否则 Err（调用方保持原 role 不变）
    pub async fn verify_role_exists(&self, agent_id: &str, role_name: &str) -> Result<()> {
        if role_name.is_empty() {
            return Ok(());
        }
        if self.memory_ego_client.get_role(agent_id, role_name).await?.is_some() {
            Ok(())
        } else {
            Err(Error::MemoryEgoError(format!("role 不存在: {}", role_name)))
        }
    }

    /// 校验 role 存在（/role 切换前调用）：经 channel_id 取当前 agent_id（role 变更保持 agent 不变，
    /// channel 不存在报 ConfigNotFound）；显式空串（保留 role）直接通过，其余必须 ego 中存在
    pub async fn verify_role_exists_for_channel(&self, channel_id: &str, role_name: &str) -> Result<()> {
        let Some(key) = self.channel_manager.session_key(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        self.verify_role_exists(&key.agent_id, role_name).await
    }

    /// 定位会话（不存在则创建；创建时上下文恢复/重建 + 系统消息在 get_or_create 内部完成）；返回会话（无"是否新建"标记）
    async fn ensure_session(&self, key: &SessionKey) -> Arc<Session> {
        // load_full() 直接返回 Arc<Option<ProviderModel>>（O(1)），零深拷贝传给 get_or_create
        let model = self.valid_default.load_full();
        self.session_manager.get_or_create(key, model).await
    }

    /// role 模式上下文构建（新建/溢出重置共用）：查询记忆打包 → 归档旧上下文+清空缓存（内部幂等）→ 重建
    /// 取记忆用会话状态保存的 agent_id（来自会话 key）
    pub async fn build_context_from_memory_store(&self, agent_id: Arc<String>, role_name: Arc<String>) -> Vec<Message> {
        let cfg = ConfigManager::get().context_config(agent_id.as_str(), role_name.as_str()).await;
        self.memory_store_client
            .read_recent_for_context(agent_id, role_name, cfg.memory_time_secs, cfg.memory_count).await
            .map_or_else(|_| vec![], |msgs| pack_memory_messages(&msgs))
    }

    /// 按当前全部 channel 的绑定集合清理无绑定会话
    async fn prune_sessions(&self) {
        let channels = ConfigManager::get().channels().await;
        let mut keys = HashSet::new();
        for (_, ch) in &channels {
            if let Some(key) = self.channel_manager.session_key(ch.channel_id.as_str()).await {
                keys.insert(key);
            }
        }
        self.session_manager.retain(&keys);
    }

    /// 根据 agent_id 获取系统提示词（新建会话系统消息，create_session 内调用）：
    /// 保留 agent（agent_id="0"）用 NexusRepo 默认系统提示词；其余走 ego REST（agent 元数据 + 个体识别 + 角色设定，
    /// 失败静默跳过，全部失败回退默认提示词"你是 kissbot 智能助手"）；
    /// 通过 ego_md 模块将 ego 结构转为 markdown，替代手写提示词片段
    pub async fn system_prompt_for_agent(&self, agent_id: &str, role_name: &str) -> Result<String> {
        if agent_id == RESERVED_AGENT_ID {
            return Ok(ConfigManager::get().default_system_prompt().await);
        }
        let mut system_parts = vec![];

        // agent 自身活跃标识集合：来自各 channel 绑定身份（messenger_id, user_id；群组不限定）
        let mut ids = std::collections::HashSet::new();
        for (_, ch) in ConfigManager::get().channels().await {
            for bu in ch.bind_users.iter() {
                ids.insert(kissbot_api::ChannelUser {
                    messenger_id: bu.messenger_id.clone(),
                    user_id: bu.user_id.clone(),
                });
            }
        }
        // 匹配的个体名，用于角色设定的 other_roles 过滤
        let mut individual_names = std::collections::HashSet::new();

        // 1. agent 元数据（按 agent_id 查询）-> 身份 markdown
        if let Ok(Some(metadata)) = self.memory_ego_client.get_agent(agent_id).await {
            system_parts.push(crate::ego_md::build_ego_identity_md(&metadata));
        }
        // 2. 个体识别（按 agent_id 查询）-> 个体识别 markdown，并收集匹配个体名
        if let Ok(Some(individuals)) = self.memory_ego_client.get_individuals(agent_id).await {
            for (name, entry) in individuals.individual_map.iter() {
                let individual = entry.load();
                if individual.identifiers.iter().any(|id| ids.contains(id)) {
                    individual_names.insert(name.clone());
                }
            }
            system_parts.push(crate::ego_md::build_ego_individual_recognition_md(&individuals, &ids));
        }
        // 3. 角色设定（按 agent_id + role_name 查询）-> 角色 markdown
        if !role_name.is_empty() {
            if let Ok(Some(role)) = self.memory_ego_client.get_role(agent_id, role_name).await {
                system_parts.push(crate::ego_md::build_role_play_md(&role, &individual_names));
            }
        }

        if system_parts.is_empty() {
            system_parts.push("你是 kissbot 智能助手".to_string());
        }

        Ok(system_parts.join("\n"))
    }

    // ==================== 运行状态修改（管理命令入口） ====================

    /// agent/role/mode 变更统一入口：三个字段独立 Option，None = 保持当前值；
    /// Nexus 结合 channel_manager 当前状态合成新三元组（写 config agent_id/role_name + 运行态 mode + 会话重定位），
    /// 走串行队列，返回时已生效
    pub async fn change_channel_key(
        &self,
        channel_id: &str,
        agent_id: Option<String>,
        role_name: Option<String>,
        mode: Option<Mode>,
    ) -> Result<()> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        self.command_tx.send(ConfigChange::ApplyKey {
            channel_id: channel_id.to_string(),
            agent_id,
            role_name,
            mode,
            done: done_tx,
        }).map_err(|_| Error::InternalError("变更队列已关闭".to_string()))?;
        done_rx.await.map_err(|_| Error::InternalError("变更处理中断".to_string()))?
    }

    /// channel 配置变更统一入口（/bind、/unbind、/bind-outgoing、/unbind-outgoing）：
    /// 排队调 ChannelManager 方法执行，与 change_channel_key 同一消费者串行；返回时已生效
    pub async fn channel_command(&self, cmd: ChannelCommand) -> Result<()> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        self.channel_task_tx.send(ChannelTask { cmd, done: done_tx })
            .map_err(|_| Error::InternalError("变更队列已关闭".to_string()))?;
        done_rx.await.map_err(|_| Error::InternalError("变更处理中断".to_string()))?
    }

    // ---- 变更消费者（队列内串行执行，不对外） ----

    /// 来源 channel 绑定信息变化后重定位会话：清理无绑定会话 + 为新三元组创建会话（apply_channel_key 专用）
    /// 运行态 mode 写 Channel.mode（/mode 切换不回写，重启回 Role）
    /// None 字段 = 保持当前值：队列内结合 channel_manager 当前状态合成新三元组（写-写串行，读-改-写无竞态）
    async fn apply_channel_key(
        &self,
        channel_id: &str,
        agent_id: Option<String>,
        role_name: Option<String>,
        mode: Option<Mode>,
    ) -> Result<()> {
        // 结合当前会话三元组生成新 key（None 字段保持当前值）
        let Some(cur) = self.channel_manager.session_key(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let new_key = SessionKey {
            agent_id: agent_id.unwrap_or(cur.agent_id),
            role_name: role_name.unwrap_or(cur.role_name),
            mode: mode.unwrap_or(cur.mode),
        };
        ConfigManager::get().update_channel(channel_id, |c| {
            c.agent_id = Arc::new(new_key.agent_id.clone());
            c.role_name = Arc::new(new_key.role_name.clone());
        }).await?;
        self.channel_manager.set_mode(channel_id, new_key.mode.clone());
        // 1. 清理无任何 channel 绑定的会话
        self.prune_sessions().await;
        // 2. 新三元组对应会话不存在则创建并构建初始上下文（agent 标识取会话 key）
        if let Some(key) = self.channel_manager.session_key(channel_id).await {
            self.ensure_session(&key).await;
        }
        Ok(())
    }

    /// channel 配置变更执行（队列内串行，不对外）：分发到 ChannelManager 方法
    async fn apply_channel_command(&self, cmd: ChannelCommand) -> Result<()> {
        match cmd {
            ChannelCommand::BindUser { channel_id, user } => self.channel_manager.bind_user(&channel_id, &user).await,
            ChannelCommand::UnbindUser { channel_id, user } => self.channel_manager.unbind_user(&channel_id, &user).await,
            ChannelCommand::BindOutgoing { channel_id, params } => self.channel_manager.bind_outgoing(&channel_id, &params).await,
            ChannelCommand::ClearOutgoing { channel_id } => self.channel_manager.clear_outgoing(&channel_id).await,
        }
    }

    /// 校验模型有效性：从 API 拉模型列表，确认 pm.model 在列表中。
    /// Err 表示校验失败（API 调用失败 / 模型不在列表），调用方决定如何处理。
    async fn verify_model(&self, pm: &ProviderModel) -> Result<()> {
        let models = self.model_client.list_models(pm).await
            .map_err(|e| Error::ModelApiError(format!("获取模型列表失败: {}", e)))?;
        if !models.iter().any(|m| m == &pm.model) {
            return Err(Error::ModelProviderNotSupported(format!(
                "模型 {} 不在 {} 的 API 模型列表", pm.model, pm.provider)));
        }
        Ok(())
    }

    /// 设置来源 channel 所属会话的模型（运行态，不回写；每次切换都从 API 拉模型列表校验）
    pub async fn set_session_model(&self, channel_id: &str, pm: ProviderModel) -> Result<()> {
        let Some(key) = self.channel_manager.session_key(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        // 每次切换都从 API 拉模型列表校验（失败拒绝，保持原模型）
        self.verify_model(&pm).await?;
        let session = self.ensure_session(&key).await;
        session.model.store(Arc::new(Some(pm)));
        Ok(())
    }

    /// 启动主循环（保持进程运行）：初始化会话 + 连接全部 channel
    pub async fn run(&self) {
        info!("Nexus 启动，等待外部输入...");
        // 按 channel 绑定三元组初始化会话集合（agent_id 取 config，保留 agent = "0"）
        for (_, ch) in ConfigManager::get().channels().await {
            if let Some(key) = self.channel_manager.session_key(ch.channel_id.as_str()).await {
                self.ensure_session(&key).await;
            }
        }
        // 连接所有 enabled 的 channel（连接/重连/回显/发送全部归 ChannelManager 通道适配层）
        self.channel_manager.clone().connect_all().await;
        // channel-client 通过 Terminal 回调驱动，此处保持进程不退出
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
}

// ==================== 消息处理 ====================

impl Nexus {
    /// 业务消息入口（由 ChannelManager 的 Terminal 转发调用；回显已在通道层 consume_pending 过滤，此处不见自身回显）
    pub(crate) async fn incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        // 1. 来源 channel 必须在配置中（会话三元组计算即校验，channel 不存在返回 None）
        let Some(key) = self.channel_manager.session_key(channel_id).await else { return; };

        // 2. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 取会话 key，事件模式编码）
        let role_name = memory_role(&key.role_name, &key.mode);
        let agent_id = Arc::new(key.agent_id.clone());
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

        // 3. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id；会话三元组透传，避免重复计算）
        self.handle_incoming(channel_id, key, event).await;
    }
}

impl Nexus {
    async fn handle_incoming(
        &self,
        channel_id: &str,
        key: SessionKey,
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
            if CommandRouter::check_admin(channel_id, &messenger_id, &user_id).await {
                self.handle_admin_command(channel_id, event, &content_text).await;
            }
            // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
            return;
        }

        // 3. 普通消息：无 out_channel 不进 Agentic Loop（ChannelRecord 已存，结束）
        let Some(_out_channel) = self.resolve_out_channel(channel_id).await else {
            return;
        };
        let session = self.ensure_session(&key).await;
        self.enqueue_batch(session, event).await;
    }

    /// 合批：数据直取会话生产侧入队（Arc<IncomingMessageEvent>）→ 更新截止时间（防抖）→ 发送触发时间（At）。
    /// 无 sleep、无逐消息任务——触发由 session 的 trigger 任务经 DelayQueue 定时处理。
    /// BatchProducer 已从 Channel 删除：enqueue 时 ensure_session 已返回会话，生产侧经 session.enqueue_batch 入队
    /// （batch_producer 已收窄为 Session 私有字段，外部不直接访问），无 Channel 中转
    async fn enqueue_batch(&self, session: Arc<Session>, event: Arc<IncomingMessageEvent>) {
        let cfg = ConfigManager::get().context_config(session.agent_id.as_str(), session.role_name.as_str()).await;
        session.enqueue_batch(event, cfg.channel_batch_interval_secs).await;
    }

    async fn handle_admin_command(
        &self,
        channel_id: &str,
        event: Arc<IncomingMessageEvent>,
        content: &str,
    ) {
        match CommandRouter::parse(content) {
            Ok(cmd) => {
                match CommandRouter::execute(&cmd, channel_id).await {
                    Ok(reply) => {
                        // 回复：系统命令始终发回来源 channel（不走 out_channel）
                        self.send_admin_reply(channel_id, event, reply).await;
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
    async fn send_admin_reply(&self, channel_id: &str, event: Arc<IncomingMessageEvent>, content: String) {
        let Some(key) = self.channel_manager.session_key(channel_id).await else {
            warn!("send_admin_reply: 未找到 channel 配置: {}", channel_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.recipient_user_id.clone(),
            group_id: event.incoming_message.group_id.clone(),
            content: Content::Text(Arc::new(content.clone())),
        };

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        match self.channel_manager.send(channel_id, msg).await {
            Ok(response) => {
                // 下行成功后：推记忆（is_self=1）
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = Arc::new(key.agent_id.clone());
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
        let ch = ConfigManager::get().channel(channel_id).await?;
        let channels = ConfigManager::get().channels().await;
        for (_, c) in &channels {
            if c.agent_id == ch.agent_id && c.role_name == ch.role_name {
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
    pub(crate) async fn resolve_out_channel_for_session(&self, session: Arc<Session>) -> Option<OutChannel> {
        let channels = ConfigManager::get().channels().await;
        for (_, c) in &channels {
            if c.agent_id.as_str() == session.agent_id.as_str()
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

    pub async fn call_provider_model(&self, pm: &ProviderModel, messages: &Vec<Message>, tools: &Vec<ToolConfig>) -> Result<ModelResponse> {
        self.model_client.call(pm, messages, tools).await
    }

    pub async fn write_memory_think(&self, request: ThinkRequest, out_channel: &OutChannel) {
        let placeholder = placeholder_request(
            request.agent_id.clone(),
            request.role_name.clone(),
            Content::Think(request.key.clone()),
            request.time.clone(),
            out_channel
        );
        self.memory_store_client.push_channel_record(placeholder).await;
        self.memory_store_client.push_think(request).await;
    }

    pub async fn write_memory_tool_call(&self, request: ToolCallRequest, out_channel: &OutChannel) {
        let placeholder = placeholder_request(
            request.agent_id.clone(),
            request.role_name.clone(),
            Content::ToolCall(request.key.clone()),
            request.time.clone(),
            out_channel
        );
        self.memory_store_client.push_channel_record(placeholder).await;
        self.memory_store_client.push_tool_call(request).await;
    }

    pub async fn write_memory_tool_result(&self, request: ToolResultRequest, out_channel: &OutChannel) {
        let placeholder = placeholder_request(
            request.agent_id.clone(),
            request.role_name.clone(),
            Content::ToolResult(request.key.clone()),
            request.time.clone(),
            out_channel
        );
        self.memory_store_client.push_channel_record(placeholder).await;
        self.memory_store_client.push_tool_result(request).await;
    }

    /// 会话可用工具：context 配置的启用 toolkits 白名单 → Station 平铺查询（本地 + 直接子递归）
    /// tools 聚合为空则请求不携带 tools 字段（兼容无工具场景）
    pub async fn tools_for_session(&self, session: Arc<Session>) -> Vec<ToolConfig> {
        let cfg = ConfigManager::get().context_config(session.agent_id.as_str(), session.role_name.as_str()).await;
        if cfg.toolkits.is_empty() {
            return Vec::new();
        }
        match Station::get().tools(Some(&cfg.toolkits)).await {
            Ok(tools) => tools,
            Err(e) => {
                warn!("工具查询失败: {}", e);
                Vec::new()
            }
        }
    }

    /// 执行单个 tool call：全局 Station 本地实现表 → 直接子递归；找不到/调用失败返回错误 JSON
    pub async fn execute_tool_call(&self, call: Arc<ToolCall>) -> serde_json::Value {
        match Station::get().call_tool(call.name.as_str(), (*call.arguments).clone()).await {
            Ok(v) => v,
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        }
    }

    /// Agentic Loop 产出回复：发到 out_channel（channel_id + ChannelUser + group_id）
    pub async fn send_outgoing(&self, out_channel: &OutChannel, content: Arc<String>) {
        let Some(key) = self.channel_manager.session_key(out_channel.channel_id.as_str()).await else {
            warn!("send_outgoing: 未找到 channel 配置: {}", out_channel.channel_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
            user_id: Arc::new(out_channel.user.user_id.clone()),
            group_id: out_channel.group_id.clone(),
            content: Content::Text(content),
        };

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        match self.channel_manager.send(out_channel.channel_id.as_str(), msg).await {
            Ok(response) => {
                // 下行成功后：推记忆（is_self=1）
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = Arc::new(key.agent_id.clone());
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

/// 从 Content 枚举中提取文本（session_manager.try_flush 复用，pub(crate)）
pub(crate) fn extract_text(content: &Content) -> String {
    match content {
        Content::Text(t) => t.as_str().to_string(),
        Content::Multi(items) => items.iter()
            .filter_map(|c| match c { Content::Text(t) => Some(t.as_str().to_string()), _ => None })
            .collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn placeholder_request(
    agent_id: Arc<String>,
    role_name: Arc<String>,
    content: Content,
    time: Arc<String>,
    out_channel: &OutChannel,
) -> ChannelRequest {
    let empty = Arc::new(String::new());
    let messenger_id = Arc::new(out_channel.user.messenger_id.clone());
    let user_id = Arc::new(out_channel.user.user_id.clone());
    ChannelRequest {
        agent_id,
        role_name,
        messenger_id,
        user_id: user_id.clone(),
        self_user_id: user_id.clone(),
        group_id: out_channel.group_id.clone(),
        is_self: 1,
        messenger_name: empty.clone(),
        user_name: empty.clone(),
        group_name: empty.clone(),
        content,
        time,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== verify_agent_exists：保留 id / 空串直接通过 =====
    // 成员函数化后需构造实例取 &self；MemoryEgoClient/MemoryStoreClient 构造读 ApiConfig/SecurityConfig
    // 进程级单例（kissbot_config::Config::get 读 KISSBOT_CONFIG env），按 http_server 测试先例写临时配置
    async fn test_nexus(dir: &tempfile::TempDir) -> Nexus {
        let data_dir = dir.path().join("data");
        let cfg_path = dir.path().join("config.json");
        let cfg_json = format!(
            r#"{{"api":{{"memory_store_url":"","memory_ego_url":""}},"security":{{"api_key":"user-key-456","admin_api_key":"admin-key-123"}},"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9090,"ws_reconnect_interval_secs":5}}}}"#,
            data_dir.to_str().unwrap()
        );
        std::fs::write(&cfg_path, cfg_json).unwrap();
        // 2024 edition：设置环境变量需要 unsafe
        unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };
        let (command_tx, _command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (channel_task_tx, _channel_task_rx) = tokio::sync::mpsc::unbounded_channel();
        Nexus {
            memory_store_client: Arc::new(MemoryStoreClient::new()),
            memory_ego_client: Arc::new(MemoryEgoClient::new()),
            session_manager: SessionManager::new(data_dir.to_str().unwrap()),
            model_client: Arc::new(ModelClient::new()),
            valid_default: ArcSwap::from_pointee(None),
            channel_manager: Arc::new(ChannelManager::new()),
            command_tx,
            channel_task_tx,
        }
    }

    #[tokio::test]
    async fn verify_agent_exists_reserved_or_empty_passes() {
        let dir = tempfile::tempdir().unwrap();
        let nexus = test_nexus(&dir).await;
        // 保留 id "0" 与空串直接 Ok，提前返回不触 ego HTTP
        assert!(nexus.verify_agent_exists("0").await.is_ok());
        assert!(nexus.verify_agent_exists("").await.is_ok());
    }

    #[tokio::test]
    async fn verify_role_exists_empty_passes() {
        let dir = tempfile::tempdir().unwrap();
        let nexus = test_nexus(&dir).await;
        // 显式空串（保留 role）直接 Ok，提前返回不触 ego HTTP；
        // 非空分支依赖 ego 服务（memory_ego_url 为空返回 Err），暂不测
        assert!(nexus.verify_role_exists("a1", "").await.is_ok());
    }
}
