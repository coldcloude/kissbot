use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Local;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::types::{
    Mode, WriteTask, ContextMessage, AdminCommand, Result, Error,
};
use crate::config_manager::{ConfigManager, ProviderModel};
use crate::command_router::CommandRouter;
use crate::model_client::ModelClient;
use crate::context_builder::ContextBuilder;
use crate::memory_reader::MemoryReader;
use crate::memory_writer::MemoryWriter;
use crate::memory_store_client::{MemoryStoreClient, ChannelRecord};
use crate::config_manager::{ChannelConfig, ChannelUser};

use kissbot_api::channel::{IncomingMessage, OutgoingMessage, BindRequest};
use kissbot_api::message::{Content, AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};

pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    memory_reader: Arc<MemoryReader>,
    memory_writer: Arc<MemoryWriter>,
    memory_store_client: Arc<MemoryStoreClient>,
    context_builder: Arc<tokio::sync::Mutex<ContextBuilder>>,
    model_client: Arc<tokio::sync::Mutex<ModelClient>>,
    /// 运行状态：当前 agent / 角色 / 模型 / 模式（启动从 NexusRepo 默认值初始化，运行期不回写）
    current_agent_id: ArcSwap<String>,
    current_role: ArcSwap<String>,
    current_model: ArcSwap<ProviderModel>,   // (provider, model) 打包
    current_mode: ArcSwap<Mode>,
    /// 运行状态：已绑定 channel（channel_id → ChannelUser），/bind /unbind 修改
    bound_channels: Arc<DashMap<String, Arc<ChannelUser>>>,
    /// 运行状态：当前选中的 memory-struct（本期未实现，先占位）
    #[allow(dead_code)]
    selected_memory_structs: Arc<DashMap<String, ()>>,
    /// 按 agent 内部 channel_id 索引的 ChannelClient
    channel_clients: Arc<DashMap<String, Arc<ChannelClient>>>,
    /// 断线通知：channel_id → Notify，closed() 通知重连循环
    disconnect_notify: Arc<DashMap<String, Arc<tokio::sync::Notify>>>,
}

impl AgentCoordinator {
    pub async fn new(
        config: Arc<ConfigManager>,
        memory_writer: MemoryWriter,
    ) -> Result<Arc<Self>> {
        // mode 不再落盘，由 coordinator 的 current_mode (ArcSwap) 持有，默认角色模式
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_writer = Arc::new(memory_writer);
        let memory_store_client = Arc::new(MemoryStoreClient::new());

        // 运行状态从 NexusRepo 默认值初始化
        let default_agent_id = config.default_agent_id().await;
        let default_role = config.default_role().await;
        let default_model = config.default_model().await;

        // ModelClient 每次调用现场合成配置，无需预查 model 配置
        let model_client = ModelClient::new(config.clone());

        // 初始化 ContextBuilder
        let context_builder = ContextBuilder::new();

        // 初始化 bound_channels：enabled_by_default 且 default_bind_user 非空（绑定集合）
        let bound_channels = Arc::new(Self::bound_channels_from_channels(config.channels().await));

        let coordinator = Arc::new(Self {
            config: config.clone(),
            memory_reader,
            memory_writer,
            memory_store_client,
            context_builder: Arc::new(tokio::sync::Mutex::new(context_builder)),
            model_client: Arc::new(tokio::sync::Mutex::new(model_client)),
            current_agent_id: ArcSwap::from_pointee(default_agent_id),
            current_role: ArcSwap::from_pointee(default_role),
            current_model: ArcSwap::from_pointee(default_model),
            current_mode: ArcSwap::from_pointee(Mode::Role),
            bound_channels,
            selected_memory_structs: Arc::new(DashMap::new()),
            channel_clients: Arc::new(DashMap::new()),
            disconnect_notify: Arc::new(DashMap::new()),
        });

        // 读取自我认知
        if let Ok(ego_info) = coordinator.load_ego_info().await {
            let mut ctx = coordinator.context_builder.lock().await;
            ctx.set_system_message(ego_info);
        }

        // 读取历史记忆
        let mode = coordinator.current_mode();
        if let Ok(history) = coordinator.memory_reader
            .read_history(&config, &coordinator.current_agent_id(), &coordinator.current_role(), &mode)
            .await
        {
            let mut ctx = coordinator.context_builder.lock().await;
            ctx.load_history(history);
        }

        // 读取顶层记忆索引（memory-struct 未实现时静默跳过）
        let mode = coordinator.current_mode();
        let _ = coordinator.memory_reader
            .read_memory_struct_index(&config, &coordinator.current_agent_id(), &coordinator.current_role(), &mode)
            .await;

        // 连接所有 channel
        coordinator.connect_channels().await;

        info!("AgentCoordinator 初始化完成");
        Ok(coordinator)
    }

    // ==================== 运行状态 getter/setter（不回写 NexusRepo） ====================

    pub fn current_agent_id(&self) -> String { self.current_agent_id.load().to_string() }
    pub fn current_role(&self) -> String { self.current_role.load().to_string() }
    pub fn current_model(&self) -> ProviderModel { (*self.current_model.load_full()).clone() }
    pub fn current_mode(&self) -> Mode { (*self.current_mode.load_full()).clone() }
    /// 切换当前模式（仅存状态，上下文重建由调用方触发 reset_context）
    pub fn set_current_mode(&self, mode: Mode) { self.current_mode.store(Arc::new(mode)); }

    /// 切换当前角色（角色切换同时重建上下文）
    pub async fn set_current_role(&self, role: Option<String>) {
        self.current_role.store(Arc::new(role.unwrap_or_default()));
        self.reset_context().await;
    }

    /// 切换当前模型（校验 provider/model 存在；每次调用由 ConfigManager 现场合成，无需热更新）
    pub async fn set_current_model(&self, pm: ProviderModel) -> Result<()> {
        // 校验 provider 与 model 存在
        if self.config.resolve_effective_config(&pm).await.is_none() {
            return Err(Error::ModelProviderNotSupported(format!(
                "provider/model 不存在: {}/{}", pm.provider, pm.model)));
        }
        self.current_model.store(Arc::new(pm));
        Ok(())
    }

    /// 切换当前 agent（agent 切换同时重建上下文）
    pub async fn set_current_agent_id(&self, id: String) {
        self.current_agent_id.store(Arc::new(id));
        self.reset_context().await;
    }

    // 运行时 /bind /unbind 目前仅修改 bound_channels（消息过滤），
    // 连接/绑定请求的运行期管理（自动连断）推迟到后续轮次：本轮不自动连断。
    /// 绑定 channel 用户（仅运行状态，不回写）；key 为 agent 内部 channel_id
    pub async fn bind_channel(&self, channel_id: &str, binding: ChannelUser) {
        self.bound_channels.insert(channel_id.to_string(), Arc::new(binding));
    }

    /// 解绑 channel（仅运行状态，不回写）
    pub async fn unbind_channel(&self, channel_id: &str) {
        self.bound_channels.remove(channel_id);
    }

    /// 计算启动时绑定集合：enabled_by_default 且 default_bind_user 非空的 channel
    /// （连接由 connect_channels 按 enabled_by_default 独立控制，此处只算绑定集合）
    /// 索引 key 为 agent 内部 channel_id（与消息方 messenger 无关）
    fn bound_channels_from_channels(
        channels: Vec<(String, Arc<ChannelConfig>)>,
    ) -> DashMap<String, Arc<ChannelUser>> {
        let map = DashMap::new();
        for (_, ch) in channels {
            if ch.enabled_by_default {
                if let Some(bu) = &ch.default_bind_user {
                    map.insert(ch.channel_id.to_string(), Arc::new(bu.clone()));
                }
            }
        }
        map
    }

    /// 连接所有 enabled_by_default 的 channel（NexusRepo channel 配置为连接来源）
    /// 连接与绑定两轴分离：enabled_by_default 控制连接，bound_channels 控制绑定
    async fn connect_channels(self: &Arc<Self>) {
        let reconnect_secs = self.config.ws_reconnect_interval_secs();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        let coordinator = self.clone();

        // 遍历 NexusRepo 中所有 channel，enabled_by_default 才连接
        for (_, ch) in self.config.channels().await {
            if !ch.enabled_by_default {
                continue; // 未启用：不连接
            }
            let channel_id = ch.channel_id.to_string();
            let ws_url = ch.ws_url.to_string();
            // 绑定身份来自运行状态 bound_channels；不在绑定集合则仅连接不绑定
            // ChannelUser 携带对端 messenger 标识（如 "web"）与 user 标识，bind 需用它们向通道服务注册身份
            let bound_user = self.bound_channels.get(&channel_id).map(|e| e.value().clone());

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

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
                        Ok(()) => {
                            info!("已连接 channel: {}", channel_id);
                            // 绑定用户（仅 bound_channels 中存在的 channel 发送绑定请求）
                            // BindRequest.messenger_id 用绑定身份的 messenger 标识（channel-web 注册的 messenger，如 "web"）
                            if let Some(bu) = &bound_user {
                                let _ = client_clone.bind(BindRequest {
                                    messenger_id: bu.messenger_id.clone(),
                                    user_id: bu.user_id.clone(),
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
        // 1. 推上行消息到记忆（使用 Arc 引用避免深复制）
        let agent_id = Arc::new(self.current_agent_id());
        let role_name = Arc::new(self.current_role());
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

        // 2. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
        self.handle_incoming(channel_id, message).await;
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
    async fn handle_incoming(&self, channel_id: &str, incoming: Arc<IncomingMessage>) {
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let is_self = incoming.is_self;
        let content_text = extract_text(&incoming.content);

        // 1. 检查 channel 是否在绑定范围内（key = agent 内部 channel_id）
        if !self.bound_channels.contains_key(channel_id) {
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
                self.handle_admin_command(channel_id, &content_text, &group_id).await;
            }
            // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
            return;
        }

        // 4. 普通消息 → agentic loop
        self.run_agentic_loop(channel_id, incoming).await;
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
                    Ok((reply, cmd_needs_reset)) => {
                        self.send_reply(channel_id, group_id, reply).await;

                        // 处理需要触发上下文重建的命令
                        if cmd_needs_reset {
                            match &cmd {
                                AdminCommand::ModeEvent(event_id) => {
                                    let eid = event_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                                    self.set_current_mode(Mode::Event(eid.clone()));
                                    self.reset_context().await;
                                }
                                AdminCommand::ModeRole => {
                                    self.set_current_mode(Mode::Role);
                                    self.reset_context().await;
                                }
                                AdminCommand::Reenter(event_id) => {
                                    self.set_current_mode(Mode::Event(event_id.clone()));
                                    self.reset_context().await;
                                }
                                AdminCommand::Reset => {
                                    self.reset_context().await;
                                }
                                // SetRole / Agent 已在 coordinator setter 内重建上下文，此处不重复 reset
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        self.send_reply(channel_id, group_id,
                            format!("❌ 命令执行失败: {}", e)).await;
                    }
                }
            }
            Err(e) => {
                self.send_reply(channel_id, group_id,
                    format!("⚠️ {}", e)).await;
            }
        }
    }

    async fn run_agentic_loop(&self, channel_id: &str, incoming: Arc<IncomingMessage>) {
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

        // 2. 调用模型
        let response = {
            let ctx = self.context_builder.lock().await;
            let messages = ctx.build();
            let model = self.model_client.lock().await;
            model.call(&self.current_model(), &messages).await
        };

        match response {
            Ok(model_resp) => {
                let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

                // 3. 记录已发送内容
                {
                    let mut ctx = self.context_builder.lock().await;
                    ctx.push_assistant(model_resp.content.clone(), now.clone());
                    ctx.record_sent_content(model_resp.content.clone());
                }

                // 4. 推送 think 到 MemoryWriter
                let agent_id = self.current_agent_id();
                let role_name = self.current_role();
                let _ = self.memory_writer.push(WriteTask::Think {
                    agent_id,
                    role_name: Some(role_name),
                    content: model_resp.content.clone(),
                    time: now,
                });

                // 5. 发送回复到通道
                self.send_reply(channel_id, &group_id, model_resp.content).await;

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
                warn!("模型调用失败: {:?}", e);
                self.send_reply(channel_id, &group_id,
                    format!("❌ 模型调用失败: {}", e)).await;
            }
        }
    }

    /// 发送回复消息到通道，成功后推记忆（is_self=1）
    /// channel_id 定位连接（agent 内部标识），messenger_id 为消息方身份（回复目标）
    /// 发送回复消息到通道，成功后推记忆（is_self=1）
    /// 发件人身份为 agent 绑定的用户（bound_channels[channel_id] 的 ChannelUser），回复到原群组
    async fn send_reply(&self, channel_id: &str, group_id: &str, content: String) {
        let Some(client) = self.channel_clients.get(channel_id) else {
            warn!("send_reply: 未找到 channel client: {}", channel_id);
            return;
        };
        let Some(bound) = self.bound_channels.get(channel_id).map(|e| e.value().clone()) else {
            warn!("send_reply: channel 未绑定: {}", channel_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: bound.messenger_id.clone(),   // 对端 messenger 标识（如 "web"）
            user_id: bound.user_id.clone(),             // agent 绑定的用户（如 "u1"）
            group_id: Arc::new(group_id.to_string()),
            content: Content::Text(Arc::new(content.clone())),
        };

        match client.send_message(msg).await {
            Ok(response) => {
                // 下行成功后推记忆（is_self=1，使用返回的 content）
                let agent_id = Arc::new(self.current_agent_id());
                let role_name = Arc::new(self.current_role());
                self.memory_store_client.push_channel_record(ChannelRecord {
                    agent_id,
                    role_name,
                    messenger_id: bound.messenger_id.clone(),
                    user_id: bound.user_id.clone(),
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
        if let Ok(ego_info) = self.load_ego_info().await {
            let mut ctx = self.context_builder.lock().await;
            ctx.set_system_message(ego_info);
        }

        // 读取历史记忆
        let mode = self.current_mode();
        if let Ok(history) = self.memory_reader
            .read_history(&self.config, &self.current_agent_id(), &self.current_role(), &mode)
            .await
        {
            let mut ctx = self.context_builder.lock().await;
            ctx.load_history(history);
        }

        // 读取顶层记忆索引（memory-struct 未实现时静默跳过）
        let mode = self.current_mode();
        let _ = self.memory_reader
            .read_memory_struct_index(&self.config, &self.current_agent_id(), &self.current_role(), &mode)
            .await;

        info!("上下文已重置");
    }

    /// 读取自我认知（agent 元数据 + 角色设定），agent_id/role 取当前运行状态
    async fn load_ego_info(&self) -> Result<String> {
        let agent_id = self.current_agent_id();
        let role_name = self.current_role();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use crate::config_manager::ChannelConfig;

    // 注：ConfigManager::new 依赖 KISSBOT_CONFIG 全局单例，单元测试难注入。
    // bound_channels 初始化逻辑用直接构造验证（连接/绑定两轴分离）：
    #[test]
    fn bound_channels_init_logic() {
        // 模拟三个 channel：
        // 1) c1: enabled + default_bind_user 非空 → 连接且绑定
        // 2) c3: enabled + default_bind_user 为空 → 连接但不绑定（消息被过滤直到运行时 /bind）
        // 3) c2: disabled + default_bind_user 非空 → 不连接也不绑定
        let enabled_with_bind = ChannelConfig {
            channel_id: Arc::new("c1".into()), ws_url: Arc::new("ws://x".into()),
            admins: Arc::new(HashSet::new()),
            default_bind_user: Some(ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) }),
            enabled_by_default: true,
        };
        let enabled_no_bind = ChannelConfig {
            channel_id: Arc::new("c3".into()),
            default_bind_user: None, ..enabled_with_bind.clone()
        };
        let disabled_with_bind = ChannelConfig {
            channel_id: Arc::new("c2".into()), enabled_by_default: false, ..enabled_with_bind.clone()
        };
        let all = vec![
            (enabled_with_bind.channel_id.to_string(), Arc::new(enabled_with_bind.clone())),
            (enabled_no_bind.channel_id.to_string(), Arc::new(enabled_no_bind.clone())),
            (disabled_with_bind.channel_id.to_string(), Arc::new(disabled_with_bind.clone())),
        ];

        // 绑定集合：仅 enabled_by_default 且 default_bind_user 非空的 channel（key = channel_id）
        let bound = AgentCoordinator::bound_channels_from_channels(all);
        assert_eq!(bound.len(), 1, "只有 c1 应入绑定集合");
        let entry = bound.get("c1").unwrap();
        assert_eq!(*entry.value().messenger_id, "m1");
        assert_eq!(*entry.value().user_id, "u1");
        // enabled 但无 default_bind_user：连接（见下）但不绑定
        assert!(!bound.contains_key("c3"), "enabled 无 default_bind_user 不应入绑定集合");
        // disabled：不连接也不绑定
        assert!(!bound.contains_key("c2"), "disabled 不应入绑定集合");

        // 连接集合：enabled_by_default 控制，与绑定无关
        let connect_set: Vec<String> = [
            enabled_with_bind, enabled_no_bind, disabled_with_bind,
        ].iter().filter(|c| c.enabled_by_default)
            .map(|c| c.channel_id.to_string())
            .collect();
        assert!(connect_set.contains(&"c1".to_string()), "c1 应连接");
        assert!(connect_set.contains(&"c3".to_string()), "c3 应连接（仅连接不绑定）");
        assert!(!connect_set.contains(&"c2".to_string()), "c2 不应连接");
    }
}
