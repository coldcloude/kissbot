use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use chrono::Local;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::channel_manager::ChannelManager;
use crate::types::{
    Mode, Message, Result, Error, SessionKey, memory_role,
};
use crate::session_manager::{Session, SessionManager};
use crate::config_manager::{ConfigManager, ProviderModel, OutChannel, ToolConfig, EffectiveContextConfig};
use crate::command_router::CommandRouter;
use crate::model_client::ModelClient;
use crate::memory_reader::{MemoryReader, pack_memory_messages};
use crate::memory_store_client::MemoryStoreClient;
use crate::station::{self, StationRuntime};

use kissbot_api::channel::{IncomingMessageEvent, OutgoingMessage, ChannelUser};
use kissbot_api::memory::{ChannelRequest, ThinkRequest, ToolCallRequest, ToolResultRequest};
use kissbot_api::message::Content;
/// 保留 agent/role：agent_name 为空 = 保留 agent（建会话但初始上下文用默认系统提示词，见 ensure_session）；
/// 保留 agent 的 memory-store/ego agent_id 为 RESERVED_AGENT_ID（"0"）。
pub const RESERVED_AGENT_NAME: &str = "";
pub const RESERVED_AGENT_ID: &str = "0";
pub const RESERVED_ROLE_NAME: &str = "";

/// Agentic Loop 工具调用轮次上限（防死循环）
const MAX_TOOL_ROUNDS: usize = 10;

// 上下文消息数量上限（溢出触发重置/压缩）已废弃硬编码常量 MAX_CONTEXT_MESSAGES——
// 阈值统一由会话模型 effective.max_context_messages（provider/model 配置合成）决定，见 run_agentic_loop 溢出检查。

/// agent/role/event 变更任务（mpsc 队列串行处理，避免写-写竞态；读无需外部加锁）
/// 统一为「应用新的会话三元组」：写 config + 运行态 mode + （可选）agent_id + 会话重定位
enum ConfigChange {
    /// 应用新会话三元组（agent/role/mode 任一变化）；agent_id 仅 /agent 切换时 Some
    ApplyKey { channel_id: String, new_key: SessionKey, agent_id: Option<Arc<String>>, done: tokio::sync::oneshot::Sender<Result<()>> },
}

/// AgentCoordinator 全局单例（进程内唯一；new() 完成时注册，此后 instance() 可用）。
/// 所有使用 coordinator 的位置一律不传参数、从单例获取（Session/Channel 不保存引用）。
static SINGLETON: OnceLock<AgentCoordinator> = OnceLock::new();

pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    memory_reader: Arc<MemoryReader>,
    memory_store_client: Arc<MemoryStoreClient>,
    session_manager: Arc<SessionManager>,
    model_client: Arc<tokio::sync::Mutex<ModelClient>>,
    /// 启动校验后的 default_model（从 API 模型列表校验）；None = 无模型（普通消息静默忽略）
    valid_default: ArcSwap<Option<ProviderModel>>,
    /// 每 channel 运行时管理（ChannelManager：内部 DashMap 无锁并发，含 pending/agent_id/mode/client）
    channel_manager: Arc<ChannelManager>,
    /// agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
    command_tx: tokio::sync::mpsc::UnboundedSender<ConfigChange>,
    /// station_id → StationRuntime（启动时按配置构建；base_url 为空的本地 station 注册内置 Read 工具）
    station_runtimes: Arc<DashMap<String, Arc<StationRuntime>>>,
}

impl AgentCoordinator {
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn instance() -> &'static AgentCoordinator {
        SINGLETON.get().expect("AgentCoordinator 未初始化")
    }

    pub async fn new(
        config: Arc<ConfigManager>,
    ) -> Result<()> {
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let data_dir = config.data_dir().to_string();
        let session_manager = SessionManager::new(&data_dir);
        let model_client = ModelClient::new(config.clone());
        // agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<ConfigChange>();

        let coordinator = Self {
            config: config.clone(),
            memory_reader,
            memory_store_client,
            session_manager,
            model_client: Arc::new(tokio::sync::Mutex::new(model_client)),
            channel_manager: Arc::new(ChannelManager::new(config.clone())),
            valid_default: ArcSwap::from_pointee(None),
            command_tx,
            station_runtimes: Arc::new(DashMap::new()),
        };

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
        // 以及连接全部 enabled channel——已整体移入 run()（启动动作统一在 run 中执行）

        // 注册全局单例（此后 instance() 可用；run() 中启动动作与连接回调均晚于此）
        let _ = SINGLETON.set(coordinator);

        // 启动变更消费者：agent/role/event 变更串行处理（避免写-写竞态；读不受影响）
        // spawn 晚于 SINGLETON.set，任务内 instance() 必然就绪
        tokio::spawn(async move {
            while let Some(change) = command_rx.recv().await {
                match change {
                    ConfigChange::ApplyKey { channel_id, new_key, agent_id, done } => {
                        let coordinator = AgentCoordinator::instance();
                        let rst = coordinator.apply_channel_key(&channel_id, &new_key, agent_id).await;
                        let _ = done.send(rst);
                    }
                }
            }
        });

        info!("AgentCoordinator 初始化完成");
        Ok(())
    }

    // ==================== 会话定位与构建 ====================

    /// 按来源 channel 的绑定配置 + 运行态 mode 计算会话 key（agent/role 取绑定配置，纯函数逻辑见 session_key_of）
    fn session_key_for(&self, ch: &crate::config_manager::ChannelConfig) -> SessionKey {
        // 运行态 mode 参与会话定位（从 ChannelManager 读）；无脱离态，agent_name 为空 = 保留 agent
        let mode = self.channel_mode(&ch.channel_id);
        session_key_of(&ch.agent_name, &ch.role_name, mode)
    }

    /// 读取 channel 运行态 agent_id；未绑定（异常路径）时懒绑定
    async fn channel_agent(&self, channel_id: &str) -> Arc<String> {
        if let Some(agent_id) = self.channel_manager.agent_id(channel_id) {
            return agent_id;
        }
        // 未绑定/缺失：懒绑定（正常启动路径已在 run() 中绑定全部 channel）
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
        self.channel_manager.set_agent_id(channel_id, agent_id.clone());
        agent_id
    }

    /// 设置 channel 运行态模式（写 Channel.mode；/mode 切换，不回写，重启回 Role）
    pub fn set_channel_mode(&self, channel_id: &str, mode: Mode) {
        self.channel_manager.set_mode(channel_id, mode);
    }

    /// 取 channel 运行态模式（未绑定/缺失回退角色模式）
    pub fn channel_mode(&self, channel_id: &str) -> Mode {
        self.channel_manager.mode(channel_id)
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
            // 新建会话上下文：event 从缓存恢复（全量回读；文件不存在为空，不清理）；role 查询记忆重建（归档+清空在 build_role_context 内部）
            match &*session.mode {
                Mode::Event(_) => {
                    let _ = session.context.lock().await.recover_from_cache().await;
                }
                Mode::Role => {
                    self.build_role_context(&session).await;
                }
            }
            // 系统消息：保留 agent（agent_id="0"）用 NexusRepo 默认系统提示词；其余走 load_ego_info。
            // 生成结果执行一次 set（待定，下次发送前对比应用；与缓存恢复的系统不一致时旧上下文先归档）
            if session.agent_id.as_str() == RESERVED_AGENT_ID {
                let prompt = self.config.default_system_prompt().await;
                session.context.lock().await.set_system_message(prompt);
            } else if let Ok(ego_info) = self.load_ego_info(session.agent_id.as_str(), &session.role_name).await {
                session.context.lock().await.set_system_message(ego_info);
            }
            // 顶层记忆索引（memory-struct 未实现时静默跳过）
            let _ = self.memory_reader
                .read_memory_struct_index(&self.config, session.agent_id.as_str(), &session.role_name, &session.mode)
                .await;
        }
        (session, created)
    }

    /// role 模式上下文构建（新建/溢出重置共用）：查询记忆打包 → 归档旧上下文+清空缓存（内部幂等）→ 重建
    /// 取记忆用会话状态保存的 agent_id（session_key 仅去重，不从 key 提取 agent_name）
    async fn build_role_context(&self, session: &Arc<Session>) {
        // 记忆打包：组合查询 + 每组合全史查询 + 并集算法（最后 N 条 ∪ [M, T_N] 同时间组，窗口内早于 T_N 的记录不含），打包为一条 user 消息作为首条内容
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        let packed = if let Ok(msgs) = self.memory_reader
            .read_recent_for_context(session.agent_id.as_str(), session.role_name.as_str(), &cfg)
            .await
        {
            pack_memory_messages(&msgs)
        } else {
            None
        };
        // 归档旧上下文（新建时无内容幂等跳过）+ 清空缓存 → 重建（清空内存 + 从内存写回缓存；无消息不落盘）
        let _ = session.context.lock().await.archive_and_clear_cache().await;
        let msgs = packed.map(|m| vec![m]).unwrap_or_default();
        let _ = session.context.lock().await.rebuild(msgs).await;
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

    /// event 模式超长压缩：归档当前缓存 → LLM 总结（compress_prompt + 当前上下文）→
    /// 重写缓存为 system + user(压缩指令) + assistant(总结)，等待后续 channel 消息
    async fn compress_context(&self, session: &Arc<Session>) {
        // 0. 先校验模型可用（无模型早退，避免留下冗余归档副本）
        let model = session.model.load_full();
        let Some(pm) = model.as_ref() else { return; };
        // 发送前应用待定系统消息（压缩也是一次发送：不一致时旧系统上下文先归档，再按新系统压缩）
        let _ = session.context.lock().await.apply_pending_system().await;
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        // 1. 取当前完整上下文（含 system），末尾追加压缩指令 user 消息（压缩基于当前 session）
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
        // 3. 压缩完成后：归档当前上下文（含原系统消息）→ 清空缓存 → 重建压缩后上下文
        // （归档与清空连在一起，中间不隔压缩；压缩前 apply_pending_system 已处理系统切换）
        let _ = session.context.lock().await.archive_and_clear_cache().await;
        let _ = session.context.lock().await.rebuild(compressed_messages(&cfg, &summary)).await;
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

    /// 启动主循环（保持进程运行）：绑定运行态 agent + 初始化会话 + 连接全部 channel
    pub async fn run(&self) {
        info!("AgentCoordinator 启动，等待外部输入...");
        // 启动：为全部 channel 绑定运行态 agent（解析失败回退保留 agent），
        // 再按 channel 绑定三元组初始化会话集合（agent_name 为空 = 保留 agent，同样建会话）
        for (_, ch) in self.config.channels().await {
            self.bind_channel_runtime(&ch.channel_id).await;
            let key = self.session_key_for(&ch);
            self.ensure_session(&key, &ch.channel_id).await;
        }
        // 连接所有 enabled 的 channel（连接/重连/回显/发送全部归 ChannelManager 通道适配层）
        self.channel_manager.connect_all().await;
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
        let Some(ch) = self.config.channel(channel_id).await else { return; };

        // 2. 推上行消息到记忆（is_self=0，name 取自 IncomingMessage；agent_id 取来源 channel 运行态绑定，事件模式编码）
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
        self.enqueue_batch(&session, event).await;
    }

    /// 合批：数据直取会话生产侧入队（Arc<IncomingMessageEvent>）→ 更新截止时间（防抖）→ 发送触发时间（At）。
    /// 无 sleep、无逐消息任务——触发由 session 的 trigger 任务经 DelayQueue 定时处理。
    /// BatchProducer 已从 Channel 删除：enqueue 时 ensure_session 已返回会话，生产侧直接取 session.batch_producer，无 Channel 中转
    async fn enqueue_batch(&self, session: &Arc<Session>, event: Arc<IncomingMessageEvent>) {
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        let interval = Duration::from_secs(cfg.channel_batch_interval_secs);

        let producer = Arc::new(session.batch_producer.clone());
        let _ = producer.tx.send(event);                                // 数据入队（队列累积，不逐条消费）
        let at = Instant::now() + interval;                             // 单次计算：deadline 与触发时间同源
        producer.set_deadline(at);                                      // 更新截止（防抖，后推覆盖）
        let _ = producer.trigger_tx.send(crate::session_manager::Trigger::At(at));  // 发送触发时间（绝对）
    }

    async fn handle_admin_command(
        &self,
        channel_id: &str,
        event: &Arc<IncomingMessageEvent>,
        content: &str,
    ) {
        match CommandRouter::parse(content) {
            Ok(cmd) => {
                match CommandRouter::execute(&cmd, &self.config, channel_id).await {
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
    async fn send_admin_reply(&self, channel_id: &str, event: &Arc<IncomingMessageEvent>, content: String) {
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

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        match self.channel_manager.send(channel_id, msg).await {
            Ok(response) => {
                // 下行成功后：推记忆（is_self=1）
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = self.channel_agent(channel_id).await;
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
    pub(crate) async fn resolve_out_channel_for_session(&self, session: &Arc<Session>) -> Option<OutChannel> {
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

    pub(crate) async fn run_agentic_loop(&self, _channel_id: &str, session: &Arc<Session>, content_text: String, out_channel: &OutChannel) {
        // 无可用模型：静默忽略普通消息（仅管理指令可用）
        if session.model.load().is_none() {
            return;
        }

        // 0. 发送前应用待定系统消息变更（对比当前；不一致 → 旧上下文（含原系统消息）归档历史 → 替换 → 重建缓存）
        let _ = session.context.lock().await.apply_pending_system().await;
        // 1. 追加用户消息到该会话上下文（合批已打包为一条 user 消息，time/messenger 等不保留，只留文本）
        // 内存 + 缓存一体追加（best-effort，失败仅丢缓存不阻塞流程）
        let _ = session.context.lock().await.append(&[Message::User { content: Arc::new(content_text.clone()) }]).await;

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
                    // 4. 追加 assistant(tool_calls)（内存 + 缓存一体）
                    // reasoning_content 必须保留并随请求回传：DeepSeek 带 tools 的请求须完整回传否则 400；
                    // Kimi 单轮工具循环（多步推理）须保留并回传全部思考内容；openai_body 自动序列化
                    let _ = session.context.lock().await.append(&[Message::Assistant {
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
                        // 5d. Tool 消息（内存 + 缓存一体）
                        let _ = session.context.lock().await.append(&[Message::Tool { tool_call_id: call.id.clone(), name: call.name.clone(), content: Arc::new(result_text.clone()) }]).await;
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

                    // 6. 追加 assistant 回复（内存 + 缓存一体）
                    // 超限兜底：rounds 超过 MAX_TOOL_ROUNDS 后模型仍返回 tool_calls 时 content 为空，
                    // 用兜底文案作为回复（不把空内容发送给用户）
                    // reasoning_content 保留并回传：带 tools 的请求须完整回传所有 assistant 的思考内容
                    // （DeepSeek 400 规则 / Kimi 保留式思考）；同时思考内容在步骤 7 写 memory-store think 记录
                    let reply_content = if model_resp.tool_calls.is_empty() {
                        model_resp.content.clone()
                    } else {
                        "工具调用轮次已达上限，请稍后再试".to_string()
                    };
                    let _ = session.context.lock().await.append(&[Message::Assistant {
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
            warn!("会话上下文超长，触发重建: role={} mode={:?}", session.role_name, session.mode);
            // 按模式重建：event 超长压缩（LLM 总结归档）；role 从记忆重建（新建/重置共用 build_role_context）
            match &*session.mode {
                Mode::Event(_) => self.compress_context(session).await,
                Mode::Role => {
                    self.build_role_context(session).await;
                    info!("会话上下文已重置: role={} mode={:?}", session.role_name, session.mode);
                }
            }
            // 重建完成：强制 flush（不检查 deadline），重建期间到达的消息即刻并入新上下文（Role/Event 共通）
            // （重建可能由 trigger 任务的 flush → run_agentic_loop 溢出路径调用，Forced 入队后由任务串行处理）
            session.batch_producer.trigger_tx.send(crate::session_manager::Trigger::Forced).ok();
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

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        match self.channel_manager.send(out_channel.channel_id.as_str(), msg).await {
            Ok(response) => {
                // 下行成功后：推记忆（is_self=1）
                let key = self.session_key_for(&ch);
                let role_name = memory_role(&key.role_name, &key.mode);
                let agent_id = self.channel_agent(out_channel.channel_id.as_str()).await;
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
        // 构造最小 Session + OutChannel（参照既有测试模式；经 get_or_create 走真实构造路径）
        let key = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let (session, _) = mgr.get_or_create(&key, None, Arc::new("aid".into()));
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
