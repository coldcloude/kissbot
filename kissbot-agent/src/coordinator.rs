use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use arc_swap::ArcSwap;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::channel_manager::ChannelManager;
use crate::types::{
    Error, Message, Mode, ModelResponse, RESERVED_AGENT_ID, Result, SessionKey, ToolCall, memory_role,
};
use crate::session_manager::{Session, SessionManager};
use crate::config_manager::{ConfigManager, ProviderModel, OutChannel, ToolConfig};
use crate::command_router::CommandRouter;
use crate::model_client::ModelClient;
use crate::message::pack_memory_messages;
use crate::memory_store_client::MemoryStoreClient;
use crate::station::{self, StationRuntime};

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
    /// 应用新会话三元组（agent/role/mode 任一变化）
    ApplyKey { channel_id: String, new_key: SessionKey, done: tokio::sync::oneshot::Sender<Result<()>> },
}

/// AgentCoordinator 全局单例（进程内唯一；new() 完成时注册，此后 get() 可用）。
/// 所有使用 coordinator 的位置一律不传参数、从单例获取（Session/Channel 不保存引用）。
static SINGLETON: OnceLock<AgentCoordinator> = OnceLock::new();

pub struct AgentCoordinator {
    memory_store_client: Arc<MemoryStoreClient>,
    session_manager: Arc<SessionManager>,
    model_client: Arc<ModelClient>,
    /// 启动校验后的 default_model（从 API 模型列表校验）；None = 无模型（普通消息静默忽略）
    valid_default: ArcSwap<Option<ProviderModel>>,
    /// 每 channel 运行时管理（ChannelManager：内部 DashMap 无锁并发，含 pending/mode/client）
    channel_manager: Arc<ChannelManager>,
    /// agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
    command_tx: tokio::sync::mpsc::UnboundedSender<ConfigChange>,
    /// station_id → StationRuntime（启动时按配置构建；base_url 为空的本地 station 注册内置 Read 工具）
    station_runtimes: Arc<DashMap<String, Arc<StationRuntime>>>,
}

impl AgentCoordinator {
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn get() -> &'static AgentCoordinator {
        SINGLETON.get().expect("AgentCoordinator 未初始化")
    }

    pub async fn new() -> Result<()> {
        let config = ConfigManager::get();
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let data_dir = config.data_dir().to_string();
        let session_manager = SessionManager::new(&data_dir);
        let model_client = ModelClient::new();
        // agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<ConfigChange>();

        let coordinator = Self {
            memory_store_client,
            session_manager,
            model_client: Arc::new(model_client),
            channel_manager: Arc::new(ChannelManager::new()),
            valid_default: ArcSwap::from_pointee(None),
            command_tx,
            station_runtimes: Arc::new(DashMap::new()),
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

        // 启动动作（绑定运行态 agent / 初始化会话 / 连接 channel）统一在 run() 中执行

        // 注册全局单例（此后 get() 可用；run() 中启动动作与连接回调均晚于此）
        let _ = SINGLETON.set(coordinator);

        // 启动变更消费者：agent/role/event 变更串行处理（避免写-写竞态；读不受影响）
        // spawn 晚于 SINGLETON.set，任务内 get() 必然就绪
        tokio::spawn(async move {
            while let Some(change) = command_rx.recv().await {
                match change {
                    ConfigChange::ApplyKey { channel_id, new_key, done } => {
                        let coordinator = AgentCoordinator::get();
                        let rst = coordinator.apply_channel_key(&channel_id, &new_key).await;
                        let _ = done.send(rst);
                    }
                }
            }
        });

        info!("AgentCoordinator 初始化完成");
        Ok(())
    }

    // ==================== 会话定位与构建 ====================

    /// 按来源 channel 的绑定配置 + 运行态 mode 计算会话 key（agent/role 取绑定配置）
    fn session_key_for(&self, ch: &crate::config_manager::ChannelConfig) -> SessionKey {
        SessionKey {
            agent_id: ch.agent_id.to_string(),
            role_name: ch.role_name.to_string(),
            // 运行态 mode（未绑定/缺失回退角色模式）
            mode: self.channel_manager.mode(&ch.channel_id),
        }
    }

    /// 校验 agent_id 存在（/agent 切换前调用）：空或保留 id "0" 直接通过；
    /// ego 未配置/HTTP 失败/data 为 null 返回 Err（调用方保持原 agent 不变）
    pub async fn verify_agent_exists(agent_id: &str) -> Result<()> {
        if agent_id.is_empty() || agent_id == RESERVED_AGENT_ID {
            return Ok(());
        }
        let ego_url = kissbot_api::ApiConfig::get().memory_ego_url.clone();
        if ego_url.is_empty() {
            return Err(Error::MemoryEgoError("ego 未配置（memory_ego_url 为空）".to_string()));
        }
        let client = reqwest::Client::new();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        let resp = client.post(format!("{}/agent/get", ego_url))
            .header(kissbot_security::HEADER_API_KEY, api_key.as_str())
            .json(&serde_json::json!({ "agent_id": agent_id }))
            .send()
            .await
            .map_err(|e| Error::MemoryEgoError(format!("agent/get 请求失败: {}", e)))?;
        let data: serde_json::Value = resp.json().await
            .map_err(|e| Error::MemoryEgoError(format!("agent/get 响应解析失败: {}", e)))?;
        if data["data"].is_null() {
            Err(Error::MemoryEgoError(format!("agent 不存在: {}", agent_id)))
        } else {
            Ok(())
        }
    }

    /// 定位会话，新建时构建初始上下文；返回 (会话, 是否新建)
    /// channel_id 为触发会话创建/重置的来源 channel；新建会话的 agent_id 取自 key（config 绑定）
    async fn ensure_session(&self, key: &SessionKey, _channel_id: &str) -> (Arc<Session>, bool) {
        // valid_default.load_full() 返回 Arc<Option<ProviderModel>>，解引用克隆得 Option
        let model = (*self.valid_default.load_full()).clone();
        let (session, created) = self.session_manager.get_or_create(key, model);
        if created {
            // 新建会话上下文：event 从缓存恢复（全量回读；文件不存在为空，不清理）；role 查询记忆重建（归档+清空在 build_role_context 内部）
            match session.mode.as_ref() {
                Mode::Event(_) => {
                    let _ = session.context.lock().await.recover_from_cache().await;
                }
                Mode::Role => {
                    let messages = self.build_context_from_memory_store(session.agent_id.clone(), session.role_name.clone()).await;
                    let _ = session.context.lock().await.archive_and_clear_cache_and_reset_messages(Some(messages)).await;
                }
            }
            // 系统消息：保留 agent（agent_id="0"）用 NexusRepo 默认系统提示词；其余走 load_ego_info。
            // 生成结果执行一次 set（待定，下次发送前对比应用；与缓存恢复的系统不一致时旧上下文先归档）
            if session.agent_id.as_str() == RESERVED_AGENT_ID {
                let prompt = ConfigManager::get().default_system_prompt().await;
                session.context.lock().await.set_system_message(prompt);
            } else if let Ok(ego_info) = self.load_ego_info(session.agent_id.as_str(), &session.role_name).await {
                session.context.lock().await.set_system_message(ego_info);
            }
        }
        (session, created)
    }

    /// role 模式上下文构建（新建/溢出重置共用）：查询记忆打包 → 归档旧上下文+清空缓存（内部幂等）→ 重建
    /// 取记忆用会话状态保存的 agent_id（来自会话 key）
    pub async fn build_context_from_memory_store(&self, agent_id: Arc<String>, role_name: Arc<String>) -> Vec<Message> {
        let cfg = ConfigManager::get().context_config(agent_id.as_str(), role_name.as_str()).await;
        self.memory_store_client
            .read_recent_for_context(agent_id, role_name, &cfg).await
            .map_or_else(|_| vec![], |msgs| pack_memory_messages(&msgs))
    }

    /// 按当前全部 channel 的绑定集合清理无绑定会话
    async fn prune_sessions(&self) {
        let channels = ConfigManager::get().channels().await;
        let mut keys = HashSet::new();
        for (_, ch) in &channels {
            keys.insert(self.session_key_for(ch));
        }
        self.session_manager.retain(&keys);
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
        for (_, ch) in ConfigManager::get().channels().await {
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

    /// agent/role/mode 变更统一入口：应用新会话三元组（写 config agent_id/role_name + 运行态 mode + 会话重定位），
    /// 走串行队列，返回时已生效
    pub async fn change_channel_key(&self, channel_id: &str, new_key: SessionKey) -> Result<()> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        self.command_tx.send(ConfigChange::ApplyKey {
            channel_id: channel_id.to_string(),
            new_key,
            done: done_tx,
        }).map_err(|_| Error::InternalError("变更队列已关闭".to_string()))?;
        done_rx.await.map_err(|_| Error::InternalError("变更处理中断".to_string()))?
    }

    /// 取 channel 当前会话三元组（config 的 agent_id/role_name + 运行态 mode），命令构造新三元组用
    pub async fn channel_session_key(&self, channel_id: &str) -> Option<SessionKey> {
        let ch = ConfigManager::get().channel(channel_id).await?;
        Some(self.session_key_for(&ch))
    }

    // ---- 变更消费者（队列内串行执行，不对外） ----

    /// 来源 channel 绑定信息变化后重定位会话：清理无绑定会话 + 为新三元组创建会话（apply_channel_key 专用）
    /// 运行态 mode 写 Channel.mode（/mode 切换不回写，重启回 Role）
    async fn apply_channel_key(&self, channel_id: &str, new_key: &SessionKey) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            c.agent_id = Arc::new(new_key.agent_id.clone());
            c.role_name = Arc::new(new_key.role_name.clone());
        }).await?;
        self.channel_manager.set_mode(channel_id, new_key.mode.clone());
        // 1. 清理无任何 channel 绑定的会话
        self.prune_sessions().await;
        // 2. 新三元组对应会话不存在则创建并构建初始上下文（agent 标识取会话 key）
        if let Some(ch) = ConfigManager::get().channel(channel_id).await {
            let key = self.session_key_for(&ch);
            self.ensure_session(&key, channel_id).await;
        }
        Ok(())
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
        let Some(ch) = ConfigManager::get().channel(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let key = self.session_key_for(&ch);
        // 每次切换都从 API 拉模型列表校验（失败拒绝，保持原模型）
        self.verify_model(&pm).await?;
        let (session, _) = self.ensure_session(&key, channel_id).await;
        session.model.store(Arc::new(Some(pm)));
        Ok(())
    }

    /// 启动主循环（保持进程运行）：初始化会话 + 连接全部 channel
    pub async fn run(&self) {
        info!("AgentCoordinator 启动，等待外部输入...");
        // 按 channel 绑定三元组初始化会话集合（agent_id 取 config，保留 agent = "0"）
        for (_, ch) in ConfigManager::get().channels().await {
            let key = self.session_key_for(&ch);
            self.ensure_session(&key, &ch.channel_id).await;
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

impl AgentCoordinator {
    /// 业务消息入口（由 ChannelManager 的 Terminal 转发调用；回显已在通道层 consume_pending 过滤，此处不见自身回显）
    pub(crate) async fn incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        // 1. 来源 channel 必须在配置中
        let Some(ch) = ConfigManager::get().channel(channel_id).await else { return; };

        // 2. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 取会话 key，事件模式编码）
        let key = self.session_key_for(&ch);
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

        // 3. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
        self.handle_incoming(channel_id, ch, event).await;
    }
}

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
            if CommandRouter::check_admin(ConfigManager::get(), channel_id, &messenger_id, &user_id).await {
                self.handle_admin_command(channel_id, event, &content_text).await;
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
        self.enqueue_batch(session, event).await;
    }

    /// 合批：数据直取会话生产侧入队（Arc<IncomingMessageEvent>）→ 更新截止时间（防抖）→ 发送触发时间（At）。
    /// 无 sleep、无逐消息任务——触发由 session 的 trigger 任务经 DelayQueue 定时处理。
    /// BatchProducer 已从 Channel 删除：enqueue 时 ensure_session 已返回会话，生产侧直接取 session.batch_producer，无 Channel 中转
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
                match CommandRouter::execute(&cmd, ConfigManager::get(), channel_id).await {
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
        let Some(ch) = ConfigManager::get().channel(channel_id).await else {
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
                let key = self.session_key_for(&ch);
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

    /// 会话可用工具：context 配置的启用的 stations ∩ 实际配置的 station → 收集 ToolConfig
    /// tools 聚合为空则请求不携带 tools 字段（兼容无工具场景）
    pub async fn tools_for_session(&self, session: Arc<Session>) -> Vec<ToolConfig> {
        let cfg = ConfigManager::get().context_config(session.agent_id.as_str(), session.role_name.as_str()).await;
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
    pub async fn execute_tool_call(&self, session: Arc<Session>, call: Arc<ToolCall>) -> serde_json::Value {
        let cfg = ConfigManager::get().context_config(session.agent_id.as_str(), session.role_name.as_str()).await;
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
    pub async fn send_outgoing(&self, out_channel: &OutChannel, content: Arc<String>) {
        let Some(ch) = ConfigManager::get().channel(out_channel.channel_id.as_str()).await else {
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
                let key = self.session_key_for(&ch);
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
    // 注：其余分支（ego HTTP 校验）依赖 ApiConfig/SecurityConfig 进程级单例，单测无法控制 url/时序，暂不测

    #[tokio::test]
    async fn verify_agent_exists_reserved_or_empty_passes() {
        // 保留 id "0" 与空串直接 Ok，提前返回不触全局配置单例
        assert!(AgentCoordinator::verify_agent_exists("0").await.is_ok());
        assert!(AgentCoordinator::verify_agent_exists("").await.is_ok());
    }
}
