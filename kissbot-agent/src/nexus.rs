use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tracing::{info, warn};

use crate::memory_summary_client::MemorySummaryClient;
use crate::channel_manager::ChannelManager;
use crate::config_manager::ConfigManager;
use crate::memory_ego_client::MemoryEgoClient;
use crate::memory_store_client::MemoryStoreClient;
use crate::pipelines::PipelineManager;
use crate::provider::ProviderManager;
use crate::types::Result;

// 上下文压缩/回溯阈值来自当前 session 模型 effective.max_tokens_usage（provider/model 配置合成）：
// 最近一次模型响应的 usage.total_tokens 超过其 compress_threshold（默认 0.8）倍时触发，
// 见 pipelines.rs 的 send_messages_with_limit 与 DefaultAgenticLoopPipelineWithMemory。

/// Nexus 全局单例（进程内唯一；new() 完成时注册，此后 get() 可用）。
/// 所有使用 Nexus 的位置一律不传参数、从单例获取（Session/Channel 不保存引用）。
static SINGLETON: OnceLock<Nexus> = OnceLock::new();

/// 运行时组件集合（进程级单例）：消息入口在 channel_manager，各组件经 get() 取用
pub struct Nexus {
    pub memory_store_client: Arc<MemoryStoreClient>,
    pub memory_ego_client: Arc<MemoryEgoClient>,
    pub memory_summary_client: Arc<MemorySummaryClient>,
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
        let memory_summary_client = Arc::new(MemorySummaryClient::new());
        let provider_manager = Arc::new(ProviderManager::new());
        let pipeline_manager = Arc::new(PipelineManager::new());
        // channel 运行态与 channel 变更串行队列（队列消费者在 ChannelManager::new 内启动）
        let channel_manager = ChannelManager::new();

        let nexus = Self {
            memory_store_client,
            memory_ego_client,
            memory_summary_client,
            provider_manager,
            pipeline_manager,
            channel_manager,
        };

        // 注册全局单例（此后 get() 可用；run() 中启动动作与连接回调均晚于此）
        let _ = SINGLETON.set(nexus);

        info!("Nexus 初始化完成");
        Ok(())
    }

    /// 启动主循环（保持进程运行）：初始化 pipeline + 连接全部 channel
    pub async fn run(&self) {
        info!("Nexus 启动，等待外部输入...");
        // 按 channel 绑定的 agent/role 装配 pipeline（当前 event 由 pipeline 自己维护，新建时从角色模式开始）
        for (_, ch) in ConfigManager::get().channels().await {
            if let Some(key) = self.channel_manager.pipeline_key(ch.channel_id.as_str()).await {
                if let Err(e) = self.pipeline_manager.ensure_pipeline(key).await {
                    warn!("pipeline 装配失败: {:?}", e);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use axum::{Json, Router, extract::State, routing::post};

    use arcstr::ArcStr;
    use kissbot_api::{ChannelKey, RoleKey};
    use kissbot_api::channel::{ChannelUser, IncomingMessage, IncomingMessageEvent};
    use kissbot_api::message::Content;
    use kissbot_channel_client::Terminal;
    use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, ModelConfig, ProviderConfig, ProviderModelConfig, Session};

    use crate::configs::{ChannelBatchConfig, ChannelConfig, CompressConfig, EffectiveCompressConfig, EffectiveMemoryPipelineConfig, MemoryPipelineConfig, PipelineConfig};
    use crate::pipeline::{AgentPipeline, IncomingMessageBatchProcessor};
    use crate::pipelines::{outgoing_channel, pipeline_session_id, topic_session_id, SESSION_PREFIX_DEFAULT, SESSION_PREFIX_DEFAULT_MEMORY, MemorySummaryPipeline, DefaultAgenticLoopPipeline, DefaultAgenticLoopPipelineWithMemory};
    use crate::types::{ChannelCommand, Error, MemoryKey};

    /// 测试用角色键（pipeline 身份）
    fn role_key(agent_id: &str, role_name: &str) -> Arc<RoleKey> {
        Arc::new(RoleKey { agent_id: ArcStr::from(agent_id), role_name: ArcStr::from(role_name) })
    }

    /// 测试用记忆范围键（角色模式）
    fn memory_key(agent_id: &str, role_name: &str) -> Arc<MemoryKey> {
        MemoryKey::from_role_key(&role_key(agent_id, role_name), None)
    }

    /// 测试用记忆范围键（事件模式）
    fn event_memory_key(agent_id: &str, role_name: &str, event_id: &str) -> Arc<MemoryKey> {
        MemoryKey::from_role_key(&role_key(agent_id, role_name), Some(ArcStr::from(event_id)))
    }

    /// 测试用来源通道三元组（命令的 channel 参数）
    fn command_channel() -> Arc<ChannelKey> {
        Arc::new(ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1")))
    }

    /// 构造一条管理员命令事件：通道三元组与 command_channel 保持一致
    fn command_event(content: &str) -> Arc<IncomingMessageEvent> {
        let channel = command_channel();
        Arc::new(IncomingMessageEvent {
            recipient_user_id: channel.self_user_id.clone(),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: ArcStr::from("cmd"),
                messenger_id: channel.messenger_id.clone(),
                user_id: ArcStr::from("admin"),
                group_id: channel.group_id.clone(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::from("admin"),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from(content)),
                time: ArcStr::from("2026-11-11 10:00:00"),
            }),
        })
    }

    /// 以管理员身份向 pipeline 投递一条命令，返回命令回复（普通消息与未知命令返回 None）
    async fn command<P: AgentPipeline + 'static>(
        pipeline: Arc<P>, channel_id: ArcStr, content: &str,
    ) -> Option<Result<ArcStr>> {
        pipeline.incoming_message(command_event(content), channel_id, true).await
    }

    /// 等待 pipeline 后台任务把 session 缓存写成期望状态（任务队列是异步消费的）
    async fn wait_for_context<F>(session_id: ArcStr, mut predicate: F, message: &str)
    where
        F: FnMut(&[Message]) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let session = Session::new(ConfigManager::get().context_dir(), session_id.clone());
            let _ = session.recover_from_cache().await;
            let ctx = session.build_context_full().await;
            if predicate(&ctx) {
                return;
            }
            assert!(Instant::now() < deadline, "{}: {:?}", message, ctx);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// 等待 pipeline 后台任务完成指定次数的模型调用
    async fn wait_for_calls(calls: Arc<AtomicUsize>, expected: usize, message: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while calls.load(Ordering::Relaxed) < expected {
            assert!(Instant::now() < deadline, "{}: 实际 {}", message, calls.load(Ordering::Relaxed));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(calls.load(Ordering::Relaxed), expected, "{}", message);
    }

    /// 在 context 目录中查找前缀匹配且不是 exclude 的消息缓存文件，返回其 session_id
    /// （话题切换后的新 session_id 自动生成，测试通过缓存文件名反查）
    fn find_new_session_id(prefix: &str, exclude: &str) -> Option<ArcStr> {
        let dir = std::path::Path::new(ConfigManager::get().context_dir());
        std::fs::read_dir(dir).ok()?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                let id = name.strip_suffix(".msgs.jsonl")?;
                (id != exclude && id.starts_with(prefix)).then(|| ArcStr::from(id))
            })
            .next()
    }

    /// 等待话题切换后新 session 的缓存出现，返回新 session_id
    async fn wait_for_new_session_id(prefix: &str, exclude: &str) -> ArcStr {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(id) = find_new_session_id(prefix, exclude) {
                return id;
            }
            assert!(Instant::now() < deadline, "未等到新 session 的缓存文件: {}", prefix);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// 读取 session 的完整上下文（含系统提示词）
    async fn read_context(session_id: ArcStr) -> Vec<Message> {
        let session = Session::new(ConfigManager::get().context_dir(), session_id);
        let _ = session.recover_from_cache().await;
        session.build_context_full().await
    }

    /// 设置 session 消息缓存的修改时间，用于构造“最新 session”场景
    fn set_msgs_mtime(session_id: &ArcStr, modified: std::time::SystemTime) {
        let path = std::path::Path::new(ConfigManager::get().context_dir())
            .join(format!("{}.msgs.jsonl", session_id));
        let file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        file.set_modified(modified).unwrap();
    }

    // ===== 消息入口链路（incoming_message → 合批 → pipeline → session 上下文） =====
    // 覆盖一整条链：pipeline 装配（由 pipeline 创建 session 与系统提示词初始化）/
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
        let key = memory_key(agent_id, "");
        let session_id = pipeline_session_id(SESSION_PREFIX_DEFAULT, &key);

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

        // 3. 消息入口（非命令消息，直接进 pipeline）：首次处理消息时装配 pipeline、
        //    由 pipeline 创建 session 并初始化系统提示词，随后消息分发到合批输入处理器
        //    pipeline 按 channel 的 agent/role 定位，先登记该 channel 的运行态条目
        Nexus::get().channel_manager.get_or_create(channel_id);
        Nexus::get().channel_manager.incoming_message(ArcStr::from(channel_id), Arc::new(IncomingMessageEvent {
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
        let session = Session::new(ConfigManager::get().context_dir(), session_id);
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
        // pipeline 身份只看 agent + role，不同 role 各自一条 pipeline
        let k1 = role_key(agent_id, "");
        let k2 = role_key(agent_id, "r2");
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

    #[tokio::test]
    async fn pipeline_incoming_message_parses_own_commands() {
        ensure_test_globals().await;
        let channel_id = ArcStr::from("cmd-detect-channel");

        // default：接 /model 与 /mode，不接 outgoing
        let default = DefaultAgenticLoopPipeline::new(role_key("cmd-detect-default", "")).await;
        assert!(command(default.clone(), channel_id.clone(), "/bind-outgoing web u1 g1").await.is_none(),
            "default 不接 outgoing 命令");
        match command(default.clone(), channel_id.clone(), "/model deepseek deepseek-chat maybe").await {
            Some(Err(Error::InvalidCommand(msg))) => assert!(msg.contains("/model <provider> <model>"), "参数错误文案: {}", msg),
            other => panic!("default 应识别 /model 并返回参数错误: {:?}", other),
        }
        let reply = command(default.clone(), channel_id.clone(), "/mode event e-detect").await
            .expect("default 应识别 /mode").unwrap();
        assert_eq!(reply.as_str(), "✅ 新事件 ID: e-detect");

        // default_memory / memory_summary：接 /model、/mode 与 outgoing；不是自己的命令返回 None
        let key = memory_key("cmd-detect-agent", "");
        // 预置话题 session：pipeline 构造时加载最新话题 session，命令写入该 session
        let role_session_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let role_session = Session::new(ConfigManager::get().context_dir(), role_session_id.clone());
        role_session.context_append(vec![Message::User { content: ArcStr::from("已有上下文") }]).await.unwrap();
        let memory = DefaultAgenticLoopPipelineWithMemory::new(role_key("cmd-detect-agent", "")).await;
        assert!(command(memory.clone(), channel_id.clone(), "/send-channel on").await.is_none(),
            "未知命令应返回 None");
        match command(memory.clone(), channel_id.clone(), "/bind-outgoing web u1").await {
            Some(Err(Error::InvalidCommand(_))) => {}
            other => panic!("default_memory 应识别 /bind-outgoing 并返回参数错误: {:?}", other),
        }
        match command(memory.clone(), channel_id.clone(), "/load-memory maybe").await {
            Some(Err(Error::InvalidCommand(_))) => {}
            other => panic!("default_memory 应识别 /load-memory 并返回参数错误: {:?}", other),
        }
        let reply = command(memory.clone(), channel_id.clone(), "/load-memory off").await
            .expect("应识别 /load-memory").unwrap();
        assert!(reply.contains("load_memory"), "回复文案: {}", reply);
        let cfg = ConfigManager::get()
            .session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(role_session_id.clone()).await;
        assert!(!cfg.load_memory, "/load-memory off 应写入 session 配置");

        // 切到事件模式后再设 load_memory：只写当前 event 的 session，角色模式 session 保持原值
        command(memory.clone(), channel_id.clone(), "/mode event e-load").await
            .expect("default_memory 应识别 /mode").unwrap();
        command(memory.clone(), channel_id.clone(), "/load-memory on").await
            .expect("应识别 /load-memory").unwrap();
        let event_id = pipeline_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &event_memory_key("cmd-detect-agent", "", "e-load"));
        let event_cfg = ConfigManager::get()
            .session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(event_id).await;
        assert!(event_cfg.load_memory, "/load-memory 应写入当前 event session");
        let role_cfg = ConfigManager::get()
            .session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(role_session_id.clone()).await;
        assert!(!role_cfg.load_memory, "角色模式 session 不应被当前 event 的命令改动");

        let reply = command(memory.clone(), channel_id.clone(), "/mode role").await
            .expect("default_memory 应识别 /mode").unwrap();
        assert_eq!(reply.as_str(), "✅ 已切换为角色模式");

        let memory_summary = MemorySummaryPipeline::new(role_key("cmd-detect-agent", ""));
        assert!(command(memory_summary.clone(), channel_id.clone(), "/send-channel on").await.is_none(),
            "未知命令应返回 None");
        match command(memory_summary.clone(), channel_id.clone(), "/unbind-outgoing extra").await {
            Some(Err(Error::InvalidCommand(_))) => {}
            other => panic!("memory_summary 应识别 /unbind-outgoing 并返回参数错误: {:?}", other),
        }
        let reply = command(memory_summary.clone(), channel_id.clone(), "/mode event e-summary").await
            .expect("memory_summary 应识别 /mode").unwrap();
        assert_eq!(reply.as_str(), "✅ 新事件 ID: e-summary");
        match command(memory_summary.clone(), channel_id.clone(), "/mode maybe").await {
            Some(Err(Error::InvalidCommand(msg))) => assert!(msg.contains("未知模式"), "参数错误文案: {}", msg),
            other => panic!("memory_summary 的 /mode 参数错误应返回 InvalidCommand: {:?}", other),
        }
    }

    #[tokio::test]
    async fn role_mode_without_saved_session_starts_new_topic_session() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();

        let calls = Arc::new(AtomicUsize::new(0));
        let base_url = start_mock_same_topic_chat(calls.clone()).await;
        let provider = "first-topic-provider";
        let model = "first-topic-model";
        cm.add_provider(provider, ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from(base_url),
                api_key: ArcStr::from("k"),
            }),
            default_model_config: ModelConfig { max_tokens_usage: 0, timeout_secs: None, retry_count: None },
            model_configs: Arc::new(HashMap::new()),
        }).await.unwrap();
        let agent = "a-first-topic";
        let role = "r-first-topic";
        // 没有历史 session：新话题 session 的配置由 agent/role 导入
        cm.set_agent_role_config::<LLMConfig>(
            ArcStr::from(agent), ArcStr::from(""), Arc::new(LLMConfig {
                provider: Some(ArcStr::from(provider)),
                model: Some(ArcStr::from(model)),
                ..Default::default()
            })).await.unwrap();

        let key = memory_key(agent, role);
        let base_id = pipeline_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(role_key(agent, role)).await;
        pipeline.incoming_message_batch(vec![incoming_event("first-topic-m1", "第一句话")]).await;

        let topic_id = wait_for_new_session_id(base_id.as_str(), base_id.as_str()).await;
        assert!(topic_id.starts_with(&format!("{}-topic-", base_id)),
            "无可加载 session 时应新建话题 session: {}", topic_id);
        wait_for_context(topic_id.clone(), |ctx| {
            ctx.iter().any(|m| matches!(m, Message::User { content } if content.contains("第一句话")))
                && ctx.iter().any(|m| matches!(m, Message::Assistant { content, .. } if content.as_str() == Some("新回复")))
        }, "新话题 session 应写入首轮对话").await;
        assert!(!std::path::Path::new(cm.context_dir())
            .join(format!("{}.msgs.jsonl", base_id)).exists(),
            "不应再使用无话题的 role 模式 session_id");
        let llm = cm.session_config::<LLMConfig, EffectiveLLMConfig>(topic_id).await;
        assert_eq!(llm.provider.as_str(), provider, "新话题 session 应从 agent/role 导入配置");
        assert_eq!(calls.load(Ordering::Relaxed), 1, "无历史上下文时跳过同话题判定，只有一次正式运行");
    }

    #[tokio::test]
    async fn mode_switch_uses_per_event_session_and_restores_role_session() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let agent = "mode-session-agent";
        let role = "mode-session-role";
        let channel_id = ArcStr::from("mode-session-channel");
        // 显式 default（无记忆）：pipeline 只由 agent + role 定位，当前 event 由 /mode 切换
        cm.set_agent_role_config::<PipelineConfig>(
            ArcStr::from(agent), ArcStr::from(""),
            Arc::new(PipelineConfig { pipeline_name: Some(ArcStr::from("default")) })).await.unwrap();
        let pipeline = DefaultAgenticLoopPipeline::new(role_key(agent, role)).await;

        // 角色模式 session（pipeline 新建时创建）：先写入一条消息，模拟已存在的上下文
        let role_session_id = pipeline_session_id(SESSION_PREFIX_DEFAULT, &memory_key(agent, role));
        let role_session = Session::new(cm.context_dir(), role_session_id.clone());
        role_session.context_append(vec![Message::User { content: ArcStr::from("角色会话") }]).await.unwrap();

        // 切到 e1：新建 e1 的 session，与角色模式上下文隔离
        let reply = command(pipeline.clone(), channel_id.clone(), "/mode event e1").await
            .expect("应识别 /mode").unwrap();
        assert_eq!(reply.as_str(), "✅ 新事件 ID: e1");
        let e1_session_id = pipeline_session_id(SESSION_PREFIX_DEFAULT, &event_memory_key(agent, role, "e1"));
        assert_ne!(e1_session_id, role_session_id);
        let e1 = Session::new(cm.context_dir(), e1_session_id.clone());
        e1.recover_from_cache().await.unwrap();
        assert!(e1.build_context().await.is_empty(), "新事件 session 不应带角色模式上下文");
        e1.context_append(vec![Message::User { content: ArcStr::from("事件会话") }]).await.unwrap();

        // 切到 e2：又一个新 session
        command(pipeline.clone(), channel_id.clone(), "/mode event e2").await
            .expect("应识别 /mode").unwrap();
        let e2_id = pipeline_session_id(SESSION_PREFIX_DEFAULT, &event_memory_key(agent, role, "e2"));
        let e2 = Session::new(cm.context_dir(), e2_id);
        e2.recover_from_cache().await.unwrap();
        assert!(e2.build_context().await.is_empty(), "另一个新事件 session 也应隔离");

        // 回到角色模式：重新从本地缓存恢复角色模式 session
        let reply = command(pipeline.clone(), channel_id, "/mode role").await
            .expect("应识别 /mode").unwrap();
        assert_eq!(reply.as_str(), "✅ 已切换为角色模式");
        // 角色 session 由 pipeline 重新创建后从缓存恢复；这里直接读缓存确认消息仍在
        let restored = Session::new(cm.context_dir(), role_session_id);
        restored.recover_from_cache().await.unwrap();
        assert!(
            restored.build_context().await.iter().any(|m| matches!(m, Message::User { content } if content.as_str() == "角色会话")),
            "切回角色模式应恢复角色 session 的缓存");
    }

    #[tokio::test]
    async fn outgoing_command_writes_pipeline_session_config() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let channel_id = ArcStr::from("cmd-outgoing-channel");
        let user = ChannelUser { messenger_id: ArcStr::from("web"), user_id: ArcStr::from("u-out") };
        let _ = cm.add_channel(ChannelConfig {
            channel_id: channel_id.clone(),
            ws_url: ArcStr::from("ws://127.0.0.1:8397"),
            admins: Arc::new(HashSet::new()),
            bind_users: Arc::new(HashSet::from([Arc::new(user)])),
            agent_id: ArcStr::from("cmd-outgoing-agent"),
            role_name: ArcStr::from(""),
            enabled: true,
        }).await;
        let key = memory_key("cmd-outgoing-agent", "");
        let session_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        // 预置话题 session：pipeline 构造时加载最新话题 session，命令写入该 session
        let role_session = Session::new(cm.context_dir(), session_id.clone());
        role_session.context_append(vec![Message::User { content: ArcStr::from("已有上下文") }]).await.unwrap();
        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(role_key("cmd-outgoing-agent", "")).await;

        // 身份未绑定 → 拒绝
        let err = command(pipeline.clone(), channel_id.clone(), "/bind-outgoing web nobody g1").await
            .expect("应识别 /bind-outgoing").unwrap_err();
        assert!(matches!(err, Error::InvalidCommand(_)), "未绑定身份应拒绝: {:?}", err);

        // 绑定成功 → 写入当前 pipeline 的 session 配置
        let reply = command(pipeline.clone(), channel_id.clone(), "/bind-outgoing web u-out g1").await
            .expect("应识别 /bind-outgoing").unwrap();
        assert!(reply.contains("已设发送通道"), "回复文案: {}", reply);
        let out = outgoing_channel(&session_id).await.expect("out_channel 应已写入 session");
        assert_eq!(out.channel_id, channel_id);
        assert_eq!(out.user.user_id.as_str(), "u-out");
        assert_eq!(out.group_id.as_str(), "g1");

        // 解除 → 清空
        command(pipeline.clone(), channel_id.clone(), "/unbind-outgoing").await
            .expect("应识别 /unbind-outgoing").unwrap();
        assert!(outgoing_channel(&session_id).await.is_none(), "解除后为空");
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
        let key_default = role_key(agent_default, "");
        pm.ensure_pipeline(key_default.clone()).await.unwrap_or_else(|e| panic!("显式 default 应能装配 pipeline: {:?}", e));
        assert!(pm.contains(key_default.as_ref()), "显式 default 应已注册");

        // 2. 显式 default_memory 可装配
        let agent_memory = "pipeline-memory-agent";
        ConfigManager::get().set_agent_role_config::<PipelineConfig>(
            ArcStr::from(agent_memory), ArcStr::from(""),
            Arc::new(PipelineConfig { pipeline_name: Some(ArcStr::from("default_memory")) })).await.unwrap();
        let key_memory = role_key(agent_memory, "");
        pm.ensure_pipeline(key_memory.clone()).await.unwrap_or_else(|e| panic!("显式 default_memory 应能装配 pipeline: {:?}", e));
        assert!(pm.contains(key_memory.as_ref()), "显式 default_memory 应已注册");

        // 3. 未配置名称 → 默认 default_memory
        let agent_fallback = "pipeline-fallback-agent";
        let key_fallback = role_key(agent_fallback, "");
        pm.ensure_pipeline(key_fallback.clone()).await.unwrap_or_else(|e| panic!("未配置名称应回退默认 pipeline: {:?}", e));
        assert!(pm.contains(key_fallback.as_ref()), "默认 pipeline 应已注册");

        // 4. 未知名称 → PipelineNotFound
        let agent_unknown = "pipeline-unknown-agent";
        ConfigManager::get().set_agent_role_config::<PipelineConfig>(
            ArcStr::from(agent_unknown), ArcStr::from(""),
            Arc::new(PipelineConfig { pipeline_name: Some(ArcStr::from("no-such-pipeline")) })).await.unwrap();
        let key_unknown = role_key(agent_unknown, "");
        let err = match pm.ensure_pipeline(key_unknown).await {
            Ok(_) => panic!("未知 pipeline 名应返回 Err"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::PipelineNotFound(_, _)), "未知 pipeline 名应返回 PipelineNotFound: {:?}", err);
    }
    // ===== /pipeline、/agent-pipeline：修改 pipeline 配置并让受影响的已装配 pipeline 失效 =====

    /// 构造测试用 channel 配置（仅填命令所需字段）
    fn pipeline_test_channel(channel_id: &str, agent_id: &str, role_name: &str, ws_port: u16) -> ChannelConfig {
        ChannelConfig {
            channel_id: ArcStr::from(channel_id),
            ws_url: ArcStr::from(format!("ws://127.0.0.1:{}", ws_port)),
            admins: Arc::new(HashSet::new()),
            bind_users: Arc::new(HashSet::new()),
            agent_id: ArcStr::from(agent_id),
            role_name: ArcStr::from(role_name),
            enabled: true,
        }
    }

    #[tokio::test]
    async fn pipeline_commands_write_agent_and_role_config() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let channel_manager = Nexus::get().channel_manager.clone();

        // /pipeline：写来源 channel 的 (agent, role) 级配置
        let role_channel = ArcStr::from("cmd-pipeline-role");
        let _ = cm.add_channel(pipeline_test_channel(role_channel.as_str(), "cmd-pipeline-agent", "r1", 8396)).await;
        let reply = channel_manager.execute_default_command("/pipeline memory_summary", role_channel).await
            .expect("应命中 /pipeline").expect("应有回复");
        assert!(reply.contains("已设置 pipeline"), "回复文案: {}", reply);
        let cfg = cm.agent_role_config::<PipelineConfig>("cmd-pipeline-agent", "r1").await.expect("role 配置应存在");
        assert_eq!(cfg.pipeline_name.as_deref(), Some("memory_summary"));

        // /agent-pipeline：写来源 channel 的 agent（role 空）级配置
        let agent_channel = ArcStr::from("cmd-agent-pipeline");
        let _ = cm.add_channel(pipeline_test_channel(agent_channel.as_str(), "cmd-agent-pipeline-agent", "r2", 8395)).await;
        let reply = channel_manager.execute_default_command("/agent-pipeline default", agent_channel).await
            .expect("应命中 /agent-pipeline").expect("应有回复");
        assert!(reply.contains("agent pipeline"), "回复文案: {}", reply);
        let cfg = cm.agent_role_config::<PipelineConfig>("cmd-agent-pipeline-agent", "").await.expect("agent 配置应存在");
        assert_eq!(cfg.pipeline_name.as_deref(), Some("default"));
    }

    #[tokio::test]
    async fn pipeline_commands_reject_default_identity_and_unknown_name() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let channel_manager = Nexus::get().channel_manager.clone();

        // 默认 role（空）不可设置
        let default_role_channel = ArcStr::from("cmd-pipeline-default-role");
        let _ = cm.add_channel(pipeline_test_channel(default_role_channel.as_str(), "cmd-pipeline-default-agent", "", 8394)).await;
        let err = channel_manager.execute_default_command("/pipeline default", default_role_channel).await.unwrap_err();
        assert!(matches!(err, Error::InvalidCommand(ref msg) if msg.contains("默认 role")), "默认 role 应拒绝: {:?}", err);

        // 默认 agent（"0"）不可设置
        let default_agent_channel = ArcStr::from("cmd-pipeline-default-agent");
        let _ = cm.add_channel(pipeline_test_channel(default_agent_channel.as_str(), "0", "r9", 8393)).await;
        let err = channel_manager.execute_default_command("/agent-pipeline default", default_agent_channel).await.unwrap_err();
        assert!(matches!(err, Error::InvalidCommand(ref msg) if msg.contains("默认 agent")), "默认 agent 应拒绝: {:?}", err);

        // 未知 pipeline 名称
        let unknown_channel = ArcStr::from("cmd-pipeline-unknown");
        let _ = cm.add_channel(pipeline_test_channel(unknown_channel.as_str(), "cmd-pipeline-unknown-agent", "r1", 8392)).await;
        let err = channel_manager.execute_default_command("/pipeline no-such-pipeline", unknown_channel).await.unwrap_err();
        assert!(matches!(err, Error::InvalidCommand(ref msg) if msg.contains("未知 pipeline")), "未知名称应拒绝: {:?}", err);
    }

    #[tokio::test]
    async fn pipeline_config_change_invalidates_affected_pipelines() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let agent = "cmd-pipe-inval-agent";
        // agent 级默认 default_memory；r2 用 role 覆盖 default
        cm.set_agent_role_config::<PipelineConfig>(
            ArcStr::from(agent), ArcStr::from(""),
            Arc::new(PipelineConfig { pipeline_name: Some(ArcStr::from("default_memory")) })).await.unwrap();
        cm.set_agent_role_config::<PipelineConfig>(
            ArcStr::from(agent), ArcStr::from("r2"),
            Arc::new(PipelineConfig { pipeline_name: Some(ArcStr::from("default")) })).await.unwrap();

        let pm = PipelineManager::new();
        // pipeline 身份只看 agent + role：r1、r2、r3 各一条
        let k1 = role_key(agent, "r1");
        let k2 = role_key(agent, "r2");
        let k3 = role_key(agent, "r3");
        for key in [&k1, &k2, &k3] {
            pm.ensure_pipeline(key.clone()).await.unwrap_or_else(|e| panic!("应能装配 pipeline: {:?}", e));
        }
        assert!(pm.contains(k1.as_ref()) && pm.contains(k2.as_ref()) && pm.contains(k3.as_ref()));

        // 改 agent 级为 default：r1、r3 合并值变化（default_memory → default）失效；r2 的 role 覆盖不变，保留
        pm.set_pipeline_config(ArcStr::from(agent), ArcStr::new(), ArcStr::from("default")).await.unwrap();
        assert!(!pm.contains(k1.as_ref()), "r1 合并配置变化，pipeline 应失效");
        assert!(!pm.contains(k3.as_ref()), "r3 合并配置变化，pipeline 应失效");
        assert!(pm.contains(k2.as_ref()), "r2 合并配置未变，pipeline 保留");

        // 改 r2 的 role 级为 memory_summary：合并值变化，r2 pipeline 失效
        pm.set_pipeline_config(ArcStr::from(agent), ArcStr::from("r2"), ArcStr::from("memory_summary")).await.unwrap();
        assert!(!pm.contains(k2.as_ref()), "r2 合并配置变化，pipeline 应失效");
    }

    fn incoming_event(msg_id: &str, text: &str) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: ArcStr::from("self"),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: ArcStr::from(msg_id),
                messenger_id: ArcStr::from("web"),
                user_id: ArcStr::from("u1"),
                group_id: ArcStr::from("g1"),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::from("u1"),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from(text)),
                time: ArcStr::from("2026-11-04 10:00:00"),
            }),
        })
    }

    /// 同话题判定请求返回 NO，正常对话/压缩请求返回固定回复；total_tokens 固定 100 便于触发阈值
    async fn mock_topic_chat(
        State(calls): State<Arc<AtomicUsize>>,
        body: String,
    ) -> Json<serde_json::Value> {
        calls.fetch_add(1, Ordering::Relaxed);
        let is_topic_check = body.contains("只回答 YES 或 NO") || body.contains("必须且只能回答");
        let content = if is_topic_check { "NO" } else { "新回复" };
        Json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"total_tokens": 100}
        }))
    }

    /// 启动本地 mock OpenAI 服务并返回 base_url
    async fn start_mock_chat(calls: Arc<AtomicUsize>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/chat/completions", post(mock_topic_chat))
            .with_state(calls);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base_url
    }

    /// 同话题判定请求返回 YES（同一件事），正式运行返回固定回复
    async fn mock_same_topic_chat(
        State(calls): State<Arc<AtomicUsize>>,
        body: String,
    ) -> Json<serde_json::Value> {
        calls.fetch_add(1, Ordering::Relaxed);
        let is_topic_check = body.contains("只回答 YES 或 NO") || body.contains("必须且只能回答");
        let content = if is_topic_check { "YES" } else { "新回复" };
        Json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"total_tokens": 100}
        }))
    }

    async fn start_mock_same_topic_chat(calls: Arc<AtomicUsize>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/chat/completions", post(mock_same_topic_chat))
            .with_state(calls);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base_url
    }

    /// 判定请求返回带解释的 NO（非精确 YES/NO），正式运行返回固定回复
    async fn mock_non_exact_chat(
        State(calls): State<Arc<AtomicUsize>>,
        body: String,
    ) -> Json<serde_json::Value> {
        calls.fetch_add(1, Ordering::Relaxed);
        let is_topic_check = body.contains("YES 或 NO");
        let content = if is_topic_check { "NO, different topic" } else { "新回复" };
        Json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"total_tokens": 1}
        }))
    }

    async fn start_mock_non_exact_chat(calls: Arc<AtomicUsize>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/chat/completions", post(mock_non_exact_chat))
            .with_state(calls);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base_url
    }

    /// 第一次判定返回非精确的 MAYBE，重试提示词返回 NO，正式运行返回固定回复
    async fn mock_retry_chat(
        State(calls): State<Arc<AtomicUsize>>,
        body: String,
    ) -> Json<serde_json::Value> {
        calls.fetch_add(1, Ordering::Relaxed);
        let retry_prompt = body.contains("必须且只能回答");
        let first_prompt = body.contains("只回答 YES 或 NO") && !retry_prompt;
        let content = if retry_prompt {
            "NO"
        } else if first_prompt {
            "MAYBE"
        } else {
            "新回复"
        };
        Json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"total_tokens": 1}
        }))
    }

    async fn start_mock_retry_chat(calls: Arc<AtomicUsize>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/chat/completions", post(mock_retry_chat))
            .with_state(calls);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base_url
    }

    #[tokio::test]
    async fn role_mode_topic_change_switches_to_new_session_and_inherits_config() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();

        // 本地 mock OpenAI 服务：判定请求 → NO，正式运行 → 固定回复
        let calls = Arc::new(AtomicUsize::new(0));
        let base_url = start_mock_chat(calls.clone()).await;

        // 唯一 provider / session，避免与其他用例共享
        let provider = "topic-switch-provider";
        let model = "topic-switch-model";
        cm.add_provider(provider, ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from(base_url),
                api_key: ArcStr::from("k"),
            }),
            default_model_config: ModelConfig { max_tokens_usage: 0, timeout_secs: None, retry_count: None },
            model_configs: Arc::new(HashMap::new()),
        }).await.unwrap();
        let key = memory_key("a-topic-switch", "r-topic-switch");
        let base_id = pipeline_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let session_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        cm.set_session_config::<LLMConfig, EffectiveLLMConfig>(session_id.clone(), &LLMConfig {
            provider: Some(ArcStr::from(provider)),
            model: Some(ArcStr::from(model)),
            ..Default::default()
        }).await.unwrap();
        // 开启 load_memory：话题判定不再依赖该开关
        cm.set_session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(
            session_id.clone(),
            &MemoryPipelineConfig { load_memory: Some(true) },
        ).await.unwrap();
        cm.set_session_config::<CompressConfig, EffectiveCompressConfig>(
            session_id.clone(),
            &CompressConfig { compress_prompt: None, compress_threshold: Some(0.5) },
        ).await.unwrap();

        // 预置上一件事的上下文
        let old = Session::new(cm.context_dir(), session_id.clone());
        old.context_append(vec![
            Message::User { content: ArcStr::from("旧问题") },
            Message::Assistant {
                content: serde_json::json!("旧回复"),
                reasoning_content: serde_json::Value::Null,
                tool_calls: serde_json::Value::Null,
            },
        ]).await.unwrap();

        // 触发新一批消息：判定为不同 → 换用新 session_id 与空上下文运行
        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(role_key("a-topic-switch", "r-topic-switch")).await;
        pipeline.incoming_message_batch(vec![incoming_event("topic-m1", "新问题")]).await;

        let new_session_id = wait_for_new_session_id(base_id.as_str(), session_id.as_str()).await;
        assert_ne!(new_session_id, session_id, "话题切换应换新的 session_id");

        // 新 session 只包含本批新消息与本次回复
        wait_for_context(new_session_id.clone(), |ctx| {
            ctx.iter().all(|m| !matches!(m, Message::User { content } if content.as_str() == "旧问题"))
                && ctx.iter().any(|m| matches!(m, Message::User { content } if content.contains("新问题")))
                && ctx.iter().any(|m| matches!(m, Message::Assistant { content, .. } if content.as_str() == Some("新回复")))
        }, "新 session 应从空上下文开始且写入新对话").await;

        // 旧 session 不归档、不清空：缓存原样保留
        let old_reloaded = Session::new(cm.context_dir(), session_id.clone());
        old_reloaded.recover_from_cache().await.unwrap();
        assert!(old_reloaded.build_context_full().await.iter()
            .any(|m| matches!(m, Message::User { content } if content.as_str() == "旧问题")),
            "旧上下文应保留在原 session 缓存中");
        let versions_dir = std::path::Path::new(cm.context_dir()).join("versions");
        let archived = std::fs::read_dir(&versions_dir).map(|entries| entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(session_id.as_str()))
            .any(|entry| std::fs::read_to_string(entry.path()).map(|text| text.contains("旧问题")).unwrap_or(false)))
            .unwrap_or(false);
        assert!(!archived, "话题切换不保存版本");

        // 新 session 继承旧 session 的整份 session 级配置
        let llm = cm.session_config::<LLMConfig, EffectiveLLMConfig>(new_session_id.clone()).await;
        assert_eq!(llm.provider.as_str(), provider);
        assert_eq!(llm.model.as_str(), model);
        let memory = cm.session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(new_session_id.clone()).await;
        assert!(memory.load_memory, "load_memory 随 session 配置继承");
        let compress = cm.session_config::<CompressConfig, EffectiveCompressConfig>(new_session_id).await;
        assert_eq!(compress.compress_threshold, 0.5, "其它 session 配置同样继承");

        assert_eq!(calls.load(Ordering::Relaxed), 2, "一次话题判定 + 一次正式运行");
    }

    #[tokio::test]
    async fn role_mode_topic_check_non_exact_answer_keeps_context() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();

        let calls = Arc::new(AtomicUsize::new(0));
        let base_url = start_mock_non_exact_chat(calls.clone()).await;
        let provider = "topic-ambiguous-provider";
        let model = "topic-ambiguous-model";
        cm.add_provider(provider, ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from(base_url),
                api_key: ArcStr::from("k"),
            }),
            default_model_config: ModelConfig { max_tokens_usage: 0, timeout_secs: None, retry_count: None },
            model_configs: Arc::new(HashMap::new()),
        }).await.unwrap();
        let key = memory_key("a-topic-ambiguous", "r-topic-ambiguous");
        let base_id = pipeline_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let session_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        cm.set_session_config::<LLMConfig, EffectiveLLMConfig>(session_id.clone(), &LLMConfig {
            provider: Some(ArcStr::from(provider)),
            model: Some(ArcStr::from(model)),
            ..Default::default()
        }).await.unwrap();
        cm.set_session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(
            session_id.clone(),
            &MemoryPipelineConfig { load_memory: Some(true) },
        ).await.unwrap();
        let old = Session::new(cm.context_dir(), session_id.clone());
        old.context_append(vec![
            Message::User { content: ArcStr::from("旧问题") },
            Message::Assistant {
                content: serde_json::json!("旧回复"),
                reasoning_content: serde_json::Value::Null,
                tool_calls: serde_json::Value::Null,
            },
        ]).await.unwrap();

        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(role_key("a-topic-ambiguous", "r-topic-ambiguous")).await;
        pipeline.incoming_message_batch(vec![incoming_event("ambiguous-m1", "新问题")]).await;

        // 两次判定回答都不是精确的 YES/NO → 按同一件事处理，随后正式运行一次
        wait_for_context(session_id.clone(), |ctx| {
            ctx.iter().any(|m| matches!(m, Message::User { content } if content.as_str() == "旧问题"))
                && ctx.iter().any(|m| matches!(m, Message::Assistant { content, .. } if content.as_str() == Some("新回复")))
        }, "无法判定时按同一件事处理，旧上下文应保留且本次回复应写入").await;
        assert!(find_new_session_id(base_id.as_str(), session_id.as_str()).is_none(),
            "按同一件事处理时不应创建新 session");
        assert_eq!(calls.load(Ordering::Relaxed), 3, "首次判定 + 写死重试一次 + 正式运行");
    }

    #[tokio::test]
    async fn role_mode_topic_check_retries_once_with_retry_prompt() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();

        let calls = Arc::new(AtomicUsize::new(0));
        let base_url = start_mock_retry_chat(calls.clone()).await;
        let provider = "topic-retry-once-provider";
        let model = "topic-retry-once-model";
        cm.add_provider(provider, ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from(base_url),
                api_key: ArcStr::from("k"),
            }),
            default_model_config: ModelConfig { max_tokens_usage: 0, timeout_secs: None, retry_count: None },
            model_configs: Arc::new(HashMap::new()),
        }).await.unwrap();
        let key = memory_key("a-topic-retry-once", "r-topic-retry-once");
        let base_id = pipeline_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let session_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        cm.set_session_config::<LLMConfig, EffectiveLLMConfig>(session_id.clone(), &LLMConfig {
            provider: Some(ArcStr::from(provider)),
            model: Some(ArcStr::from(model)),
            ..Default::default()
        }).await.unwrap();
        cm.set_session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(
            session_id.clone(),
            &MemoryPipelineConfig { load_memory: Some(true) },
        ).await.unwrap();
        let old = Session::new(cm.context_dir(), session_id.clone());
        old.context_append(vec![
            Message::User { content: ArcStr::from("旧问题") },
            Message::Assistant {
                content: serde_json::json!("旧回复"),
                reasoning_content: serde_json::Value::Null,
                tool_calls: serde_json::Value::Null,
            },
        ]).await.unwrap();

        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(role_key("a-topic-retry-once", "r-topic-retry-once")).await;
        pipeline.incoming_message_batch(vec![incoming_event("retry-once-m1", "新问题")]).await;

        // 首次 MAYBE（非精确）→ 写死用 RETRY_PROMPT 重试一次 → NO（不同）→ 换新 session 运行
        let new_session_id = wait_for_new_session_id(base_id.as_str(), session_id.as_str()).await;
        wait_for_context(new_session_id, |ctx| {
            ctx.iter().all(|m| !matches!(m, Message::User { content } if content.as_str() == "旧问题"))
                && ctx.iter().any(|m| matches!(m, Message::Assistant { content, .. } if content.as_str() == Some("新回复")))
        }, "重试确认不同后应换新 session").await;
        // 旧上下文仍在原 session 缓存中
        let old_reloaded = Session::new(cm.context_dir(), session_id);
        old_reloaded.recover_from_cache().await.unwrap();
        assert!(old_reloaded.build_context_full().await.iter()
            .any(|m| matches!(m, Message::User { content } if content.as_str() == "旧问题")),
            "旧上下文应保留在原 session 缓存中");
        assert_eq!(calls.load(Ordering::Relaxed), 3, "首次判定 + 重试一次 + 正式运行");
    }

    #[tokio::test]
    async fn pipeline_rebuild_loads_latest_topic_session() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();

        let calls = Arc::new(AtomicUsize::new(0));
        let base_url = start_mock_same_topic_chat(calls.clone()).await;
        let provider = "resume-provider";
        let model = "resume-model";
        cm.add_provider(provider, ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from(base_url),
                api_key: ArcStr::from("k"),
            }),
            default_model_config: ModelConfig { max_tokens_usage: 0, timeout_secs: None, retry_count: None },
            model_configs: Arc::new(HashMap::new()),
        }).await.unwrap();

        let key = memory_key("a-resume", "r-resume");
        let older_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let newer_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let event_id = pipeline_session_id(
            SESSION_PREFIX_DEFAULT_MEMORY, &event_memory_key("a-resume", "r-resume", "e1"));
        for id in [&older_id, &newer_id] {
            cm.set_session_config::<LLMConfig, EffectiveLLMConfig>(id.clone(), &LLMConfig {
                provider: Some(ArcStr::from(provider)),
                model: Some(ArcStr::from(model)),
                ..Default::default()
            }).await.unwrap();
            cm.set_session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(
                id.clone(), &MemoryPipelineConfig { load_memory: Some(true) }).await.unwrap();
        }

        // 两个话题 session：较旧 + 最新；event session 修改时间最新但不参与
        for (id, text) in [(&older_id, "较旧话题"), (&newer_id, "最新话题"), (&event_id, "事件话题")] {
            let session = Session::new(cm.context_dir(), id.clone());
            session.context_append(vec![Message::User { content: ArcStr::from(text) }]).await.unwrap();
        }
        let now = std::time::SystemTime::now();
        set_msgs_mtime(&older_id, now - Duration::from_secs(30));
        set_msgs_mtime(&newer_id, now - Duration::from_secs(20));
        set_msgs_mtime(&event_id, now - Duration::from_secs(10));

        // 重建 pipeline：应扫描并加载最新话题 session，忽略 event session
        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(role_key("a-resume", "r-resume")).await;
        pipeline.incoming_message_batch(vec![incoming_event("resume-m1", "继续最新话题")]).await;

        wait_for_context(newer_id.clone(), |ctx| {
            ctx.iter().any(|m| matches!(m, Message::User { content } if content.contains("继续最新话题")))
                && ctx.iter().any(|m| matches!(m, Message::Assistant { content, .. } if content.as_str() == Some("新回复")))
        }, "重建后应加载最新话题 session 并写入新对话").await;

        assert!(read_context(older_id).await.iter()
            .all(|m| !matches!(m, Message::User { content } if content.contains("继续最新话题"))),
            "不应加载较旧的话题 session");
        assert!(read_context(event_id).await.iter()
            .all(|m| !matches!(m, Message::User { content } if content.contains("继续最新话题"))),
            "event session 不参与重启加载");
        assert_eq!(calls.load(Ordering::Relaxed), 2, "一次话题判定 + 一次正式运行");
    }

    #[tokio::test]
    async fn mode_role_resumes_latest_topic_session() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();

        let calls = Arc::new(AtomicUsize::new(0));
        let base_url = start_mock_same_topic_chat(calls.clone()).await;
        let provider = "role-resume-provider";
        let model = "role-resume-model";
        cm.add_provider(provider, ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from(base_url),
                api_key: ArcStr::from("k"),
            }),
            default_model_config: ModelConfig { max_tokens_usage: 0, timeout_secs: None, retry_count: None },
            model_configs: Arc::new(HashMap::new()),
        }).await.unwrap();

        let key = memory_key("a-role-resume", "r-role-resume");
        let older_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let newer_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        for id in [&older_id, &newer_id] {
            cm.set_session_config::<LLMConfig, EffectiveLLMConfig>(id.clone(), &LLMConfig {
                provider: Some(ArcStr::from(provider)),
                model: Some(ArcStr::from(model)),
                ..Default::default()
            }).await.unwrap();
            cm.set_session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(
                id.clone(), &MemoryPipelineConfig { load_memory: Some(true) }).await.unwrap();
        }
        for (id, text) in [(&older_id, "较旧话题"), (&newer_id, "最新话题")] {
            let session = Session::new(cm.context_dir(), id.clone());
            session.context_append(vec![Message::User { content: ArcStr::from(text) }]).await.unwrap();
        }
        let now = std::time::SystemTime::now();
        set_msgs_mtime(&older_id, now - Duration::from_secs(30));
        set_msgs_mtime(&newer_id, now - Duration::from_secs(20));

        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(role_key("a-role-resume", "r-role-resume")).await;
        // 切到事件模式再切回角色模式：应从本地缓存重新加载最新话题 session
        command(pipeline.clone(), ArcStr::from("role-resume-channel"), "/mode event e-role-resume").await
            .expect("应识别 /mode event").unwrap();
        command(pipeline.clone(), ArcStr::from("role-resume-channel"), "/mode role").await
            .expect("应识别 /mode role").unwrap();
        pipeline.incoming_message_batch(vec![incoming_event("role-resume-m1", "再次继续")]).await;

        wait_for_context(newer_id, |ctx| {
            ctx.iter().any(|m| matches!(m, Message::User { content } if content.contains("再次继续")))
                && ctx.iter().any(|m| matches!(m, Message::Assistant { content, .. } if content.as_str() == Some("新回复")))
        }, "/mode role 应加载最新话题 session").await;
        assert!(read_context(older_id).await.iter()
            .all(|m| !matches!(m, Message::User { content } if content.contains("再次继续"))),
            "不应回到较旧的话题 session");
        assert_eq!(calls.load(Ordering::Relaxed), 2, "一次话题判定 + 一次正式运行");
    }

    #[tokio::test]
    async fn event_mode_never_reads_memory_even_when_load_memory_enabled() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();

        let calls = Arc::new(AtomicUsize::new(0));
        let base_url = start_mock_chat(calls.clone()).await;
        let provider = "event-no-read-provider";
        let model = "event-no-read-model";
        cm.add_provider(provider, ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from(base_url),
                api_key: ArcStr::from("k"),
            }),
            // max_tokens_usage = 1：第一次响应的 100 tokens 必然超过阈值，触发降级处理
            default_model_config: ModelConfig { max_tokens_usage: 1, timeout_secs: None, retry_count: None },
            model_configs: Arc::new(HashMap::new()),
        }).await.unwrap();
        let key = event_memory_key("a-event-no-read", "r-event", "e1");
        let session_id = pipeline_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        cm.set_session_config::<LLMConfig, EffectiveLLMConfig>(session_id.clone(), &LLMConfig {
            provider: Some(ArcStr::from(provider)),
            model: Some(ArcStr::from(model)),
            ..Default::default()
        }).await.unwrap();
        // 故意开启 load_memory：事件模式应忽略它
        cm.set_session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(
            session_id,
            &MemoryPipelineConfig { load_memory: Some(true) },
        ).await.unwrap();

        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(role_key("a-event-no-read", "r-event")).await;
        // pipeline 新建时为角色模式，先切到事件模式：event session 使用本用例预置的配置
        command(pipeline.clone(), ArcStr::from("event-no-read-channel"), "/mode event e1").await
            .expect("应识别 /mode").unwrap();
        pipeline.clone().incoming_message_batch(vec![incoming_event("event-m1", "第一件事")]).await;
        pipeline.clone().incoming_message_batch(vec![incoming_event("event-m2", "第二件事")]).await;

        // 第一次：1 次正式调用；第二次超阈值：压缩 1 次 + 正式调用 1 次
        wait_for_calls(calls.clone(), 3, "事件模式应使用压缩（多一次压缩调用）而不是读取记忆").await;
    }
}
