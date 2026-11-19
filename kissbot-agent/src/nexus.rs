use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tracing::{info, warn};

use crate::config_manager::ConfigManager;
use crate::configs::InputChannelsConfig;
use crate::memory_summary_client::MemorySummaryClient;
use crate::channel_manager::ChannelManager;
use crate::memory_ego_client::MemoryEgoClient;
use crate::memory_store_client::MemoryStoreClient;
use crate::pipelines::{is_supported_pipeline_name, PipelineManager};
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

    /// 启动主循环（保持进程运行）：按顶层 pipelines 配置装配全部 pipeline 并绑定 input channels
    pub async fn run(&self) {
        info!("Nexus 启动，装配全部 pipeline...");
        // 只在启动时装配一次：pipeline 集合由 nexus.json 的顶层 pipelines 决定，
        // 每条 pipeline 的 input channels 决定需要哪些 channel 连接、绑定哪些身份
        let cfg = ConfigManager::get();
        for (key, config) in cfg.pipelines().await {
            let pipeline_name = config.pipeline_name.clone();
            if !is_supported_pipeline_name(pipeline_name.as_str()) {
                warn!("未知 pipeline 名称，跳过: {} ({}/{})", pipeline_name, key.agent_id, key.role_name);
                continue;
            }
            if let Err(e) = self.pipeline_manager.ensure_pipeline(key.clone(), pipeline_name).await {
                warn!("pipeline 装配失败: {:?}", e);
                continue;
            }
            let inputs = cfg.agent_role_config::<InputChannelsConfig>(key.agent_id.as_str(), key.role_name.as_str()).await
                .map(|config| config.input_channels)
                .unwrap_or_default();
            for input in inputs.iter() {
                self.channel_manager.bind_input(input.clone(), key.clone()).await;
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
    use kissbot_api::channel::{IncomingMessage, IncomingMessageEvent};
    use kissbot_api::message::Content;
    use kissbot_channel_client::Terminal;
    use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, ModelConfig, ProviderConfig, ProviderModelConfig, Session};

    use crate::config_manager::ConfigManager;
    use crate::configs::{ChannelBatchConfig, ChannelConfig, CompressConfig, EffectiveCompressConfig, EffectiveMemoryPipelineConfig, InputChannel, MemoryPipelineConfig};
    use crate::pipeline::{AgentPipeline, IncomingMessageBatchProcessor};
    use crate::pipelines::{outgoing_channel, pipeline_session_id, topic_session_id, SESSION_PREFIX_DEFAULT, SESSION_PREFIX_DEFAULT_MEMORY, MemorySummaryPipeline, DefaultAgenticLoopPipeline, DefaultAgenticLoopPipelineWithMemory};
    use crate::types::{Error, MemoryKey};

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

    /// 以管理员身份向 pipeline 投递一条命令，返回命令回复（普通消息与不认识的命令返回 None，按普通消息处理）
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

    /// 进程级装配（幂等）：ConfigManager/Station/Nexus 单例各注册一次（与其它测试模块共用同一套装配）
    async fn ensure_test_globals() {
        crate::test_support::init().await;
    }

    #[tokio::test]
    async fn incoming_message_grows_session_context_through_pipeline() {
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let agent_id = "a1";
        let channel_id = "nexus-chain-test";
        let key = memory_key(agent_id, "");
        let session_id = pipeline_session_id(SESSION_PREFIX_DEFAULT, &key);

        // 1. provider/model 已配置但 provider_type 无法映射 → 发送器解析到模型配置、写入上下文后，
        //    模型调用返回 Err（不触网），不影响本用例断言上下文已增长
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

        // 2. channel 连接配置（enabled=false：测试不建立真实连接，路由仍登记）；channel_id 用本用例专用名
        let _ = cm.add_channel(ChannelConfig {
            channel_id: ArcStr::from(channel_id),
            ws_url: ArcStr::from("ws://127.0.0.1:8399"),
            admins: Arc::new(HashSet::new()),
            enabled: false,
        }).await;

        // 3. 装配 default pipeline 并登记输入身份路由（对应 Nexus::run 的启动装配动作）
        let role = role_key(agent_id, "");
        Nexus::get().pipeline_manager.ensure_pipeline(role.clone(), ArcStr::from("default")).await.unwrap();
        Nexus::get().channel_manager.bind_input(Arc::new(InputChannel {
            channel_id: ArcStr::from(channel_id),
            messenger_id: ArcStr::from("web"),
            user_id: ArcStr::from("self"),
        }), role).await;

        // 4. 消息入口（非命令消息）：按 input channel 路由到已装配 pipeline，
        //    消息经合批输入处理器进入 pipeline 的 session（channel 未启用，无连接运行态，回显判定不命中）
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

        // 5. 等合批 flush + pipeline 运行：session 上下文应含系统消息（首条，由系统提示词初始化设置）
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
    async fn input_channel_binds_only_first_pipeline() {
        // 同一 (channel_id, messenger_id, user_id) 只能绑定一条 pipeline：先绑定者生效，后续跳过
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let channel_id = "nexus-route-test";
        let _ = cm.add_channel(ChannelConfig {
            channel_id: ArcStr::from(channel_id),
            ws_url: ArcStr::from("ws://127.0.0.1:8398"),
            admins: Arc::new(HashSet::new()),
            enabled: false,
        }).await;

        let channel_manager = ChannelManager::new();
        let input = Arc::new(InputChannel {
            channel_id: ArcStr::from(channel_id),
            messenger_id: ArcStr::from("web"),
            user_id: ArcStr::from("u-route"),
        });
        let k1 = role_key("route-agent", "");
        let k2 = role_key("route-agent", "r2");
        channel_manager.bind_input(input.clone(), k1.clone()).await;
        channel_manager.bind_input(input.clone(), k2.clone()).await;

        assert_eq!(
            channel_manager.input_pipeline(
                ArcStr::from(channel_id), ArcStr::from("web"), ArcStr::from("u-route")),
            Some(k1.clone()),
            "第一个 pipeline 绑定成功，后续 pipeline 跳过");
        assert!(channel_manager.input_pipeline(
            ArcStr::from(channel_id), ArcStr::from("web"), ArcStr::from("u-route")).is_some());
        assert!(channel_manager.input_pipeline(
            ArcStr::from(channel_id), ArcStr::from("web"), ArcStr::from("other")).is_none());
    }

    #[tokio::test]
    async fn pipeline_incoming_message_parses_own_commands() {
        ensure_test_globals().await;
        let channel_id = ArcStr::from("cmd-detect-channel");

        // default：接 /model 与 /mode；不认识的命令按普通消息处理（None）
        let default = DefaultAgenticLoopPipeline::new(role_key("cmd-detect-default", "")).await;
        assert!(command(default.clone(), channel_id.clone(), "/bind-outgoing web u1 g1").await.is_none(),
            "default 不接 outgoing 命令，按普通消息处理");
        assert!(command(default.clone(), channel_id.clone(), "普通消息").await.is_none(),
            "非命令消息应进入 pipeline");
        match command(default.clone(), channel_id.clone(), "/model deepseek deepseek-chat maybe").await {
            Some(Err(Error::InvalidCommand(msg))) => assert!(msg.contains("/model <provider> <model>"), "参数错误文案: {}", msg),
            other => panic!("default 应识别 /model 并返回参数错误: {:?}", other),
        }
        let reply = command(default.clone(), channel_id.clone(), "/mode event e-detect").await
            .expect("default 应识别 /mode").unwrap();
        assert_eq!(reply.as_str(), "✅ 新事件 ID: e-detect");

        // default_memory / memory_summary：接 /model、/mode 与 outgoing；不认识的命令按普通消息处理（None）
        let key = memory_key("cmd-detect-agent", "");
        // 预置话题 session：pipeline 构造时加载最新话题 session，命令写入该 session
        let role_session_id = topic_session_id(SESSION_PREFIX_DEFAULT_MEMORY, &key);
        let role_session = Session::new(ConfigManager::get().context_dir(), role_session_id.clone());
        role_session.context_append(vec![Message::User { content: ArcStr::from("已有上下文") }]).await.unwrap();
        let memory = DefaultAgenticLoopPipelineWithMemory::new(role_key("cmd-detect-agent", "")).await;
        assert!(command(memory.clone(), channel_id.clone(), "/send-channel on").await.is_none(),
            "default_memory 对不认识的命令按普通消息处理");
        assert!(command(memory.clone(), channel_id.clone(), "普通消息").await.is_none(),
            "非命令消息应进入 pipeline");
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
            "memory_summary 对不认识的命令按普通消息处理");
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
        // pipeline 直接构造（不依赖顶层 pipelines 配置），当前 event 由 /mode 切换
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
        let _ = cm.add_channel(ChannelConfig {
            channel_id: channel_id.clone(),
            ws_url: ArcStr::from("ws://127.0.0.1:8397"),
            admins: Arc::new(HashSet::new()),
            enabled: false,
        }).await;
        // pipeline 的输入身份 (channel, web, u-out)：/bind-outgoing 以此校验
        Nexus::get().channel_manager.bind_input(
            Arc::new(InputChannel {
                channel_id: channel_id.clone(),
                messenger_id: ArcStr::from("web"),
                user_id: ArcStr::from("u-out"),
            }),
            role_key("cmd-outgoing-agent", ""),
        ).await;
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
    async fn ensure_pipeline_selects_implementation_and_rejects_unknown() {
        // pipeline_name 为必填，选择 pipeline 执行流程；未知名称返回 PipelineNotFound
        ensure_test_globals().await;
        let pm = PipelineManager::new();

        let k_default = role_key("ensure-default-agent", "");
        pm.ensure_pipeline(k_default.clone(), ArcStr::from("default")).await.unwrap();
        assert!(pm.contains(k_default.as_ref()), "default 应已装配");

        let k_memory = role_key("ensure-memory-agent", "");
        pm.ensure_pipeline(k_memory.clone(), ArcStr::from("default_memory")).await.unwrap();
        assert!(pm.contains(k_memory.as_ref()), "default_memory 应已装配");

        // 同一 RoleKey 再次 ensure：复用已装配 pipeline
        pm.ensure_pipeline(k_default.clone(), ArcStr::from("default")).await.unwrap();
        assert!(pm.contains(k_default.as_ref()));

        let err = match pm.ensure_pipeline(role_key("ensure-unknown-agent", ""), ArcStr::from("no-such-pipeline")).await {
            Ok(_) => panic!("未知名称应返回 Err"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::PipelineNotFound(_, _)), "未知名称应返回 PipelineNotFound: {:?}", err);
    }

    #[tokio::test]
    async fn runtime_pipeline_add_change_and_remove() {
        // 运行期单条 pipeline 增删与 input channel 变化入口（启动装配不使用）
        ensure_test_globals().await;
        let cm = ConfigManager::get();
        let channel_id = "runtime-change-channel";
        let _ = cm.add_channel(ChannelConfig {
            channel_id: ArcStr::from(channel_id),
            ws_url: ArcStr::from("ws://127.0.0.1:8389"),
            admins: Arc::new(HashSet::new()),
            enabled: false,
        }).await;

        let pm = PipelineManager::new();
        let key = role_key("runtime-change-agent", "");
        pm.ensure_pipeline(key.clone(), ArcStr::from("default_memory")).await.unwrap();
        assert!(pm.contains(key.as_ref()));

        let input1 = Arc::new(InputChannel {
            channel_id: ArcStr::from(channel_id),
            messenger_id: ArcStr::from("web"),
            user_id: ArcStr::from("u1"),
        });
        let input2 = Arc::new(InputChannel {
            channel_id: ArcStr::from(channel_id),
            messenger_id: ArcStr::from("web"),
            user_id: ArcStr::from("u2"),
        });
        pm.set_input_channels(&key, &[input1]).await;
        assert_eq!(
            Nexus::get().channel_manager.input_pipeline(
                ArcStr::from(channel_id), ArcStr::from("web"), ArcStr::from("u1")),
            Some(key.clone()));

        // 替换输入身份：旧身份解绑，新身份绑定
        pm.set_input_channels(&key, &[input2]).await;
        assert_eq!(
            Nexus::get().channel_manager.input_pipeline(
                ArcStr::from(channel_id), ArcStr::from("web"), ArcStr::from("u1")),
            None);
        assert_eq!(
            Nexus::get().channel_manager.input_pipeline(
                ArcStr::from(channel_id), ArcStr::from("web"), ArcStr::from("u2")),
            Some(key.clone()));

        // 删除单条 pipeline：输入身份一并解绑
        pm.remove_pipeline(&key).await;
        assert!(!pm.contains(key.as_ref()));
        assert_eq!(
            Nexus::get().channel_manager.input_pipeline(
                ArcStr::from(channel_id), ArcStr::from("web"), ArcStr::from("u2")),
            None);
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
