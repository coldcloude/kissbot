use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use arc_swap::ArcSwap;
use bytes::Bytes;
use chrono::Local;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::types::{
    Mode, WriteTask, ContextMessage, Result, Error, SessionKey, memory_role,
};
use crate::config_manager::{ConfigManager, ProviderModel};
use crate::command_router::CommandRouter;
use crate::model_client::ModelClient;
use crate::session_manager::{Session, SessionManager};
use crate::memory_reader::MemoryReader;
use crate::memory_writer::MemoryWriter;
use crate::memory_store_client::{MemoryStoreClient, ChannelRecord};

use kissbot_api::channel::{IncomingMessage, OutgoingMessage, BindRequest};
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

/// 每 channel 运行时上下文：维护「已发出但尚未收到回显」的 msg_id 集合 + 运行态 agent_id
/// 运行态 agent_id（UUID）在启动绑定/切换 agent 时确定并自主保存；
/// 解析失败回退保留 agent_id（"0"，等同 agent_name="" 的保留语义）
struct ChannelContext {
    pending_outgoing: HashMap<String, Instant>,
    /// 运行态 agent_id（启动/切换时确定；未绑定为 None，channel_agent 懒绑定）
    agent_id: Option<Arc<String>>,
}

impl ChannelContext {
    fn new() -> Self {
        Self { pending_outgoing: HashMap::new(), agent_id: None }
    }

    fn add_pending(&mut self, msg_id: String) {
        self.evict(CHANNEL_CONTEXT_TTL);
        self.pending_outgoing.insert(msg_id, Instant::now());
    }

    /// 命中则移除并返回 true（回显消费）
    fn consume_pending(&mut self, msg_id: &str) -> bool {
        self.evict(CHANNEL_CONTEXT_TTL);
        self.pending_outgoing.remove(msg_id).is_some()
    }

    /// TTL 懒清理：淘汰超过 ttl 的条目（调用点传固定 TTL；测试可传 0 即插入即过期）
    fn evict(&mut self, ttl: Duration) {
        self.pending_outgoing.retain(|_, t| t.elapsed() < ttl);
    }
}

pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    memory_reader: Arc<MemoryReader>,
    memory_writer: Arc<MemoryWriter>,
    memory_store_client: Arc<MemoryStoreClient>,
    session_manager: Arc<SessionManager>,
    model_client: Arc<tokio::sync::Mutex<ModelClient>>,
    /// 启动校验后的 default_model（从 API 模型列表校验）；None = 无模型（普通消息静默忽略）
    valid_default: ArcSwap<Option<ProviderModel>>,
    /// 按 agent 内部 channel_id 索引的 ChannelClient
    channel_clients: Arc<DashMap<String, Arc<ChannelClient>>>,
    /// 每 channel 运行时上下文（msg_id 回显判定 + 运行态 agent 绑定）
    channel_contexts: Arc<DashMap<String, Arc<tokio::sync::Mutex<ChannelContext>>>>,
    /// 断线通知：channel_id → Notify，closed() 通知重连循环
    disconnect_notify: Arc<DashMap<String, Arc<tokio::sync::Notify>>>,
}

impl AgentCoordinator {
    pub async fn new(
        config: Arc<ConfigManager>,
        memory_writer: MemoryWriter,
    ) -> Result<Arc<Self>> {
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_writer = Arc::new(memory_writer);
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let session_manager = SessionManager::new();
        let model_client = ModelClient::new(config.clone());

        let coordinator = Arc::new(Self {
            config: config.clone(),
            memory_reader,
            memory_writer,
            memory_store_client,
            session_manager,
            model_client: Arc::new(tokio::sync::Mutex::new(model_client)),
            channel_clients: Arc::new(DashMap::new()),
            channel_contexts: Arc::new(DashMap::new()),
            disconnect_notify: Arc::new(DashMap::new()),
            valid_default: ArcSwap::from_pointee(None),
        });

        // 启动校验 default_model：从 API 拉模型列表，不在列表则无模型（告警）
        let default_model = config.default_model().await;
        let valid_default = match coordinator.model_client.lock().await.list_models(&default_model).await {
            Ok(list) if list.iter().any(|m| m == &default_model.model) => Some(default_model.clone()),
            Ok(_) => { tracing::warn!("default_model {}/{} 不在 API 模型列表", default_model.provider, default_model.model); None }
            Err(e) => { tracing::warn!("校验 default_model 失败（API 不可用?）: {:?}", e); None }
        };
        coordinator.valid_default.store(Arc::new(valid_default));

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
        // 运行态 mode 参与会话定位；无脱离态，agent_name 为空 = 保留 agent
        let mode = self.session_manager.channel_mode(&ch.channel_id);
        session_key_of(&ch.agent_name, &ch.role_name, mode)
    }

    /// 记录已发出的 outgoing msg_id 到该 channel 的 pending 集合（回显判定用）
    async fn record_outgoing_msg_id(&self, channel_id: &str, msg_id: &Arc<String>) {
        let ctx = self.channel_contexts
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(ChannelContext::new())))
            .clone();
        ctx.lock().await.add_pending(msg_id.as_str().to_string());
    }

    /// 按 msg_id 判定是否为自身发出的回显；命中则消费（移除）并返回 true
    async fn is_self_echo_by_msg_id(&self, channel_id: &str, msg_id: &Arc<String>) -> bool {
        if let Some(ctx) = self.channel_contexts.get(channel_id) {
            ctx.lock().await.consume_pending(msg_id.as_str())
        } else {
            false
        }
    }

    /// 读取 channel 运行态 agent_id；未绑定（异常路径）时懒绑定
    async fn channel_agent(&self, channel_id: &str) -> Arc<String> {
        let ctx = self.channel_contexts.get(channel_id).map(|c| c.clone());
        if let Some(ctx) = ctx {
            let guard = ctx.lock().await;
            if let Some(agent_id) = guard.agent_id.clone() {
                return agent_id;
            }
        }
        // 未绑定/缺失：懒绑定（正常启动路径已在 new() 中绑定全部 channel）
        self.bind_channel_runtime(channel_id).await
    }

    /// 绑定（或重绑）channel 运行态 agent_id：从配置 agent_name 解析，写入 channel 运行状态；
    /// 空 agent_name 直接保留；解析失败回退保留 agent_id（"0"）并告警
    async fn bind_channel_runtime(&self, channel_id: &str) -> Arc<String> {
        let agent_name = self.channel_config(channel_id).await
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
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(ChannelContext::new())))
            .clone();
        ctx.lock().await.agent_id = Some(agent_id.clone());
        agent_id
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
        } else if let Ok(ego_info) = self.load_ego_info(session.agent_id.as_str(), &session.key.role_name).await {
            session.context.lock().await.set_system_message(ego_info);
        }
        // 历史记忆照常加载（保留 agent 也调 memory-store；URL 空则优雅跳过）
        if let Ok(history) = self.memory_reader
            .read_history(&self.config, session.agent_id.as_str(), &session.key.role_name, &session.key.mode)
            .await
        {
            session.context.lock().await.load_history(history);
        }
        // 顶层记忆索引（memory-struct 未实现时静默跳过）——保持不变
        let _ = self.memory_reader
            .read_memory_struct_index(&self.config, session.agent_id.as_str(), &session.key.role_name, &session.key.mode)
            .await;
    }

    /// 来源 channel 绑定信息变化后重定位会话：清理无绑定会话 + 为新三元组创建会话
    async fn relocate_channel(&self, channel_id: &str) {
        // 1. 清理无任何 channel 绑定的会话
        self.prune_sessions().await;
        // 2. 新三元组对应会话不存在则创建并构建初始上下文（agent 标识取该 channel 运行态绑定）
        if let Some(ch) = self.channel_config(channel_id).await {
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
        if let Some(ch) = self.channel_config(channel_id).await {
            let key = self.session_key_for(&ch);
            if let Some(session) = self.session_manager.get(&key) {
                self.reset_context(&session).await;
                return;
            }
        }
        warn!("reset: channel {} 无会话可重置", channel_id);
    }

    /// 上下文重置：清空后重建初始上下文（取记忆/ego 用会话保存的 agent_id）
    async fn reset_context(&self, session: &Arc<Session>) {
        session.context.lock().await.clear();
        self.build_initial_context(session).await;
        info!("会话上下文已重置: {:?}", session.key);
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
            let bu = &ch.bind_user;
            ids.insert(kissbot_api::ChannelUser {
                messenger_id: bu.messenger_id.to_string(),
                user_id: bu.user_id.to_string(),
            });
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

    /// 切换来源 channel 的运行态模式（不回写，会话重定位由调用方触发）
    pub async fn set_channel_mode(&self, channel_id: &str, mode: Mode) {
        self.session_manager.set_channel_mode(channel_id, mode);
    }

    /// 设置/取消来源 channel 为其会话的发送 channel（回写配置）
    /// on 时清除同会话其他 channel 的 is_send_channel 标志
    pub async fn set_send_channel(&self, channel_id: &str, on: bool) -> Result<()> {
        let Some(ch) = self.channel_config(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let key = self.session_key_for(&ch);
        if on {
            // 同会话其他 channel 的发送标志清除（保持会话内只有一个发送 channel）
            let channels = self.config.channels().await;
            for (cid, other) in channels {
                if cid == channel_id {
                    continue;
                }
                let same_key = other.agent_name.as_str() == key.agent_name
                    && other.role_name.as_str() == key.role_name
                    && self.session_manager.channel_mode(&cid) == key.mode;
                if same_key && other.is_send_channel {
                    self.config.update_channel(&cid, |c| c.is_send_channel = false).await?;
                }
            }
        }
        self.config.update_channel(channel_id, |c| c.is_send_channel = on).await
    }

    /// 设置来源 channel 所属会话的模型（运行态，不回写；每次切换都从 API 拉模型列表校验）
    pub async fn set_session_model(&self, channel_id: &str, pm: ProviderModel) -> Result<()> {
        let Some(ch) = self.channel_config(channel_id).await else {
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
        let Some(ch) = self.channel_config(channel_id).await else {
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

    /// 从配置中按 channel_id 取 channel 配置
    async fn channel_config(&self, channel_id: &str) -> Option<Arc<crate::config_manager::ChannelConfig>> {
        self.config.channels().await.into_iter()
            .find(|(id, _)| id == channel_id)
            .map(|(_, ch)| ch)
    }

    /// 连接所有 enabled 的 channel（NexusRepo channel 配置为连接来源）
    /// 连接与绑定统一由 ChannelConfig 描述：enabled 控制连接，bind_user 为绑定身份
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
                            // 绑定用户实时读取（BindRequest.messenger_id 用绑定身份的 messenger 标识，如 "web"）
                            let bind_user = coordinator_clone.channel_config(&channel_id).await
                                .map(|c| c.bind_user.clone());
                            if let Some(bu) = bind_user {
                                let _ = client_clone.bind(BindRequest {
                                    messenger_id: Arc::new(bu.messenger_id.clone()),
                                    user_id: Arc::new(bu.user_id.clone()),
                                }).await;
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
    /// 收到上行消息
    async fn incoming_message(&self, channel_id: &str, message: Arc<IncomingMessage>) {
        // 1. 来源 channel 必须在配置中
        let Some(ch) = self.channel_config(channel_id).await else { return; };

        // 2. msg_id 回显判定：命中（已发未回显）则跳过，不存 record、不进 agentic loop
        if self.is_self_echo_by_msg_id(channel_id, &message.msg_id).await {
            return;
        }

        // 3. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 取来源 channel 运行态绑定，事件模式编码）
        let key = self.session_key_for(&ch);
        let role_name = memory_role(&key);
        let agent_id = self.channel_agent(channel_id).await;
        self.memory_store_client.push_channel_record(ChannelRecord {
            agent_id,
            role_name: Arc::new(role_name),
            messenger_id: message.messenger_id.clone(),
            user_id: message.user_id.clone(),
            group_id: message.group_id.clone(),
            is_self: 0,
            messenger_name: message.messenger_name.clone(),
            user_name: message.user_name.clone(),
            group_name: message.group_name.clone(),
            content: message.content.clone(),
            time: message.time.clone(),
        }).await;

        // 4. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
        self.handle_incoming(channel_id, ch, message).await;
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
        incoming: Arc<IncomingMessage>,
    ) {
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let content_text = extract_text(&incoming.content);

        // 1. 系统事件（群组变更/用户移除）不进 agentic loop
        match &incoming.content {
            Content::GroupJoin(_) | Content::GroupLeave(_) | Content::UserRemove(_) => return,
            _ => {}
        }

        // 2. 管理命令
        if CommandRouter::is_command(&content_text) {
            if CommandRouter::check_admin(&self.config, channel_id, &messenger_id, &user_id).await {
                self.handle_admin_command(channel_id, &content_text, &group_id).await;
            }
            // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
            return;
        }

        // 3. 普通消息进入该会话的 agentic loop
        let key = self.session_key_for(&ch);
        let (session, _) = self.ensure_session(&key, channel_id).await;
        self.run_agentic_loop(channel_id, &session, incoming).await;
    }

    async fn handle_admin_command(
        &self,
        channel_id: &str,
        content: &str,
        group_id: &str,
    ) {
        match CommandRouter::parse(content) {
            Ok(cmd) => {
                match CommandRouter::execute(&cmd, &self.config, self, channel_id).await {
                    Ok((reply, effect)) => {
                        // 回复：会话存在走发送 channel，脱离态/无会话回退来源 channel
                        self.reply(channel_id, group_id, reply).await;

                        // 应用命令执行效果
                        match effect {
                            crate::types::CommandEffect::Relocate => {
                                self.relocate_channel(channel_id).await;
                            }
                            crate::types::CommandEffect::ResetSession => {
                                self.reset_session_for(channel_id).await;
                            }
                            crate::types::CommandEffect::None => {}
                        }
                    }
                    Err(e) => {
                        self.reply(channel_id, group_id,
                            format!("❌ 命令执行失败: {}", e)).await;
                    }
                }
            }
            Err(e) => {
                self.reply(channel_id, group_id,
                    format!("⚠️ {}", e)).await;
            }
        }
    }

    /// 回复消息：解析会话发送 channel，脱离态/无会话回退来源 channel
    async fn reply(&self, channel_id: &str, group_id: &str, content: String) {
        let send_channel = self.resolve_send_channel(channel_id).await
            .unwrap_or_else(|| channel_id.to_string());
        self.send_reply(&send_channel, group_id, content).await;
    }

    /// 解析来源 channel 所属会话的发送 channel
    async fn resolve_send_channel(&self, channel_id: &str) -> Option<String> {
        let ch = self.channel_config(channel_id).await?;
        let key = self.session_key_for(&ch);
        self.session_manager
            .resolve_send_channel(&key, self.config.channels().await)
            .or(Some(channel_id.to_string()))
    }

    async fn run_agentic_loop(&self, channel_id: &str, session: &Arc<Session>, incoming: Arc<IncomingMessage>) {
        // 无可用模型：静默忽略普通消息（仅管理指令可用）
        if session.model.load().is_none() {
            return;
        }
        let content_text = extract_text(&incoming.content);
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let time = incoming.time.to_string();

        // 1. 追加用户消息到该会话上下文
        {
            let mut ctx = session.context.lock().await;
            ctx.push_user_message(ContextMessage::User {
                messenger_id: messenger_id.clone(),
                user_id: user_id.clone(),
                group_id: group_id.clone(),
                content: content_text.clone(),
                time: time.clone(),
            });
        }

        // 2. 调用模型（用该会话的模型）
        let response = {
            let ctx = session.context.lock().await;
            let messages = ctx.build();
            let model = session.model.load_full();
            let Some(pm) = model.as_ref() else { return; };
            let mc = self.model_client.lock().await;
            mc.call(pm, &messages).await
        };

        match response {
            Ok(model_resp) => {
                let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

                // 3. 记录已发送内容
                {
                    let mut ctx = session.context.lock().await;
                    ctx.push_assistant(model_resp.content.clone(), now.clone());
                }

                // 4. 推送 think 到 MemoryWriter（事件模式编码；取记忆用会话保存的 agent_id）
                let role_name = memory_role(&session.key);
                let _ = self.memory_writer.push(WriteTask::Think {
                    agent_id: session.agent_id.to_string(),
                    role_name: Some(role_name),
                    content: model_resp.content.clone(),
                    time: now,
                });

                // 5. 发送回复到该会话的发送 channel
                self.reply(channel_id, &group_id, model_resp.content).await;

                // 6. 检查上下文超长
                let overflow = {
                    let ctx = session.context.lock().await;
                    ctx.is_overflow()
                };
                if overflow {
                    warn!("会话上下文超长，触发重置: {:?}", session.key);
                    self.reset_context(session).await;
                }
            }
            Err(e) => {
                warn!("模型调用失败: {:?}", e);
                self.reply(channel_id, &group_id,
                    format!("❌ 模型调用失败: {}", e)).await;
            }
        }
    }

    /// 发送回复消息到通道（send_channel_id 为该会话发送 channel），成功后推记忆（is_self=1）
    /// 发件人身份为发送 channel 配置的 bind_user
    async fn send_reply(&self, send_channel_id: &str, group_id: &str, content: String) {
        let Some(client) = self.channel_clients.get(send_channel_id) else {
            warn!("send_reply: 未找到 channel client: {}", send_channel_id);
            return;
        };
        let Some(ch) = self.channel_config(send_channel_id).await else {
            warn!("send_reply: 未找到 channel 配置: {}", send_channel_id);
            return;
        };
        let bound = ch.bind_user.clone();

        let msg = OutgoingMessage {
            messenger_id: Arc::new(bound.messenger_id.clone()),   // 对端 messenger 标识（如 "web"）
            user_id: Arc::new(bound.user_id.clone()),             // agent 绑定的用户
            group_id: Arc::new(group_id.to_string()),
            content: Content::Text(Arc::new(content.clone())),
        };

        match client.send_message(msg).await {
            Ok(response) => {
                // 下行成功后：先记 msg_id 到 pending（回显判定），再推记忆（is_self=1，name 取自 response）
                // agent_id 取发送 channel 的运行态绑定（channel 存记忆用 channel 保存的 agent_id）
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key);
                let agent_id = self.channel_agent(send_channel_id).await;
                self.record_outgoing_msg_id(send_channel_id, &response.msg_id).await;
                self.memory_store_client.push_channel_record(ChannelRecord {
                    agent_id,
                    role_name: Arc::new(role_name),
                    messenger_id: Arc::new(bound.messenger_id.clone()),
                    user_id: Arc::new(bound.user_id.clone()),
                    group_id: Arc::new(group_id.to_string()),
                    is_self: 1,
                    messenger_name: response.messenger_name.clone(),
                    user_name: response.user_name.clone(),
                    group_name: response.group_name.clone(),
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;
            }
            Err(e) => {
                warn!("send_reply 失败: {:?}", e);
            }
        }
    }
}

/// 从 Content 枚举中提取文本
fn extract_text(content: &Content) -> String {
    match content {
        Content::Text(t) => t.as_str().to_string(),
        Content::Multi(items) => items.iter()
            .filter_map(|c| match c { Content::Text(t) => Some(t.as_str().to_string()), _ => None })
            .collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
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
        let mut ctx = ChannelContext::new();
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
        let mut ctx = ChannelContext::new();
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
}
