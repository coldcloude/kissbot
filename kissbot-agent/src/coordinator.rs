use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Local;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::types::{
    Mode, WriteTask, ContextMessage, AdminCommand, Result,
};
use crate::config_manager::ConfigManager;
use crate::mode_manager::ModeManager;
use crate::command_router::CommandRouter;
use crate::llm_client::LlmClient;
use crate::context_builder::ContextBuilder;
use crate::memory_reader::MemoryReader;
use crate::memory_writer::MemoryWriter;
use crate::memory_store_client::{MemoryStoreClient, ChannelRecord};

use kissbot_api::channel::{IncomingMessage, OutgoingMessage, BindRequest};
use kissbot_api::message::{Content, AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};

pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    mode_manager: Arc<ModeManager>,
    memory_reader: Arc<MemoryReader>,
    memory_writer: Arc<MemoryWriter>,
    memory_store_client: Arc<MemoryStoreClient>,
    context_builder: Arc<tokio::sync::Mutex<ContextBuilder>>,
    llm_client: Arc<tokio::sync::Mutex<LlmClient>>,
    /// 按 messenger_id 索引的 ChannelClient
    channel_clients: Arc<DashMap<String, Arc<ChannelClient>>>,
    /// 断线通知：messenger_id → Notify，closed() 通知重连循环
    disconnect_notify: Arc<DashMap<String, Arc<tokio::sync::Notify>>>,
}

impl AgentCoordinator {
    pub async fn new(
        config: Arc<ConfigManager>,
        memory_writer: MemoryWriter,
    ) -> Result<Arc<Self>> {
        let mode = config.current_mode().await;
        let mode_manager = Arc::new(ModeManager::new(mode.clone()));
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_writer = Arc::new(memory_writer);
        let memory_store_client = Arc::new(MemoryStoreClient::new());

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

        // 读取顶层记忆索引（memory-struct 未实现时静默跳过）
        let _ = memory_reader.read_memory_struct_index(&config, &mode).await;

        let coordinator = Arc::new(Self {
            config,
            mode_manager,
            memory_reader,
            memory_writer,
            memory_store_client,
            context_builder: Arc::new(tokio::sync::Mutex::new(context_builder)),
            llm_client,
            channel_clients: Arc::new(DashMap::new()),
            disconnect_notify: Arc::new(DashMap::new()),
        });

        // 连接所有 channel
        coordinator.connect_all_channels().await;

        info!("AgentCoordinator 初始化完成");
        Ok(coordinator)
    }

    /// 连接所有已配置的 channel
    async fn connect_all_channels(self: &Arc<Self>) {
        let config = &self.config;
        let bindings = config.channel_bindings().await;
        let ws_url = config.channel_ws_url().await;
        let reconnect_secs = config.ws_reconnect_interval_secs().await;
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();

        for binding in &bindings {
            let messenger_id = binding.messenger_id.clone();
            let user_id = binding.user_id.clone();
            let terminal: Arc<dyn Terminal> = self.clone();
            let client = ChannelClient::new(messenger_id.clone(), Arc::downgrade(&terminal));
            let client_clone = client.clone();

            // 断线通知
            let notify = Arc::new(tokio::sync::Notify::new());
            self.disconnect_notify.insert(messenger_id.clone(), notify.clone());

            let ws_url = ws_url.clone();
            let api_key = api_key.clone();
            self.channel_clients.insert(messenger_id.clone(), client);

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
                        Ok(()) => {
                            info!("已连接 channel: {}", messenger_id);
                            // 绑定用户
                            let _ = client_clone.bind(BindRequest {
                                messenger_id: Arc::new(messenger_id.clone()),
                                user_id: Arc::new(user_id.clone()),
                            }).await;
                            // 等待断线通知（closed() 回调中 notify_one）
                            notify.notified().await;
                        }
                        Err(e) => {
                            warn!("连接 channel {} 失败: {:?}，{}秒后重连", messenger_id, e, reconnect_secs);
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
    async fn incoming_message(&self, _id: &str, message: Arc<IncomingMessage>) {
        // 1. 推上行消息到记忆（使用 Arc 引用避免深复制）
        let agent_id = Arc::new(self.config.agent_id().await);
        let role_name = Arc::new(self.config.current_role().await);
        self.memory_store_client.push_channel_record(ChannelRecord {
            agent_id,
            role_name,
            messenger_id: message.messenger_id.clone(),
            user_id: message.user_id.clone(),
            group_id: message.group_id.clone(),
            is_self: message.is_self,
            content: message.content.clone(),
            time: message.time.clone(),
        }).await;

        // 2. 处理消息
        self.handle_incoming(message).await;
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
    async fn handle_incoming(&self, incoming: Arc<IncomingMessage>) {
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let is_self = incoming.is_self;
        let content_text = extract_text(&incoming.content);

        // 1. 检查群组是否在绑定范围内
        let bindings = self.config.channel_bindings().await;
        if !bindings.iter().any(|b| b.messenger_id == messenger_id) {
            return; // 非绑定 channel 的消息丢弃
        }

        // 2. 检查 is_self
        if is_self == 1 {
            let ctx = self.context_builder.lock().await;
            if ctx.is_self_echo(&content_text) {
                return; // 自己发出的回显，丢弃
            }
            return;
        }

        // 3. 检查管理命令
        if CommandRouter::is_command(&content_text) {
            if CommandRouter::check_admin(&self.config, &messenger_id, &user_id).await {
                self.handle_admin_command(&content_text, &messenger_id, &user_id, &group_id).await;
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

    async fn run_agentic_loop(&self, incoming: Arc<IncomingMessage>) {
        let content_text = extract_text(&incoming.content);
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
                content: content_text.clone(),
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

    /// 发送回复消息到通道，成功后推记忆（is_self=1）
    async fn send_reply(&self, messenger_id: &str, user_id: &str, group_id: &str, content: String) {
        let Some(client) = self.channel_clients.get(messenger_id) else {
            warn!("send_reply: 未找到 channel client: {}", messenger_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: Arc::new(messenger_id.to_string()),
            user_id: Arc::new(user_id.to_string()),
            group_id: Arc::new(group_id.to_string()),
            content: Content::Text(Arc::new(content.clone())),
        };

        match client.send_message(msg).await {
            Ok(response) => {
                // 下行成功后推记忆（is_self=1，使用返回的 content）
                let agent_id = Arc::new(self.config.agent_id().await);
                let role_name = Arc::new(self.config.current_role().await);
                self.memory_store_client.push_channel_record(ChannelRecord {
                    agent_id,
                    role_name,
                    messenger_id: Arc::new(messenger_id.to_string()),
                    user_id: Arc::new(user_id.to_string()),
                    group_id: Arc::new(group_id.to_string()),
                    is_self: 1,
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;

                // 记录已发送内容（用于 is_self echo 检测）
                let mut ctx = self.context_builder.lock().await;
                ctx.record_sent_content(content);
            }
            Err(e) => {
                warn!("send_reply 失败: {:?}", e);
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

        // 读取历史记忆
        let mode = self.mode_manager.current().await;
        if let Ok(history) = self.memory_reader.read_history(&self.config, &mode).await {
            let mut ctx = self.context_builder.lock().await;
            ctx.load_history(history);
        }

        // 读取顶层记忆索引（memory-struct 未实现时静默跳过）
        let mode = self.mode_manager.current().await;
        let _ = self.memory_reader.read_memory_struct_index(&self.config, &mode).await;

        info!("上下文已重置");
    }

    async fn load_ego_info(config: &ConfigManager) -> Result<String> {
        let agent_id = config.agent_id().await;
        let role_name = config.current_role().await;
        let ego_url = kissbot_api::ApiConfig::get().memory_ego_url.clone();

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
