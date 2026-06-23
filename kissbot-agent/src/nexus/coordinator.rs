use std::sync::Arc;

use chrono::Local;
use flume::Receiver;
use tracing::{info, warn};

use crate::nexus::types::{
    Mode, WriteTask, ContextMessage, AdminCommand, Result,
};
use crate::nexus::config_manager::ConfigManager;
use crate::nexus::mode_manager::ModeManager;
use crate::nexus::command_router::CommandRouter;
use crate::nexus::llm_client::LlmClient;
use crate::nexus::context_builder::ContextBuilder;
use crate::nexus::memory_reader::MemoryReader;
use crate::nexus::memory_writer::MemoryWriter;
use crate::nexus::ws_client::{ExternalMessage, WSClient};

pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    mode_manager: Arc<ModeManager>,
    memory_reader: Arc<MemoryReader>,
    memory_writer: Arc<MemoryWriter>,
    context_builder: Arc<tokio::sync::Mutex<ContextBuilder>>,
    llm_client: Arc<tokio::sync::Mutex<LlmClient>>,
    /// 从 WSClient 接收上行消息
    external_rx: Receiver<ExternalMessage>,
    /// WSClient 用于发送回复
    ws_client: Arc<WSClient>,
}

impl AgentCoordinator {
    pub async fn new(
        config: Arc<ConfigManager>,
        external_rx: Receiver<ExternalMessage>,
        ws_client: Arc<WSClient>,
        memory_writer: MemoryWriter,
    ) -> Result<Self> {
        let mode = config.current_mode().await;
        let mode_manager = Arc::new(ModeManager::new(mode.clone()));
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_writer = Arc::new(memory_writer);

        // 初始化 LLMClient
        let llm_config = config.llm_config().await;
        let llm_client = Arc::new(tokio::sync::Mutex::new(LlmClient::new(llm_config)));

        // 初始化 ContextBuilder
        let mut context_builder = ContextBuilder::new();

        // 读取自我认知
        if let Ok(ego_info) = Self::load_ego_info(&config).await {
            context_builder.set_system_message(ego_info);
        }

        // 读取历史记忆
        if let Ok(history) = memory_reader.read_history(&config, &mode).await {
            context_builder.load_history(history);
        }

        info!("AgentCoordinator 初始化完成，当前模式: {:?}", mode);

        Ok(Self {
            config,
            mode_manager,
            memory_reader,
            memory_writer,
            context_builder: Arc::new(tokio::sync::Mutex::new(context_builder)),
            llm_client,
            external_rx,
            ws_client,
        })
    }

    /// 启动主循环
    pub async fn run(&self) {
        info!("AgentCoordinator 启动，等待外部输入...");

        loop {
            let msg = self.external_rx.recv_async().await;
            match msg {
                Ok(ExternalMessage::Incoming(incoming)) => {
                    self.handle_incoming(incoming).await;
                }
                Err(e) => {
                    warn!("接收外部消息失败: {:?}，退出主循环", e);
                    break;
                }
            }
        }
    }

    async fn handle_incoming(&self, incoming: kissbot_api::channel::IncomingMessage) {
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let content = incoming.content.to_string();
        let _time = incoming.time.to_string();
        let is_self = incoming.is_self;

        // 1. 检查群组是否在绑定范围内
        let bindings = self.config.channel_bindings().await;
        let in_bound_group = bindings.iter().any(|b| {
            b.messenger_id == messenger_id
        });
        if !in_bound_group {
            return; // 非绑定 channel 的消息丢弃
        }

        // 2. 检查 is_self
        if is_self == 1 {
            let ctx = self.context_builder.lock().await;
            if ctx.is_self_echo(&content) {
                return; // 自己发出的回显，丢弃
            }
            return;
        }

        // 3. 检查管理命令
        if CommandRouter::is_command(&content) {
            if CommandRouter::check_admin(&self.config, &messenger_id, &user_id).await {
                self.handle_admin_command(&content, &messenger_id, &user_id, &group_id).await;
            }
            // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
            return;
        }

        // 4. 普通消息 → agentic loop
        self.run_agentic_loop(incoming).await;
    }

    async fn handle_admin_command(
        &self,
        content: &str,
        messenger_id: &str,
        user_id: &str,
        group_id: &str,
    ) {
        match CommandRouter::parse(content) {
            Ok(cmd) => {
                match CommandRouter::execute(&cmd, &self.config).await {
                    Ok((reply, cmd_needs_reset)) => {
                        self.send_reply(messenger_id, user_id, group_id, reply).await;

                        // 处理需要触发上下文重建的命令
                        if cmd_needs_reset {
                            match &cmd {
                                AdminCommand::SetRole(role) => {
                                    let role = role.clone();
                                    self.mode_manager.set_mode(Mode::Role).await;
                                    self.reset_context().await;
                                    self.send_reply(messenger_id, user_id, group_id,
                                        format!("🔄 已{}，上下文已重建",
                                            role.map(|r| format!("切换角色为: {}", r))
                                                .unwrap_or("取消角色".to_string()))).await;
                                }
                                AdminCommand::ModeEvent(event_id) => {
                                    let eid = event_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                                    self.mode_manager.set_mode(Mode::Event(eid.clone())).await;
                                    let _ = self.config.set_current_mode(Mode::Event(eid)).await;
                                    self.reset_context().await;
                                }
                                AdminCommand::ModeRole => {
                                    self.mode_manager.set_mode(Mode::Role).await;
                                    let _ = self.config.set_current_mode(Mode::Role).await;
                                    self.reset_context().await;
                                }
                                AdminCommand::Reenter(event_id) => {
                                    self.mode_manager.set_mode(Mode::Event(event_id.clone())).await;
                                    let _ = self.config.set_current_mode(Mode::Event(event_id.clone())).await;
                                    self.reset_context().await;
                                }
                                AdminCommand::Reset => {
                                    self.reset_context().await;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        self.send_reply(messenger_id, user_id, group_id,
                            format!("❌ 命令执行失败: {}", e)).await;
                    }
                }
            }
            Err(e) => {
                self.send_reply(messenger_id, user_id, group_id,
                    format!("⚠️ {}", e)).await;
            }
        }
    }

    async fn run_agentic_loop(&self, incoming: kissbot_api::channel::IncomingMessage) {
        let content = incoming.content.to_string();
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let time = incoming.time.to_string();

        // 1. 追加用户消息到上下文
        {
            let mut ctx = self.context_builder.lock().await;
            ctx.push_user_message(ContextMessage::User {
                messenger_id: messenger_id.clone(),
                user_id: user_id.clone(),
                group_id: group_id.clone(),
                content: content.clone(),
                time: time.clone(),
            });
        }

        // 2. 调用 LLM
        let response = {
            let ctx = self.context_builder.lock().await;
            let messages = ctx.build();
            let llm = self.llm_client.lock().await;
            llm.call(&messages).await
        };

        match response {
            Ok(llm_resp) => {
                let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

                // 3. 记录已发送内容
                {
                    let mut ctx = self.context_builder.lock().await;
                    ctx.push_assistant(llm_resp.content.clone(), now.clone());
                    ctx.record_sent_content(llm_resp.content.clone());
                }

                // 4. 推送 think 到 MemoryWriter
                let agent_id = self.config.agent_id().await;
                let role_name = self.config.current_role().await;
                let _ = self.memory_writer.push(WriteTask::Think {
                    agent_id,
                    role_name: Some(role_name),
                    content: llm_resp.content.clone(),
                    time: now,
                });

                // 5. 发送回复到通道
                self.send_reply(&messenger_id, &user_id, &group_id, llm_resp.content).await;

                // 6. 检查上下文超长
                let overflow = {
                    let ctx = self.context_builder.lock().await;
                    ctx.is_overflow()
                };
                if overflow {
                    warn!("上下文超长，触发重置");
                    self.reset_context().await;
                }
            }
            Err(e) => {
                warn!("LLM 调用失败: {:?}", e);
                self.send_reply(&messenger_id, &user_id, &group_id,
                    format!("❌ LLM 调用失败: {}", e)).await;
            }
        }
    }

    /// 上下文重置
    async fn reset_context(&self) {
        {
            let mut ctx = self.context_builder.lock().await;
            ctx.clear();
        }

        // 重新读取自我认知
        if let Ok(ego_info) = Self::load_ego_info(&self.config).await {
            let mut ctx = self.context_builder.lock().await;
            ctx.set_system_message(ego_info);
        }

        // 重新读取历史
        let mode = self.mode_manager.current().await;
        if let Ok(history) = self.memory_reader.read_history(&self.config, &mode).await {
            let mut ctx = self.context_builder.lock().await;
            ctx.load_history(history);
        }

        info!("上下文已重置");
    }

    async fn load_ego_info(config: &ConfigManager) -> Result<String> {
        let agent_id = config.agent_id().await;
        let role_name = config.current_role().await;
        let ego_url = config.memory_ego_url().await;

        let client = reqwest::Client::new();

        let mut system_parts = vec![];

        // 获取 agent 元数据
        if let Ok(agent_resp) = client.post(&format!("{}/agent/list", ego_url))
            .json(&serde_json::json!({}))
            .send()
            .await
        {
            if let Ok(data) = agent_resp.json::<serde_json::Value>().await {
                if let Some(name) = data["data"]["individual_name"].as_str() {
                    system_parts.push(format!("你的名字是: {}", name));
                }
                if let Some(desc) = data["data"]["description"].as_str() {
                    system_parts.push(format!("你的描述: {}", desc));
                }
            }
        }

        // 获取角色设定
        if !role_name.is_empty() {
            if let Ok(role_resp) = client.post(&format!("{}/role/get", ego_url))
                .json(&serde_json::json!({
                    "agent_id": agent_id,
                    "role_name": role_name,
                }))
                .send()
                .await
            {
                if let Ok(data) = role_resp.json::<serde_json::Value>().await {
                    if let Some(desc) = data["data"]["description"].as_str() {
                        system_parts.push(format!("角色: {} - {}", role_name, desc));
                    }
                }
            }
        }

        if system_parts.is_empty() {
            system_parts.push("你是 kissbot 智能助手".to_string());
        }

        Ok(system_parts.join("\n"))
    }

    async fn send_reply(&self, messenger_id: &str, user_id: &str, group_id: &str, content: String) {
        let msg = kissbot_api::channel::OutgoingMessage {
            messenger_id: std::sync::Arc::new(messenger_id.to_string()),
            user_id: std::sync::Arc::new(user_id.to_string()),
            group_id: std::sync::Arc::new(group_id.to_string()),
            msg_type: std::sync::Arc::new("text".to_string()),
            content: std::sync::Arc::new(content),
            attachment_map: std::sync::Arc::new(dashmap::DashMap::new()),
        };
        let _ = self.ws_client.send_reply(messenger_id, msg).await;
    }
}
