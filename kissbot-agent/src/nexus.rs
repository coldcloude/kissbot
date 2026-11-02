use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tracing::{info, warn};

use crate::summary_client::SummaryClient;
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
    pub summary_client: Arc<SummaryClient>,
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
        let summary_client = Arc::new(SummaryClient::new());
        let provider_manager = Arc::new(ProviderManager::new());
        let pipeline_manager = Arc::new(PipelineManager::new());
        // channel 运行态与 channel 变更串行队列（队列消费者在 ChannelManager::new 内启动）
        let channel_manager = ChannelManager::new();

        let nexus = Self {
            memory_store_client,
            memory_ego_client,
            summary_client,
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
        // 按 channel 绑定的 agent/role/运行态事件装配 pipeline（agent_id 取 config，保留 agent = "0"）
        for (_, ch) in ConfigManager::get().channels().await {
            if let Some(key) = self.channel_manager.memory_key(ch.channel_id.as_str()).await {
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
    use std::time::Instant;

    use arcstr::ArcStr;
    use kissbot_api::RoleEvent;
    use kissbot_api::channel::{ChannelUser, IncomingMessage, IncomingMessageEvent};
    use kissbot_api::message::Content;
    use kissbot_channel_client::Terminal;
    use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, ModelConfig, ProviderConfig, ProviderModelConfig, Session};

    use crate::configs::{ChannelBatchConfig, ChannelConfig, PipelineConfig};
    use crate::pipeline::AgentPipeline;
    use crate::pipelines::{outgoing_channel, SummaryMemoryPipeline, DefaultAgenticLoopPipeline, DefaultAgenticLoopPipelineWithMemory};
    use crate::types::{ChannelCommand, Error, MemoryKey};

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

        // 3. 消息入口（非命令消息，直接进 pipeline）：首次处理消息时装配 pipeline、
        //    由 pipeline 创建 session 并初始化系统提示词，随后消息分发到合批输入处理器
        //    pipeline 定位与运行态事件都取自 ChannelManager 的 channel 运行态条目，先登记该 channel
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
    async fn pipeline_execute_command_parses_own_commands() {
        ensure_test_globals().await;
        let channel_id = ArcStr::from("cmd-detect-channel");
        let cmd = |content: &str| ArcStr::from(content);

        // default：只接 /model，不接 outgoing
        let default = DefaultAgenticLoopPipeline::new(ArcStr::from("cmd-detect-default")).await;
        assert!(default.execute_command(channel_id.clone(), cmd("/bind-outgoing web u1 g1")).await.is_none(),
            "default 不接 outgoing 命令");
        match default.execute_command(channel_id.clone(), cmd("/model deepseek deepseek-chat maybe")).await {
            Some(Err(Error::InvalidCommand(msg))) => assert!(msg.contains("/model <provider> <model>"), "参数错误文案: {}", msg),
            other => panic!("default 应识别 /model 并返回参数错误: {:?}", other),
        }

        // default_memory / summary_memory：接 /model 与 outgoing；不是自己的命令返回 None
        let key = Arc::new(MemoryKey {
            agent_id: ArcStr::from("cmd-detect-agent"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from(""))),
        });
        let memory = DefaultAgenticLoopPipelineWithMemory::new(key.clone(), true).await;
        assert!(memory.execute_command(channel_id.clone(), cmd("/send-channel on")).await.is_none(),
            "未知命令应返回 None");
        match memory.execute_command(channel_id.clone(), cmd("/bind-outgoing web u1")).await {
            Some(Err(Error::InvalidCommand(_))) => {}
            other => panic!("default_memory 应识别 /bind-outgoing 并返回参数错误: {:?}", other),
        }

        let summary_memory = SummaryMemoryPipeline::new(key);
        assert!(summary_memory.execute_command(channel_id.clone(), cmd("/send-channel on")).await.is_none(),
            "未知命令应返回 None");
        match summary_memory.execute_command(channel_id.clone(), cmd("/unbind-outgoing extra")).await {
            Some(Err(Error::InvalidCommand(_))) => {}
            other => panic!("summary_memory 应识别 /unbind-outgoing 并返回参数错误: {:?}", other),
        }
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
        let key = Arc::new(MemoryKey {
            agent_id: ArcStr::from("cmd-outgoing-agent"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from(""))),
        });
        let pipeline = DefaultAgenticLoopPipelineWithMemory::new(key.clone(), true).await;
        let session_id = key.encode();

        // 身份未绑定 → 拒绝
        let err = pipeline.execute_command(channel_id.clone(), ArcStr::from("/bind-outgoing web nobody g1")).await
            .expect("应识别 /bind-outgoing").unwrap_err();
        assert!(matches!(err, Error::InvalidCommand(_)), "未绑定身份应拒绝: {:?}", err);

        // 绑定成功 → 写入当前 pipeline 的 session 配置
        let reply = pipeline.execute_command(channel_id.clone(), ArcStr::from("/bind-outgoing web u-out g1")).await
            .expect("应识别 /bind-outgoing").unwrap();
        assert!(reply.contains("已设发送通道"), "回复文案: {}", reply);
        let out = outgoing_channel(&session_id).await.expect("out_channel 应已写入 session");
        assert_eq!(out.channel_id, channel_id);
        assert_eq!(out.user.user_id.as_str(), "u-out");
        assert_eq!(out.group_id.as_str(), "g1");

        // 解除 → 清空
        pipeline.execute_command(channel_id.clone(), ArcStr::from("/unbind-outgoing")).await
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
        let reply = channel_manager.execute_default_command("/pipeline summary_memory", role_channel).await
            .expect("应命中 /pipeline").expect("应有回复");
        assert!(reply.contains("已设置 pipeline"), "回复文案: {}", reply);
        let cfg = cm.agent_role_config::<PipelineConfig>("cmd-pipeline-agent", "r1").await.expect("role 配置应存在");
        assert_eq!(cfg.pipeline_name.as_deref(), Some("summary_memory"));

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
        let k1 = Arc::new(MemoryKey { agent_id: ArcStr::from(agent), role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))) });
        let k1e = Arc::new(MemoryKey { agent_id: ArcStr::from(agent), role_event: Arc::new(RoleEvent::with_event(ArcStr::from("r1"), ArcStr::from("e1"))) });
        let k2 = Arc::new(MemoryKey { agent_id: ArcStr::from(agent), role_event: Arc::new(RoleEvent::new(ArcStr::from("r2"))) });
        for key in [&k1, &k1e, &k2] {
            pm.ensure_pipeline(key.clone()).await.unwrap_or_else(|e| panic!("应能装配 pipeline: {:?}", e));
        }
        assert!(pm.contains(k1.as_ref()) && pm.contains(k1e.as_ref()) && pm.contains(k2.as_ref()));

        // 改 agent 级为 default：r1 合并值变化（default_memory → default）失效；r2 的 role 覆盖不变，保留
        pm.set_pipeline_config(ArcStr::from(agent), ArcStr::new(), ArcStr::from("default")).await.unwrap();
        assert!(!pm.contains(k1.as_ref()), "r1 合并配置变化，pipeline 应失效");
        assert!(!pm.contains(k1e.as_ref()), "r1 的事件 pipeline 同样失效");
        assert!(pm.contains(k2.as_ref()), "r2 合并配置未变，pipeline 保留");

        // 改 r2 的 role 级为 summary_memory：合并值变化，r2 pipeline 失效
        pm.set_pipeline_config(ArcStr::from(agent), ArcStr::from("r2"), ArcStr::from("summary_memory")).await.unwrap();
        assert!(!pm.contains(k2.as_ref()), "r2 合并配置变化，pipeline 应失效");
    }
}
