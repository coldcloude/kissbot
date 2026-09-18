use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::Local;
use kissbot_api::RESERVED_AGENT_ID;
use tracing::{info, warn};

use crate::channel_manager::ChannelManager;
use crate::configs::{EffectiveLLMConfig, EffectiveMemoryRecoverConfig, LLMConfig, MemoryRecoverConfig, OutChannel, OutChannelConfig, ToolConfig, ToolkitSetConfig};
use crate::pipelines::PipelineManager;
use crate::provider::ProviderManager;
use crate::types::{
    ChannelCommand, Error, Message, Mode, ModelResponse, Result, SessionKey, ToolCall, role_mode,
};
use crate::session_manager::{Session, SessionManager};
use crate::station::Station;
use crate::config_manager::ConfigManager;
use crate::command_router::CommandRouter;
use crate::message::pack_memory_messages;
use crate::memory_ego_client::MemoryEgoClient;
use crate::memory_store_client::MemoryStoreClient;

use kissbot_api::channel::{IncomingMessageEvent, OutgoingMessage, ChannelUser};
use kissbot_api::memory::{ChannelRequest, ThinkRequest, ToolCallRequest, ToolResultRequest};
use kissbot_api::message::Content;
/// 保留 role：空串 = 保留 role
pub const RESERVED_ROLE_NAME: &str = "";

// 上下文重置阈值来自会话模型 effective.max_tokens_usage（provider/model 配置合成）：
// 最近一次模型响应的 usage.total_tokens 超过其 compress_threshold（默认 0.8）倍时触发重置，
// 见 pipelines/message_sender.rs 的 TokenLimitMessageSender。

/// Nexus 全局单例（进程内唯一；new() 完成时注册，此后 get() 可用）。
/// 所有使用 Nexus 的位置一律不传参数、从单例获取（Session/Channel 不保存引用）。
static SINGLETON: OnceLock<Nexus> = OnceLock::new();

pub struct Nexus {
    memory_store_client: Arc<MemoryStoreClient>,
    memory_ego_client: Arc<MemoryEgoClient>,
    session_manager: Arc<SessionManager>,
    provider_manager: Arc<ProviderManager>,
    pipeline_manager: Arc<PipelineManager>,
    /// 每 channel 运行时管理（ChannelManager：内部 DashMap 无锁并发，含 pending/mode/client；
    /// 并持有 channel 变更串行队列，见 ChannelManager::change_channel_key / channel_command）
    channel_manager: Arc<ChannelManager>,
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
        let provider_manager = Arc::new(ProviderManager::new());
        let pipeline_manager = Arc::new(PipelineManager::new());

        let coordinator = Self {
            memory_store_client,
            memory_ego_client,
            session_manager,
            provider_manager,
            pipeline_manager,
            // channel 运行态与 channel 变更串行队列（队列消费者在 ChannelManager::new 内启动）
            channel_manager: Arc::new(ChannelManager::new()),
        };

        // 注册全局单例（此后 get() 可用；run() 中启动动作与连接回调均晚于此）
        let _ = SINGLETON.set(coordinator);

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

    /// 校验 role 存在（/role 切换前调用）：经 channel 配置取当前 agent_id（role 变更保持 agent 不变，
    /// channel 不存在报 ConfigNotFound）；显式空串（保留 role）直接通过，其余必须 ego 中存在
    pub async fn verify_role_exists_for_channel(&self, channel_id: &str, role_name: &str) -> Result<()> {
        let Some(ch) = ConfigManager::get().channel(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        self.verify_role_exists(ch.agent_id.as_str(), role_name).await
    }

    /// 按 channel 定位会话三元组（config agent_id/role_name + 运行态 mode；channel 不存在返回 None）
    /// 会话定位统一入口：Nexus 组合 config 与 channel_manager 运行态
    pub async fn session_key(&self, channel_id: &str) -> Option<SessionKey> {
        let ch = ConfigManager::get().channel(channel_id).await?;
        Some(SessionKey {
            agent_id: ch.agent_id.to_string(),
            role_name: ch.role_name.to_string(),
            // 运行态 mode（未绑定/缺失回退角色模式）
            mode: self.channel_manager.mode(channel_id).as_ref().clone(),
        })
    }

    /// 定位会话（不存在则创建并完成初始化：上下文恢复在 get_or_create 内，pipeline 同步与系统提示词在其之后）；返回会话
    /// key 传所有权（get_or_create 内部 move，非深拷贝）
    pub async fn ensure_session(&self, key: &SessionKey) -> Arc<Session> {
        let (session, new) = self.session_manager.get_or_create(key).await;
        if new {
            // 初始化 pipeline
            let _ = self.pipeline_manager.sync_pipeline(Arc::new(key.clone())).await;
            let _ = self.pipeline_manager.reset_system_prompt(key).await;
        }
        session
    }

    /// role 模式上下文构建（记忆回溯 reducer 调用）：查询记忆打包 → 归档旧上下文+清空缓存（内部幂等）→ 重建
    /// 取记忆用会话状态保存的 agent_id（来自会话 key）
    pub async fn build_context_from_memory_store(&self, session_key: &SessionKey) -> Vec<Message> {
        let cfg = ConfigManager::get().session_config::<MemoryRecoverConfig,EffectiveMemoryRecoverConfig>(session_key).await;
        self.memory_store_client
            .read_recent_for_context(session_key.agent_id.as_str(), session_key.role_name.as_str(), cfg.memory_time_secs, cfg.memory_count).await
            .map_or_else(|_| vec![], |msgs| pack_memory_messages(&msgs))
    }

    /// 按当前全部 channel 的绑定集合清理无绑定会话
    async fn prune_sessions(&self) {
        let channels = ConfigManager::get().channels().await;
        let mut keys = HashSet::new();
        for (_, ch) in &channels {
            if let Some(key) = self.session_key(ch.channel_id.as_str()).await {
                keys.insert(key);
            }
        }
        self.session_manager.retain(&keys);
    }

    /// 根据 agent_id 获取系统提示词（新建会话时由 MemoryEgoSystemPrompter 调用）：
    /// 保留 agent（agent_id="0"）返回 None，由 prompter 回退默认提示词；其余走 ego REST（agent 元数据 + 个体识别 + 角色设定，
    /// 失败静默跳过，全部失败回退默认提示词"你是 kissbot 智能助手"）；
    /// 通过 ego_md 模块将 ego 结构转为 markdown，替代手写提示词片段
    pub async fn system_prompt_for_agent(&self, agent_id: &str, role_name: &str) -> Option<String> {
        if agent_id == RESERVED_AGENT_ID {
            return None;
        }

        let mut system_parts = vec![];

        // 1. agent 元数据（按 agent_id 查询）-> 身份 markdown
        if let Ok(Some(metadata)) = self.memory_ego_client.get_agent(agent_id).await {
            system_parts.push(crate::ego_md::build_ego_identity_md(&metadata));
        }
        // 2. 个体识别（按 agent_id 查询）-> 个体识别 markdown，并收集匹配个体名
        if let Ok(Some(individuals)) = self.memory_ego_client.get_individuals(agent_id).await {
            system_parts.push(crate::ego_md::build_ego_individual_recognition_md(&individuals, None));
        }
        // 3. 角色设定（按 agent_id + role_name 查询）-> 角色 markdown
        if !role_name.is_empty() {
            if let Ok(Some(role)) = self.memory_ego_client.get_role(agent_id, role_name).await {
                system_parts.push(crate::ego_md::build_role_play_md(&role, None));
            }
        }

        if system_parts.is_empty() {
            return None;
        }

        Some(system_parts.join("\n"))
    }

    // ==================== 运行状态修改（管理命令入口） ====================

    /// agent/role/mode 变更统一入口：三个字段独立 Option，None = 保持当前值；
    /// 交给 ChannelManager 排队串行执行（执行体为本文件的 apply_channel_key），返回时已生效
    pub async fn change_channel_key(
        &self,
        channel_id: &str,
        agent_id: Option<Arc<String>>,
        role_name: Option<Arc<String>>,
        mode: Option<Arc<Mode>>,
    ) -> Result<()> {
        self.channel_manager.change_channel_key(channel_id, agent_id, role_name, mode).await
    }

    /// channel 配置变更统一入口（/bind、/unbind）：
    /// 交给 ChannelManager 排队串行执行（执行体为本文件的 apply_channel_command），返回时已生效
    pub async fn channel_command(&self, cmd: ChannelCommand) -> Result<String> {
        self.channel_manager.channel_command(cmd).await
    }

    // ---- 变更执行体（由 ChannelManager 的队列消费者串行调用，不对外） ----

    /// 来源 channel 绑定信息变化后重定位会话：清理无绑定会话 + 为新三元组创建会话
    /// 运行态 mode 写 Channel.mode（/mode 切换不回写，重启回 Role）
    /// None 字段 = 保持当前值：队列内结合 channel_manager 当前状态合成新三元组（写-写串行，读-改-写无竞态）
    pub(crate) async fn apply_channel_key(
        &self,
        channel_id: &str,
        agent_id: Option<Arc<String>>,
        role_name: Option<Arc<String>>,
        mode: Option<Arc<Mode>>,
    ) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            if let Some(agent_id) = agent_id.as_ref() {
                c.agent_id = agent_id.clone();
            }
            if let Some(role_name) = role_name.as_ref() {
                c.role_name = role_name.clone();
            }
        }).await?;
        if let Some(mode) = mode.as_ref() {
            self.channel_manager.set_mode(channel_id, mode.clone());
        }
        // 1. 清理无任何 channel 绑定的会话
        self.prune_sessions().await;
        // 2. 新三元组对应会话不存在则创建并构建初始上下文（agent 标识取会话 key）
        if let Some(key) = self.session_key(channel_id).await {
            self.ensure_session(&key).await;
        }
        Ok(())
    }

    /// channel 配置变更执行（队列内串行，不对外）：分发到 ChannelManager 方法
    pub(crate) async fn apply_channel_command(&self, cmd: ChannelCommand) -> Result<String> {
        match cmd {
            ChannelCommand::BindUser { channel_id, user } => {
                self.channel_manager.bind_user(&channel_id, &user).await?;
                Ok(format!("✅ 已绑定 channel 用户: {} / {}", user.messenger_id, user.user_id))
            },
            ChannelCommand::UnbindUser { channel_id, user } => {
                self.channel_manager.unbind_user(&channel_id, &user).await?;
                Ok(format!("✅ 已移除 channel 用户: {} / {}", user.messenger_id, user.user_id))
            },
        }
    }

    /// 校验模型有效性：从 API 拉模型列表，确认 pm.model 在列表中。
    /// Err 表示校验失败（API 调用失败 / 模型不在列表），调用方决定如何处理。
    async fn verify_model(&self, provider: &str, model: &str) -> Result<()> {
        let models = self.provider_manager.list_models(provider).await
            .map_err(|e| Error::ModelApiError(format!("获取模型列表失败: {}", e)))?;
        if !models.iter().any(|m| m.as_str() == model) {
            return Err(Error::ModelProviderNotSupported(format!(
                "模型 {} 不在 {} 的 API 模型列表", model, provider)));
        }
        Ok(())
    }

    /// 设置来源 channel 所属会话的模型（每次切换都从 API 拉模型列表校验）
    pub async fn set_session_model(&self, channel_id: &str, provider: &str, model: &str) -> Result<()> {
        let Some(key) = self.session_key(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        // 每次切换都从 API 拉模型列表校验（失败拒绝，保持原模型）
        self.verify_model(provider, model).await?;
        let mut config = LLMConfig::default();
        config.provider = Some(Arc::new(provider.to_string()));
        config.model = Some(Arc::new(model.to_string()));
        ConfigManager::get().set_session_config::<LLMConfig, EffectiveLLMConfig>(&key, &config).await
    }

    /// 启动主循环（保持进程运行）：初始化会话 + 连接全部 channel
    pub async fn run(&self) {
        info!("Nexus 启动，等待外部输入...");
        // 按 channel 绑定三元组初始化会话集合（agent_id 取 config，保留 agent = "0"）
        for (_, ch) in ConfigManager::get().channels().await {
            if let Some(key) = self.session_key(ch.channel_id.as_str()).await {
                self.ensure_session(&key).await;
            }
        }
        // 连接全部 enabled 的 channel（连接/重连/回显/发送归 ChannelManager 通道适配层；
        // 调度由 Nexus 负责：启动遍历 enabled 逐个连接，将来运行时新建 channel 也经此调度）
        let channels = ConfigManager::get().channels().await;
        for (_, ch) in &channels {
            if ch.enabled {
                self.channel_manager.clone().connect_channel(ch.channel_id.as_str()).await;
            }
        }
        // channel-client 通过 Terminal 回调驱动，此处保持进程不退出
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
}

// ==================== 消息处理 ====================

impl Nexus {
    /// 业务消息入口（由 ChannelManager 的 Terminal 转发调用；回显已在通道层 consume_pending 过滤，此处不见自身回显）
    /// 完整处理链：会话定位/上行记忆 → 系统事件过滤 → 管理命令 → 普通消息合批进 agentic loop
    pub async fn incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        // 0. 先取 channel 配置（命令分支 admin 判断与普通分支会话定位/记忆复用，避免重复查询）
        let Some(ch) = ConfigManager::get().channel(channel_id).await else { return; };

        // 1. 管理命令：仅 Content::Text 且以 "/" 开头才判断（不 trim——前置空格可跳过命令检查）；
        //    命令与命令回复不找 session_key、不入记忆（管理命令独立处理）
        if let Content::Text(text) = &event.incoming_message.content {
            if text.starts_with('/') {
                // 2. 判断是否管理员（admins HashSet O(1) contains，ch 已保存）；非管理员忽略不回复
                let messenger_id = event.incoming_message.messenger_id.as_str();
                let user_id = event.incoming_message.user_id.as_str();
                let is_admin = ch.admins.contains(&ChannelUser {
                    messenger_id: messenger_id.to_string(),
                    user_id: user_id.to_string(),
                });
                if is_admin {
                    // 3. 命令解析+执行（内联在 CommandRouter::execute）；回复始终发回来源 channel（不走 out_channel）
                    match CommandRouter::execute(text.as_str(), channel_id).await {
                        Ok(reply) => self.send_admin_reply(channel_id, event, reply).await,
                        Err(Error::InvalidCommand(msg)) => {
                            self.send_admin_reply(channel_id, event, format!("⚠️ {}", msg)).await;
                        }
                        Err(e) => {
                            self.send_admin_reply(channel_id, event, format!("❌ 命令执行失败: {}", e)).await;
                        }
                    }
                }
                // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
                return;
            }
        }

        // 2. 会话定位（config agent_id/role_name + 运行态 mode；channel 已存在，不重复查询）
        let mode = self.channel_manager.mode(channel_id);
        let key = SessionKey {
            agent_id: ch.agent_id.to_string(),
            role_name: ch.role_name.to_string(),
            mode: mode.as_ref().clone(),
        };

        // 3. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 直接从 config clone Arc，
        //    不 clone string；role 编码由 role_mode 函数按 config role + 运行态 mode 算）
        let agent_id = ch.agent_id.clone();
        let role_event = role_mode(ch.role_name.as_str(), mode.as_ref());
        self.memory_store_client.push_channel_record(ChannelRequest {
            agent_id,
            role_name: Arc::new(role_event),
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

        // 4. 系统事件（群组变更/用户移除）不进 agentic loop
        match &event.incoming_message.content {
            Content::GroupJoin(_) | Content::GroupLeave(_) | Content::UserRemove(_) => return,
            _ => {}
        }

        // 5. 普通消息：运行pipeline
        let _ = self.ensure_session(&key).await;
        let _ = self.pipeline_manager.incoming_message(&key, event).await;
    }

    /// 系统命令回复：始终发回来源 channel（不走 out_channel）
    /// 命令与命令回复不入记忆（管理命令不产生会话上下文）：不找 session_key、不推 ChannelRecord
    /// 身份：messenger_id = incoming.messenger_id；user_id/self_user_id = event.recipient_user_id（接收方即发声身份，且是群成员）
    async fn send_admin_reply(&self, channel_id: &str, event: Arc<IncomingMessageEvent>, content: String) {
        let msg = OutgoingMessage {
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.recipient_user_id.clone(),
            group_id: event.incoming_message.group_id.clone(),
            content: Content::Text(Arc::new(content.clone())),
        };

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        if let Err(e) = self.channel_manager.send(channel_id, msg).await {
            warn!("send_admin_reply 失败: {:?}", e);
        }
    }

    pub async fn call_provider_model(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse> {
        self.provider_manager.call(llm_cfg, messages, tools).await
    }

    pub async fn run_pipeline(&self, session_key: &SessionKey, message: Message) -> Result<()> {
        let tools = self.tools_for_session(session_key).await;
        self.pipeline_manager.run_pipeline(session_key, message, &tools).await
    }

    pub async fn send_memory_think(&self, session_key: &SessionKey, key: Arc<String>, reasoning_content: Option<Arc<String>>, thinking: Option<Arc<String>>) {
        let now = Arc::new(Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        let agent_id = Arc::new(session_key.agent_id.clone());
        let role_event = Arc::new(role_mode(session_key.role_name.as_str(), &session_key.mode));
        let oc_cfg = ConfigManager::get().session_config::<OutChannelConfig,OutChannelConfig>(session_key).await;
        if let Some(out_channel) = oc_cfg.out_channel.as_ref() {
            let placeholder = placeholder_request(
                agent_id.clone(),
                role_event.clone(),
                Content::Think(key.clone()),
                now.clone(),
                out_channel.as_ref(),
            );
            self.memory_store_client.push_channel_record(placeholder).await;
        }
        let request = ThinkRequest {
            agent_id,
            role_name: role_event,
            reasoning_content,
            thinking,
            key,
            time: now,
        };
        self.memory_store_client.push_think(request).await;
    }

    pub async fn send_memory_tool_call(&self, session_key: &SessionKey, key: Arc<String>, tool_call: &ToolCall) {
        let now = Arc::new(Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        let agent_id = Arc::new(session_key.agent_id.clone());
        let role_event = Arc::new(role_mode(session_key.role_name.as_str(), &session_key.mode));
        let oc_cfg = ConfigManager::get().session_config::<OutChannelConfig,OutChannelConfig>(session_key).await;
        if let Some(out_channel) = oc_cfg.out_channel.as_ref() {
            let placeholder = placeholder_request(
                agent_id.clone(),
                role_event.clone(),
                Content::ToolCall(key.clone()),
                now.clone(),
                out_channel.as_ref(),
            );
            self.memory_store_client.push_channel_record(placeholder).await;
        }
        let request = ToolCallRequest {
            agent_id,
            role_name: role_event,
            tool_name: tool_call.name.clone(),
            tool_params: Arc::new(tool_call.data.arguments.clone()),
            key,
            time: now.clone(),
        };
        self.memory_store_client.push_tool_call(request).await;
    }

    pub async fn send_memory_tool_result(&self, session_key: &SessionKey, key: Arc<String>, tool_call: &ToolCall) {
        let now = Arc::new(Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        let agent_id = Arc::new(session_key.agent_id.clone());
        let role_event = Arc::new(role_mode(session_key.role_name.as_str(), &session_key.mode));
        let oc_cfg = ConfigManager::get().session_config::<OutChannelConfig,OutChannelConfig>(session_key).await;
        if let Some(out_channel) = oc_cfg.out_channel.as_ref() {
            let placeholder = placeholder_request(
                agent_id.clone(),
                role_event.clone(),
                Content::ToolResult(key.clone()),
                now.clone(),
                out_channel.as_ref(),
            );
            self.memory_store_client.push_channel_record(placeholder).await;
        }
        let request = ToolResultRequest {
            agent_id,
            role_name: role_event,
            tool_result: Arc::new(tool_call.data.result.clone()),
            tool_error: Arc::new(tool_call.data.error.clone()),
            key,
            time: now.clone(),
        };
        self.memory_store_client.push_tool_result(request).await;
    }

    /// 会话可用工具：context 配置的启用 toolkits 白名单 → Station 平铺查询（本地 + 直接子递归）
    /// tools 聚合为空则请求不携带 tools 字段（兼容无工具场景）
    pub async fn tools_for_session(&self, session_key: &SessionKey) -> Vec<Arc<ToolConfig>> {
        let cfg = ConfigManager::get().session_config::<ToolkitSetConfig,ToolkitSetConfig>(session_key).await;
        if cfg.toolkit_set.is_empty() {
            return Vec::new();
        }
        match Station::get().tools(Some(&cfg.toolkit_set), &[]).await {
            Ok(tools) => tools,
            Err(e) => {
                warn!("工具查询失败: {}", e);
                Vec::new()
            }
        }
    }

    /// Agentic Loop 产出回复：发到 out_channel（agent_id/role_name 定位、role_event 记忆编码）
    /// 发送前校验 out_channel 身份在目标 channel 仍绑定；未绑定 → 清理该 (agent, role) 的 out 配置并跳过
    /// role_event 由会话建立时算好传入（记忆编码）；role_name 用于清理定位（roles key 是原始 role，编码不可逆）
    pub async fn send_outgoing(&self, agent_id: &str, role_name: &str, role_event: &str, out_channel: &OutChannel, content: Arc<String>) {
        // 1. 校验 out_channel 身份在目标 channel 仍绑定（未绑定 = 配置悬空，清理并跳过发送）
        let bound = ConfigManager::get().channel(out_channel.channel_id.as_str()).await
            .map(|c| c.bind_users.contains(&out_channel.user))
            .unwrap_or(false);
        if !bound {
            warn!("send_outgoing: out_channel 身份未绑定，清理 {}/{} 的回复通道", agent_id, role_name);
            let _ = ConfigManager::get().set_out_channel(agent_id, role_name, None).await;
            return;
        }
        // 2. 发送（role_event 由会话建立时算好传入，无需再查 channel_manager 运行态 mode）
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
                self.memory_store_client.push_channel_record(ChannelRequest {
                    agent_id: Arc::new(agent_id.to_string()),
                    role_name: Arc::new(role_event.to_string()),
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

/// 从 Content 枚举中提取文本（普通消息路径使用，当前无消费方）
#[allow(dead_code)]
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

    use std::time::Instant;

    use kissbot_api::channel::IncomingMessage;

    use crate::configs::{ChannelBatchConfig, ChannelConfig, PipelineConfig};
    use crate::pipelines::{PP_IN_BATCH, PP_MSG_RAW, PP_OUT_CHANNEL, PP_SYS_DEFAULT, PP_TOOL_STATION};

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
        Nexus {
            memory_store_client: Arc::new(MemoryStoreClient::new()),
            memory_ego_client: Arc::new(MemoryEgoClient::new()),
            session_manager: SessionManager::new(data_dir.to_str().unwrap()),
            provider_manager: Arc::new(ProviderManager::new()),
            pipeline_manager: Arc::new(PipelineManager::new()),
            channel_manager: Arc::new(ChannelManager::new()),
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

    // ===== 消息入口链路（incoming_message → 合批 → pipeline → 会话上下文） =====
    // 覆盖一整条链：ensure_session（含新建会话的 pipeline 同步与系统提示词初始化）/ pipeline 分发 /
    // 合批 flush / context_append

    /// 进程级装配（幂等，与 session_manager 测试同模式）：ConfigManager/Station/Nexus 单例各注册一次。
    /// data_dir 目录经 OnceLock 保活，避免 tempdir drop 后单例路径失效
    static TEST_GLOBAL_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static TEST_INIT_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    async fn ensure_test_globals() {
        if !TEST_INIT_DONE.load(std::sync::atomic::Ordering::Relaxed) {
            let dir = TEST_GLOBAL_DIR.get_or_init(|| tempfile::tempdir().unwrap());
            let cfg_path = dir.path().join("config.json");
            let cfg_json = format!(
                r#"{{"api":{{"memory_store_url":"","memory_ego_url":""}},"security":{{"api_key":"user-key-456","admin_api_key":"admin-key-123"}},"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9092,"ws_reconnect_interval_secs":5}}}}"#,
                dir.path().join("data").to_str().unwrap()
            );
            std::fs::write(&cfg_path, cfg_json).unwrap();
            // 2024 edition：设置环境变量需要 unsafe
            unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };
            // 幂等：每个单例只注册一次（重复 new 的实例被丢弃，set 失败被忽略）
            let _ = ConfigManager::new().await;
            let _ = Station::new().await;
            let _ = Nexus::new().await;
            TEST_INIT_DONE.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn incoming_message_grows_session_context_through_pipeline() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let agent_id = "a1";
        let channel_id = "nexus-chain-test";
        let key = SessionKey { agent_id: agent_id.into(), role_name: "".into(), mode: Mode::Role };

        // 1. agent 级（role 空串）pipeline：显式指定全部组件（不依赖 preset 映射），必须在会话首次合成
        //    session 配置之前写入。message_sender 用 raw：它先把消息写进会话上下文再调模型
        //    （provider 未配置 → 模型调用返回 Err，不影响本用例断言上下文已增长）
        let mut pipeline = PipelineConfig::default();
        pipeline.input_processor = Some(Arc::new(PP_IN_BATCH.into()));
        pipeline.system_prompter = Some(Arc::new(PP_SYS_DEFAULT.into()));
        pipeline.message_sender = Some(Arc::new(PP_MSG_RAW.into()));
        pipeline.tool_caller = Some(Arc::new(PP_TOOL_STATION.into()));
        pipeline.output_processor = Some(Arc::new(PP_OUT_CHANNEL.into()));
        cm.set_agent_role_config::<PipelineConfig, PipelineConfig>(agent_id, "", Arc::new(pipeline)).await.unwrap();
        // 合批间隔缩到 1 秒，让 flush 尽快发生
        cm.set_agent_role_config::<ChannelBatchConfig, ChannelBatchConfig>(
            agent_id, "", Arc::new(ChannelBatchConfig { channel_batch_interval_secs: 1 })).await.unwrap();

        // 2. channel 绑定 (a1, "")；channel_id 用本用例专用名，避免与其他测试互相干扰
        let _ = cm.add_channel(ChannelConfig {
            channel_id: Arc::new(channel_id.into()),
            ws_url: Arc::new("ws://127.0.0.1:8399".into()),
            admins: Arc::new(HashSet::new()),
            bind_users: Arc::new(HashSet::new()),
            agent_id: Arc::new(agent_id.into()),
            role_name: Arc::new("".into()),
            enabled: true,
        }).await;

        // 3. 消息入口（非命令消息，直接进 pipeline）：ensure_session 新建会话时会同步 pipeline
        //    并初始化系统提示词，随后消息分发到合批输入处理器
        Nexus::get().incoming_message(channel_id, Arc::new(IncomingMessageEvent {
            recipient_user_id: Arc::new("self".into()),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: Arc::new("m1".into()),
                messenger_id: Arc::new("web".into()),
                user_id: Arc::new("u1".into()),
                group_id: Arc::new("g1".into()),
                messenger_name: Arc::new("".into()),
                user_name: Arc::new("u1".into()),
                group_name: Arc::new("".into()),
                content: Content::Text(Arc::new("链路测试消息".into())),
                time: Arc::new("2026-08-07 10:00:00".into()),
            }),
        })).await;

        // 4. 等合批 flush + pipeline 运行：会话上下文应含系统消息（首条，由系统提示词初始化设置）
        //    与本条用户消息
        let session = Nexus::get().ensure_session(&key).await;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let ctx = session.build_context().await;
            let has_system = matches!(ctx.first(), Some(Message::System { .. }));
            let has_user = ctx.iter().any(|m| matches!(m, Message::User { content } if content.as_str().contains("链路测试消息")));
            if has_system && has_user {
                break;
            }
            assert!(Instant::now() < deadline,
                "消息应经 incoming_message → 合批 → pipeline 进入会话上下文，实际: {:?}", ctx);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn channel_command_runs_through_channel_manager_queue() {
        // 覆盖变更队列链路：Nexus::channel_command → ChannelManager 排队 → 队列消费者
        // → Nexus::apply_channel_command → ChannelManager::bind_user/unbind_user 落库
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let channel_id = "nexus-queue-test";
        let _ = cm.add_channel(ChannelConfig {
            channel_id: Arc::new(channel_id.into()),
            ws_url: Arc::new("ws://127.0.0.1:8398".into()),
            admins: Arc::new(HashSet::new()),
            bind_users: Arc::new(HashSet::new()),
            agent_id: Arc::new("a1".into()),
            role_name: Arc::new("".into()),
            enabled: true,
        }).await;

        let user = ChannelUser { messenger_id: "web".into(), user_id: "u-queue".into() };
        let reply = Nexus::get().channel_command(ChannelCommand::BindUser {
            channel_id: channel_id.to_string(), user: user.clone(),
        }).await.expect("绑定命令应执行成功");
        assert!(reply.contains("已绑定"), "回复文案应说明已绑定: {}", reply);
        let ch = cm.channel(channel_id).await.expect("channel 应存在");
        assert!(ch.bind_users.contains(&user), "绑定应落库");

        let reply = Nexus::get().channel_command(ChannelCommand::UnbindUser {
            channel_id: channel_id.to_string(), user: user.clone(),
        }).await.expect("解绑命令应执行成功");
        assert!(reply.contains("已移除"), "回复文案应说明已移除: {}", reply);
        let ch = cm.channel(channel_id).await.expect("channel 应存在");
        assert!(!ch.bind_users.contains(&user), "解绑应落库");
    }
}
