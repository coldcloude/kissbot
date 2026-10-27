use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use arcstr::ArcStr;
use kissbot_api::RESERVED_AGENT_ID;
use tracing::{info, warn};

use crate::abstract_client::AbstractClient;
use crate::channel_manager::ChannelManager;
use crate::pipelines::PipelineManager;
use crate::provider::ProviderManager;
use crate::types::{Error, MemoryKey, Result};
use crate::config_manager::ConfigManager;
use crate::command_router::CommandRouter;
use crate::memory_ego_client::MemoryEgoClient;
use crate::memory_store_client::MemoryStoreClient;

use kissbot_api::channel::{IncomingMessageEvent, OutgoingMessage, ChannelUser};
use kissbot_api::message::Content;

// 上下文压缩/回溯阈值来自当前 session 模型 effective.max_tokens_usage（provider/model 配置合成）：
// 最近一次模型响应的 usage.total_tokens 超过其 compress_threshold（默认 0.8）倍时触发，
// 见 pipelines.rs 的 send_messages_with_limit 与 DefaultAgenticLoopPipelineWithMemory。

/// Nexus 全局单例（进程内唯一；new() 完成时注册，此后 get() 可用）。
/// 所有使用 Nexus 的位置一律不传参数、从单例获取（Session/Channel 不保存引用）。
static SINGLETON: OnceLock<Nexus> = OnceLock::new();

pub struct Nexus {
    pub memory_store_client: Arc<MemoryStoreClient>,
    pub memory_ego_client: Arc<MemoryEgoClient>,
    pub abstract_client: Arc<AbstractClient>,
    pub provider_manager: Arc<ProviderManager>,
    pub pipeline_manager: Arc<PipelineManager>,
    pub channel_manager: Arc<ChannelManager>,
}

impl Nexus {
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn get() -> &'static Nexus {
        SINGLETON.get().expect("Nexus 未初始化")
    }

    pub async fn new() -> Result<()> {
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let memory_ego_client = Arc::new(MemoryEgoClient::new());
        let abstract_client = Arc::new(AbstractClient::new());
        let provider_manager = Arc::new(ProviderManager::new());
        let pipeline_manager = Arc::new(PipelineManager::new());
        // channel 运行态与 channel 变更串行队列（队列消费者在 ChannelManager::new 内启动）
        let channel_manager = ChannelManager::new();

        let coordinator = Self {
            memory_store_client,
            memory_ego_client,
            abstract_client,
            provider_manager,
            pipeline_manager,
            channel_manager,
        };

        // 注册全局单例（此后 get() 可用；run() 中启动动作与连接回调均晚于此）
        let _ = SINGLETON.set(coordinator);

        info!("Nexus 初始化完成");
        Ok(())
    }

    // ==================== pipeline 定位与构建 ====================

    /// 校验 agent_id 存在（/agent 切换前调用）：空或保留 id "0" 直接通过；
    /// ego 未配置/HTTP 失败/agent 不存在返回 Err（调用方保持原 agent 不变）
    pub async fn verify_agent_exists(&self, agent_id: ArcStr) -> Result<()> {
        if agent_id.is_empty() || agent_id == RESERVED_AGENT_ID {
            return Ok(());
        }
        if self.memory_ego_client.get_agent(agent_id.clone()).await?.is_some() {
            Ok(())
        } else {
            Err(Error::MemoryEgoError(format!("agent 不存在: {}", agent_id).into()))
        }
    }

    /// 校验 role 存在（apply_channel_key 应用前调用）：显式空串（保留 role）直接通过；
    /// 其余必须 ego 中存在，否则 Err（调用方保持原 role 不变）
    pub async fn verify_role_exists(&self, agent_id: ArcStr, role_name: ArcStr) -> Result<()> {
        if role_name.is_empty() {
            return Ok(());
        }
        if self.memory_ego_client.get_role(agent_id, role_name.clone()).await?.is_some() {
            Ok(())
        } else {
            Err(Error::MemoryEgoError(format!("role 不存在: {}", role_name).into()))
        }
    }

    /// 校验 role 存在（/role 切换前调用）：经 channel 配置取当前 agent_id（role 变更保持 agent 不变，
    /// channel 不存在报 ConfigNotFound）；显式空串（保留 role）直接通过，其余必须 ego 中存在
    pub async fn verify_role_exists_for_channel(&self, channel_id: &str, role_name: ArcStr) -> Result<()> {
        let Some(ch) = ConfigManager::get().channel(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id).into()));
        };
        self.verify_role_exists(ch.agent_id.clone(), role_name).await
    }

    /// 定位 pipeline（不存在则按 agent/role 配置装配；pipeline 自己决定需要哪些 session）
    async fn ensure_pipeline(&self, memory_key: Arc<MemoryKey>) {
        if let Err(e) = self.pipeline_manager.ensure_pipeline(memory_key).await {
            warn!("pipeline 装配失败: {:?}", e);
        }
    }

    /// 按当前全部 channel 的绑定集合清理无绑定 pipeline；pipeline 持有的 session 随 Arc 释放回收。
    /// pub(crate)：channel 侧的 apply_channel_key 在 pipeline 重定位时调用
    pub(crate) async fn prune_pipelines(&self) {
        let channels = ConfigManager::get().channels().await;
        let mut keys = HashSet::new();
        for (_, ch) in &channels {
            if let Some(key) = self.channel_manager.memory_key(ch.channel_id.as_str()).await {
                keys.insert(key);
            }
        }
        self.pipeline_manager.retain(&keys);
    }

    /// channel 绑定变化后的 pipeline 重绑定（解绑旧 channel + 清理无绑定 pipeline/session + 装配新 pipeline），
    /// channel 侧 apply_channel_key 在写完新配置后调用。
    ///
    /// `old_key` 为变化前的 pipeline 标识；与 `new_key` 不同时先通知旧 pipeline「该 channel 不再绑定本 pipeline」，
    /// 由 pipeline 清理该 channel（messenger + 绑定用户）的批次生产者（否则旧 channel 的消息会继续进旧 pipeline）。
    /// `bind_users` 是变化前该 channel 的绑定身份（一个 channel 可绑定多个 messenger 用户）。
    pub(crate) async fn rebind_pipeline(
        &self,
        old_key: Option<Arc<MemoryKey>>,
        new_key: Arc<MemoryKey>,
        bind_users: &HashSet<Arc<kissbot_api::channel::ChannelUser>>,
    ) {
        if let Some(old_key) = old_key {
            if old_key != new_key {
                for user in bind_users {
                    self.pipeline_manager
                        .channel_unbound(&old_key, user.messenger_id.as_str(), user.user_id.as_str())
                        .await;
                }
            }
        }
        // 清理无任何 channel 绑定的 pipeline（session 随 pipeline 一并清理），再为新三元组装配 pipeline
        self.prune_pipelines().await;
        self.ensure_pipeline(new_key).await;
    }

    // ==================== 运行状态修改（管理命令入口） ====================

    /// channel 运行态与变更队列（channel 侧入口：change_channel_key / channel_command 都在 ChannelManager 上）
    pub fn channel_manager(&self) -> &Arc<ChannelManager> {
        &self.channel_manager
    }

    /// 校验模型有效性：从 API 拉模型列表，确认 pm.model 在列表中。
    /// Err 表示校验失败（API 调用失败 / 模型不在列表），调用方决定如何处理。
    async fn verify_model(&self, provider: &str, model: &str) -> Result<()> {
        let models = self.provider_manager.list_models(provider).await
            .map_err(|e| Error::ModelApiError(format!("获取模型列表失败: {}", e).into()))?;
        if !models.iter().any(|m| m.as_str() == model) {
            return Err(Error::ModelProviderNotSupported(format!(
                "模型 {} 不在 {} 的 API 模型列表", model, provider).into()));
        }
        Ok(())
    }

    /// 设置 pipeline 的模型（每次切换都从 API 拉模型列表校验）；写入哪些 session 由 pipeline 决定
    pub async fn set_pipeline_model(&self, memory_key: Arc<MemoryKey>, provider: ArcStr, model: ArcStr) -> Result<()> {
        // 每次切换都从 API 拉模型列表校验（失败拒绝，保持原模型）
        self.verify_model(&provider, &model).await?;
        let pipeline = self.pipeline_manager.ensure_pipeline(memory_key).await?;
        pipeline.set_model(provider, model).await
    }

    /// 设置来源 channel 所属 pipeline 的模型（每次切换都从 API 拉模型列表校验）
    pub async fn set_channel_pipeline_model(&self, channel_id: &str, provider: ArcStr, model: ArcStr) -> Result<()> {
        let Some(key) = self.channel_manager.memory_key(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id).into()));
        };
        self.set_pipeline_model(key, provider, model).await
    }

    /// 启动主循环（保持进程运行）：初始化 pipeline + 连接全部 channel
    pub async fn run(&self) {
        info!("Nexus 启动，等待外部输入...");
        // 按 channel 绑定的 agent/role/运行态事件装配 pipeline（agent_id 取 config，保留 agent = "0"）
        for (_, ch) in ConfigManager::get().channels().await {
            if let Some(key) = self.channel_manager.memory_key(ch.channel_id.as_str()).await {
                self.ensure_pipeline(key).await;
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
    /// 完整处理链：管理命令 → pipeline 定位 → 系统事件过滤 → 普通消息进 pipeline
    /// （上行记忆写入由 pipeline 自己完成：default_memory 收到即写，abstract_memory 在 locate 后写）
    pub async fn incoming_message(&self, channel_id: ArcStr, event: Arc<IncomingMessageEvent>) {
        // 0. 先取 channel 配置（命令分支 admin 判断与普通分支 pipeline 定位复用，避免重复查询）
        let Some(ch) = ConfigManager::get().channel(channel_id.as_str()).await else { return; };

        // 1. 管理命令：仅 Content::Text 且以 "/" 开头才判断（不 trim——前置空格可跳过命令检查）；
        //    命令与命令回复不找 memory_key、不入记忆（管理命令独立处理）
        if let Content::Text(text) = &event.incoming_message.content {
            if text.starts_with('/') {
                // 2. 判断是否管理员（admins HashSet O(1) contains，ch 已保存）；非管理员忽略不回复
                let is_admin = ch.admins.contains(&ChannelUser {
                    messenger_id: event.incoming_message.messenger_id.clone(),
                    user_id: event.incoming_message.user_id.clone(),
                });
                if is_admin {
                    // 3. 命令解析+执行（内联在 CommandRouter::execute）；回复始终发回来源 channel（不走 out_channel）
                    match CommandRouter::execute(text.as_str(), channel_id.clone()).await {
                        Ok(reply) => self.send_admin_reply(channel_id.as_str(), event, reply).await,
                        Err(Error::InvalidCommand(msg)) => {
                            self.send_admin_reply(channel_id.as_str(), event, ArcStr::from(format!("⚠️ {}", msg))).await;
                        }
                        Err(e) => {
                            self.send_admin_reply(channel_id.as_str(), event, ArcStr::from(format!("❌ 命令执行失败: {}", e))).await;
                        }
                    }
                }
                // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
                return;
            }
        }

        // 2. pipeline 定位（config agent_id/role_name + 运行态事件；channel 已存在，不重复查询）
        let Some(key) = self.channel_manager.memory_key(channel_id.as_str()).await else {
            return;
        };

        // 3. 系统事件（群组变更/用户移除）不进 agentic loop
        match &event.incoming_message.content {
            Content::GroupJoin(_) | Content::GroupLeave(_) | Content::UserRemove(_) => return,
            _ => {}
        }

        // 4. 普通消息：运行 pipeline（记忆写入由各 pipeline 自行完成；pipeline 内部按需创建 session）
        self.pipeline_manager.incoming_message(key, event).await;
    }

    /// 系统命令回复：始终发回来源 channel（不走 out_channel）
    /// 命令与命令回复不入记忆（管理命令不产生 session 上下文）：不找 memory_key、不推 ChannelRecord
    /// 身份：messenger_id = incoming.messenger_id；user_id/self_user_id = event.recipient_user_id（接收方即发声身份，且是群成员）
    async fn send_admin_reply(&self, channel_id: &str, event: Arc<IncomingMessageEvent>, content: ArcStr) {
        let msg = OutgoingMessage {
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.recipient_user_id.clone(),
            group_id: event.incoming_message.group_id.clone(),
            content: Content::Text(content),
        };

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        if let Err(e) = self.channel_manager.send(channel_id, msg).await {
            warn!("send_admin_reply 失败: {:?}", e);
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::time::Instant;

    use kissbot_api::RoleEvent;
    use kissbot_api::channel::IncomingMessage;
    use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, ModelConfig, ProviderConfig, ProviderModelConfig, Session};

    use crate::configs::{ChannelBatchConfig, ChannelConfig, PipelineConfig};
    use crate::types::{ChannelCommand, MemoryKey};

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
            abstract_client: Arc::new(AbstractClient::new()),
            provider_manager: Arc::new(ProviderManager::new()),
            pipeline_manager: Arc::new(PipelineManager::new()),
            channel_manager: ChannelManager::new(),
        }
    }

    #[tokio::test]
    async fn verify_agent_exists_reserved_or_empty_passes() {
        let dir = tempfile::tempdir().unwrap();
        let nexus = test_nexus(&dir).await;
        // 保留 id "0" 与空串直接 Ok，提前返回不触 ego HTTP
        assert!(nexus.verify_agent_exists(ArcStr::from("0")).await.is_ok());
        assert!(nexus.verify_agent_exists(ArcStr::new()).await.is_ok());
    }

    #[tokio::test]
    async fn verify_role_exists_empty_passes() {
        let dir = tempfile::tempdir().unwrap();
        let nexus = test_nexus(&dir).await;
        // 显式空串（保留 role）直接 Ok，提前返回不触 ego HTTP；
        // 非空分支依赖 ego 服务（memory_ego_url 为空返回 Err），暂不测
        assert!(nexus.verify_role_exists(ArcStr::from("a1"), ArcStr::new()).await.is_ok());
    }

    // ===== 消息入口链路（incoming_message → 合批 → pipeline → session 上下文） =====
    // 覆盖一整条链：ensure_pipeline（含 pipeline 装配、由 pipeline 创建 session 与系统提示词初始化）/
    // pipeline 分发 / 合批 flush / context_append

    /// 进程级装配（幂等）：ConfigManager/Station/Nexus 单例各注册一次。
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
        let key = MemoryKey { agent_id: ArcStr::from(agent_id), role_event: Arc::new(RoleEvent::new(ArcStr::from(""))) };
        let session_id = key.encode();

        // 1. agent 级（role 空串）pipeline：显式指定 pipeline 名 default，
        //    必须在 pipeline 首次装配前写入。default 不依赖 agent/role，配置直接写 session
        let pipeline = PipelineConfig { pipeline_name: Some(ArcStr::from("default")) };
        cm.set_agent_role_config::<PipelineConfig>(ArcStr::from(agent_id), ArcStr::from(""), Arc::new(pipeline)).await.unwrap();
        // provider/model 已配置但 provider_type 无法映射 → 发送器解析到模型配置、写入上下文后，
        // 模型调用返回 Err（不触网），不影响本用例断言上下文已增长
        let provider = "chain-test-provider";
        let model = "chain-test-model";
        cm.add_provider(provider, ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("typo"),
                base_url: ArcStr::from("http://127.0.0.1:1"),
                api_key: ArcStr::from("k"),
            }),
            default_model_config: ModelConfig { max_tokens_usage: 0, timeout_secs: None, retry_count: None },
            model_configs: Arc::new(HashMap::new()),
        }).await.unwrap();
        cm.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            session_id.clone(),
            &LLMConfig { provider: Some(ArcStr::from(provider)), model: Some(ArcStr::from(model)), ..Default::default() },
        ).await.unwrap();
        // 合批间隔缩到 1 秒，让 flush 尽快发生
        cm.set_session_config::<ChannelBatchConfig, ChannelBatchConfig>(
            session_id.clone(), &ChannelBatchConfig { channel_batch_interval_secs: 1 }).await.unwrap();

        // 2. channel 绑定 (a1, "")；channel_id 用本用例专用名，避免与其他测试互相干扰
        let _ = cm.add_channel(ChannelConfig {
            channel_id: ArcStr::from(channel_id),
            ws_url: ArcStr::from("ws://127.0.0.1:8399"),
            admins: Arc::new(HashSet::new()),
            bind_users: Arc::new(HashSet::new()),
            agent_id: ArcStr::from(agent_id),
            role_name: ArcStr::from(""),
            enabled: true,
        }).await;

        // 3. 消息入口（非命令消息，直接进 pipeline）：首次 ensure_pipeline 时装配 pipeline、
        //    由 pipeline 创建 session 并初始化系统提示词，随后消息分发到合批输入处理器
        //    pipeline 定位与运行态事件都取自 ChannelManager 的 channel 运行态条目，先登记该 channel
        Nexus::get().channel_manager().get_or_create(channel_id);
        Nexus::get().incoming_message(ArcStr::from(channel_id), Arc::new(IncomingMessageEvent {
            recipient_user_id: ArcStr::from("self"),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: ArcStr::from("m1"),
                messenger_id: ArcStr::from("web"),
                user_id: ArcStr::from("u1"),
                group_id: ArcStr::from("g1"),
                messenger_name: ArcStr::from(""),
                user_name: ArcStr::from("u1"),
                group_name: ArcStr::from(""),
                content: Content::Text(ArcStr::from("链路测试消息")),
                time: ArcStr::from("2026-08-07 10:00:00"),
            }),
        })).await;

        // 4. 等合批 flush + pipeline 运行：session 上下文应含系统消息（首条，由系统提示词初始化设置）
        //    与本条用户消息
        // 只读回读本地缓存检查 pipeline 写入的上下文（session 由 pipeline 持有，测试不共享其实例）
        let session = Session::new(ConfigManager::get().data_dir(), key.encode());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let _ = session.recover_from_cache().await;
            let ctx = session.build_context_full().await;
            let has_system = matches!(ctx.first(), Some(Message::System { .. }));
            let has_user = ctx.iter().any(|m| matches!(m, Message::User { content } if content.as_str().contains("链路测试消息")));
            if has_system && has_user {
                break;
            }
            assert!(Instant::now() < deadline,
                "消息应经 incoming_message → 合批 → pipeline 进入 session 上下文，实际: {:?}", ctx);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn channel_command_runs_through_channel_manager_queue() {
        // 覆盖变更队列链路：ChannelManager::channel_command 入队 → 队列消费者
        // → ChannelManager::apply_channel_command → bind_user/unbind_user 落库
        // 用本地 ChannelManager（队列消费者属本用例运行时）：全局单例的消费者由首个用例的运行时 spawn，
        // 该用例结束后消费者随运行时停止，跨用例使用全局队列会因队列关闭而失败
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let channel_manager = ChannelManager::new();
        let channel_id = "nexus-queue-test";
        let _ = cm.add_channel(ChannelConfig {
            channel_id: ArcStr::from(channel_id),
            ws_url: ArcStr::from("ws://127.0.0.1:8398"),
            admins: Arc::new(HashSet::new()),
            bind_users: Arc::new(HashSet::new()),
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from(""),
            enabled: true,
        }).await;

        let user = ChannelUser { messenger_id: ArcStr::from("web"), user_id: ArcStr::from("u-queue") };
        let reply = channel_manager.channel_command(ChannelCommand::BindUser {
            channel_id: ArcStr::from(channel_id), user: user.clone(),
        }).await.expect("绑定命令应执行成功");
        assert!(reply.contains("已绑定"), "回复文案应说明已绑定: {}", reply);
        let ch = cm.channel(channel_id).await.expect("channel 应存在");
        assert!(ch.bind_users.contains(&user), "绑定应落库");

        let reply = channel_manager.channel_command(ChannelCommand::UnbindUser {
            channel_id: ArcStr::from(channel_id), user: user.clone(),
        }).await.expect("解绑命令应执行成功");
        assert!(reply.contains("已移除"), "回复文案应说明已移除: {}", reply);
        let ch = cm.channel(channel_id).await.expect("channel 应存在");
        assert!(!ch.bind_users.contains(&user), "解绑应落库");
    }

    #[tokio::test]
    async fn pipeline_retain_drops_unbound_pipelines() {
        // pipeline 被清理后其 session 随 pipeline 的 Arc 释放自然回收；
        // 用本地 PipelineManager 验证 retain 语义，避免动到全局单例
        ensure_test_globals().await;
        let agent_id = "prune-agent";
        let k1 = Arc::new(MemoryKey { agent_id: ArcStr::from(agent_id), role_event: Arc::new(RoleEvent::new(ArcStr::from(""))) });
        let k2 = Arc::new(MemoryKey { agent_id: ArcStr::from(agent_id), role_event: Arc::new(RoleEvent::with_event(ArcStr::from(""), ArcStr::from("e1"))) });
        // agent 级未配置 pipeline_name → 默认 default_memory

        let pm = PipelineManager::new();
        pm.ensure_pipeline(k1.clone()).await.unwrap_or_else(|e| panic!("k1 应能注册: {:?}", e));
        pm.ensure_pipeline(k2.clone()).await.unwrap_or_else(|e| panic!("k2 应能注册: {:?}", e));
        assert!(pm.contains(k1.as_ref()), "注册后 k1 应有 pipeline");
        assert!(pm.contains(k2.as_ref()), "注册后 k2 应有 pipeline");

        // 只保留 k1：k2 的 pipeline 被丢弃
        pm.retain(&HashSet::from([k1.clone()]));
        assert!(pm.contains(k1.as_ref()), "保留的 key 仍在");
        assert!(!pm.contains(k2.as_ref()), "被清理的 key 的 pipeline 应移除");

        // 重新装配即可恢复（pipeline 会重新创建自己的 session）
        pm.ensure_pipeline(k2.clone()).await.unwrap_or_else(|e| panic!("清理后可重新注册: {:?}", e));
        assert!(pm.contains(k2.as_ref()), "重新注册后 k2 恢复");
    }

    // ===== provider 分派（未知 provider 名 / 未知 provider_type） =====
    // 分派与工厂去重由 ProviderManager 承担，其单测见 provider.rs；此处覆盖 Nexus 的委托链路

    #[tokio::test]
    async fn provider_manager_call_delegates_unknown_provider_to_error() {
        ensure_test_globals().await;
        // provider 未配置 → ProviderManager 解析有效模型配置失败 → ModelProviderNotFound（不触网）
        let mut llm_cfg = EffectiveLLMConfig::new();
        llm_cfg.provider = ArcStr::from("no-such-provider");
        llm_cfg.model = ArcStr::from("m");
        let err = Nexus::get().provider_manager.call(&llm_cfg, vec![], &vec![]).await
            .err().expect("未配置的 provider 应返回 Err");
        assert!(matches!(err, Error::ModelProviderNotFound(_, _)), "未配置的 provider: {:?}", err);
    }

    #[tokio::test]
    async fn model_command_validates_arguments() {
        ensure_test_globals().await;
        // 参数段固定为 provider + model：段数不符返回格式错误（不访问不存在的参数段）
        for content in ["/model", "/model deepseek"] {
            let err = CommandRouter::execute(content, ArcStr::from("no-such-channel")).await.unwrap_err();
            assert!(matches!(err, Error::InvalidCommand(_)), "{} 应返回格式错误: {:?}", content, err);
        }
        // 参数齐备 → 进入模型切换流程（channel 不存在 → ConfigNotFound；未知 provider 在模型校验处返回错误，不触网）
        let err = CommandRouter::execute("/model deepseek deepseek-chat", ArcStr::from("no-such-channel")).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)), "参数齐备应进入切换流程: {:?}", err);
    }

    #[tokio::test]
    async fn pipeline_name_selects_implementation_and_rejects_unknown() {
        // pipeline_name 选择 pipeline 执行流程：显式 default / default_memory 均可装配，
        // 未配置时取 default_memory，未知名称返回 PipelineNotFound
        ensure_test_globals().await;
        let pm = PipelineManager::new();

        // 1. 显式 default 可装配
        let agent_default = "pipeline-default-agent";
        ConfigManager::get().set_agent_role_config::<PipelineConfig>(
            ArcStr::from(agent_default), ArcStr::from(""),
            Arc::new(PipelineConfig { pipeline_name: Some(ArcStr::from("default")) })).await.unwrap();
        let key_default = Arc::new(MemoryKey {
            agent_id: ArcStr::from(agent_default), role_event: Arc::new(RoleEvent::new(ArcStr::from(""))) });
        pm.ensure_pipeline(key_default.clone()).await.unwrap_or_else(|e| panic!("显式 default 应能装配 pipeline: {:?}", e));
        assert!(pm.contains(key_default.as_ref()), "显式 default 应已注册");

        // 2. 显式 default_memory 可装配
        let agent_memory = "pipeline-memory-agent";
        ConfigManager::get().set_agent_role_config::<PipelineConfig>(
            ArcStr::from(agent_memory), ArcStr::from(""),
            Arc::new(PipelineConfig { pipeline_name: Some(ArcStr::from("default_memory")) })).await.unwrap();
        let key_memory = Arc::new(MemoryKey {
            agent_id: ArcStr::from(agent_memory), role_event: Arc::new(RoleEvent::new(ArcStr::from(""))) });
        pm.ensure_pipeline(key_memory.clone()).await.unwrap_or_else(|e| panic!("显式 default_memory 应能装配 pipeline: {:?}", e));
        assert!(pm.contains(key_memory.as_ref()), "显式 default_memory 应已注册");

        // 3. 未配置名称 → 默认 default_memory
        let agent_fallback = "pipeline-fallback-agent";
        let key_fallback = Arc::new(MemoryKey {
            agent_id: ArcStr::from(agent_fallback), role_event: Arc::new(RoleEvent::new(ArcStr::from(""))) });
        pm.ensure_pipeline(key_fallback.clone()).await.unwrap_or_else(|e| panic!("未配置名称应回退默认 pipeline: {:?}", e));
        assert!(pm.contains(key_fallback.as_ref()), "默认 pipeline 应已注册");

        // 4. 未知名称 → PipelineNotFound
        let agent_unknown = "pipeline-unknown-agent";
        ConfigManager::get().set_agent_role_config::<PipelineConfig>(
            ArcStr::from(agent_unknown), ArcStr::from(""),
            Arc::new(PipelineConfig { pipeline_name: Some(ArcStr::from("no-such-pipeline")) })).await.unwrap();
        let key_unknown = Arc::new(MemoryKey {
            agent_id: ArcStr::from(agent_unknown), role_event: Arc::new(RoleEvent::new(ArcStr::from(""))) });
        let err = match pm.ensure_pipeline(key_unknown).await {
            Ok(_) => panic!("未知 pipeline 名应返回 Err"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::PipelineNotFound(_, _)), "未知 pipeline 名应返回 PipelineNotFound: {:?}", err);
    }
}
