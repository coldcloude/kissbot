//! channel 运行态管理：Channel（单 channel 连接运行态）+ ChannelManager（全部 channel 的集合管理）
//! Channel 维护「已发出但尚未收到回显」的 msg_id 集合 + 随构造创建的 client + 连接状态；
//! ChannelManager 持有全部 Channel（DashMap 无锁并发）与 pipeline 输入路由表
//! （`(channel_id, messenger_id, user_id) -> RoleKey`），并作为 Terminal 实现者处理上行消息
//! （回显过滤 → 默认管理命令 → 按输入身份交给绑定的 pipeline）。
//! channel 只描述连接与管理员；绑定身份与转发规则由引用该 channel 的 pipeline 的 input_channels 决定。

use std::sync::{Arc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use arcstr::ArcStr;
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use kissbot_api::RoleKey;
use tracing::{error, info, warn};

use kissbot_api::channel::{BindRequest, ChannelUser, IncomingMessageEvent, OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfoResponse, Content, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};

use crate::config_manager::ConfigManager;
use crate::configs::InputChannel;
use crate::nexus::Nexus;
use crate::types::{Error, Result};

/// 每 channel 运行时：已发未回显的 outgoing msg_id 集合的 TTL（秒）
const CHANNEL_CONTEXT_TTL: Duration = Duration::from_secs(60);

/// 每 channel 运行时上下文：维护「已发出但尚未收到回显」的 msg_id 集合；
/// client 与断线通知随 Channel 构造创建（连接循环与回调共用同一实例）；
/// connected 表示当前连接是否可用（bind/unbind 与发送据此判断）
pub struct Channel {
    /// 已发出未回显的 msg_id -> 记录时间（DashMap 无锁并发访问）
    pending_outgoing: DashMap<ArcStr, Instant>,
    /// 本 channel 的 ChannelClient（构造时创建；消息/回复路径直接使用）
    client: Arc<ChannelClient>,
    /// 断线通知（构造时创建；closed() 回调经本字段 notify_one 通知重连循环）
    disconnect_notify: Arc<tokio::sync::Notify>,
    /// 当前连接是否已建立（连接成功后置 true，断线/连接失败置 false）
    connected: AtomicBool,
    /// 所属 ChannelManager（弱引用：Channel 由 manager 持有，避免环；连接循环用它取输入路由）
    manager: Weak<ChannelManager>,
}

impl Channel {
    /// 构造 channel 运行态并启动连接/重连循环（连接循环只在此启动，仅 channel 创建时调用一次）：
    /// client 随 Channel 创建，manager 只保存弱引用；连接成功后按路由表 bind 该 channel 上的
    /// 全部输入身份，断线等待 closed() 通知后重连，连接失败按重连间隔重试。
    fn new(channel_id: ArcStr, ws_url: ArcStr, manager: Arc<ChannelManager>) -> Arc<Self> {
        let manager = Arc::downgrade(&manager);
        let channel = Arc::new(Self {
            pending_outgoing: DashMap::new(),
            client: ChannelClient::new(channel_id, manager.clone()),
            disconnect_notify: Arc::new(tokio::sync::Notify::new()),
            connected: AtomicBool::new(false),
            manager,
        });
        let client = channel.client.clone();
        let notify = channel.disconnect_notify.clone();
        let task_channel = channel.clone();
        tokio::spawn(async move {
            let reconnect_secs = ConfigManager::get().ws_reconnect_interval_secs();
            let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
            loop {
                match client.clone().connect(&ws_url, &api_key).await {
                    Ok(()) => {
                        info!("已连接 channel: {}", client.id);
                        task_channel.set_connected(true);
                        // manager 已释放时没有路由可绑定，结束循环
                        let Some(manager) = task_channel.manager.upgrade() else { return; };
                        // 连接成功后按路由表绑定该 channel 的全部输入身份（重连后重新绑定）
                        for input in manager.channel_inputs(client.id.as_str()) {
                            if let Err(e) = task_channel.bind(&input).await {
                                warn!("bind 失败 {} / {}: {:?}", input.channel_id, input.user_id, e);
                            }
                        }
                        // 等待断线通知（closed() 回调中 notify_one）
                        notify.notified().await;
                        task_channel.set_connected(false);
                    }
                    Err(e) => {
                        task_channel.set_connected(false);
                        warn!("连接 channel {} 失败: {:?}，{}秒后重连", client.id, e, reconnect_secs);
                        tokio::time::sleep(Duration::from_secs(reconnect_secs)).await;
                    }
                }
            }
        });
        channel
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Release);
    }

    /// 发送消息（通道适配层：发送 + 记录 pending msg_id 供回显判定）
    pub async fn send(&self, msg: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel_client::Error> {
        let response = self.client.send_message(msg).await?;
        // 记录已发出未回显的 msg_id（入站回显命中时 consume_pending 消费丢弃）
        self.evict(CHANNEL_CONTEXT_TTL);
        self.pending_outgoing.insert(response.msg_id.clone(), Instant::now());
        Ok(response)
    }

    /// 绑定输入身份（消息层 (messenger_id, user_id) 绑定到本 channel 的连接）
    async fn bind(&self, input: &InputChannel) -> std::result::Result<(), kissbot_channel_client::Error> {
        self.client.bind(BindRequest {
            messenger_id: input.messenger_id.clone(),
            user_id: input.user_id.clone(),
        }).await
    }

    /// 解除输入身份绑定（运行期解绑入口内部使用；启动装配不使用）
    async fn unbind(&self, input: &InputChannel) -> std::result::Result<(), kissbot_channel_client::Error> {
        self.client.unbind(BindRequest {
            messenger_id: input.messenger_id.clone(),
            user_id: input.user_id.clone(),
        }).await
    }

    /// 命中则移除并返回 true（回显消费）；未命中再清理过期条目（懒清理）
    fn consume_pending(&self, msg_id: &str) -> bool {
        // 先尝试匹配消费（命中直接返回，避免每次消费都遍历清理）
        if self.pending_outgoing.remove(msg_id).is_some() {
            return true;
        }
        self.evict(CHANNEL_CONTEXT_TTL);
        false
    }

    /// TTL 懒清理：先遍历收集过期条目，再逐个删除（DashMap 迭代期间不可修改，两步防死锁）
    fn evict(&self, ttl: Duration) {
        let expired: Vec<ArcStr> = self.pending_outgoing.iter()
            .filter(|e| e.value().elapsed() >= ttl)
            .map(|e| e.key().clone())
            .collect();
        for key in expired {
            self.pending_outgoing.remove(&key);
        }
    }
}

/// channel 集合管理器：通道适配层——持有全部 channel 运行态（Channel）与 pipeline 输入路由；
/// 实现 Terminal（回显过滤 + 业务消息处理：默认命令 / 按输入身份定位 pipeline）；
/// channel 创建时启动连接/重连循环（get_or_create），发送（send）与输入身份 bind/unbind。
/// 内部 DashMap 无锁并发；Nexus 持 Arc<ChannelManager>（连接循环需要 Arc<Self> 作为 Arc<dyn Terminal>）
pub struct ChannelManager {
    channels: DashMap<ArcStr, Arc<Channel>>,
    /// 输入身份 → 绑定它的 pipeline；一个输入身份只能绑定一条 pipeline，重复绑定记 error 并跳过
    bindings: DashMap<InputChannel, Arc<RoleKey>>,
}

impl ChannelManager {
    /// 构造 channel 集合管理器（不启动后台任务；channel 首次创建时启动连接/重连循环）
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            channels: DashMap::new(),
            bindings: DashMap::new(),
        })
    }

    /// 取 channel 运行态：配置存在且启用时，不存在则创建并立即启动连接/重连循环
    /// （同一个 channel 只会创建并连接一次）；配置找不到或未启用直接失败，
    /// 不创建运行态、不建立连接。
    pub async fn get_or_create(self: &Arc<Self>, channel_id: &str) -> Option<Arc<Channel>> {
        let Some(ch) = ConfigManager::get().channel(channel_id).await else {
            warn!("channel 不存在，跳过创建: {}", channel_id);
            return None;
        };
        if !ch.enabled {
            warn!("channel 未启用，跳过创建: {}", channel_id);
            return None;
        }
        let channel = self.channels
            .entry(ch.channel_id.clone())
            .or_insert_with(|| Channel::new(ch.channel_id.clone(), ch.ws_url.clone(), self.clone()))
            .clone();
        Some(channel)
    }

    // ========== pipeline 输入路由与 bind/unbind ==========

    /// pipeline 增加一个输入身份：登记路由，再取（或创建）channel 运行态；
    /// channel 首次创建时直接建立连接并进入断线重连循环，已连接则立即 bind，
    /// 未连接由连接循环在连接成功后统一 bind。
    /// 启动装配时调用：一个输入身份只能绑定一条 pipeline，先到先得；冲突时记 error 并跳过。
    pub async fn bind_input(self: &Arc<Self>, input: Arc<InputChannel>, role_key: Arc<RoleKey>) {
        // 1. 登记路由：先绑定的 pipeline 生效；已登记（同一 pipeline 重复或冲突）直接返回
        if let Some(existing) = self.bindings.get(input.as_ref()) {
            if existing.value() != &role_key {
                error!(
                    "input 已被其他 pipeline 绑定，跳过: ({} / {} / {}) 当前 {}/{}，请求 {}/{}",
                    input.channel_id, input.messenger_id, input.user_id,
                    existing.value().agent_id, existing.value().role_name,
                    role_key.agent_id, role_key.role_name,
                );
            }
            return;
        }
        self.bindings.insert(input.as_ref().clone(), role_key.clone());
        // 2. 取 channel 运行态：首次创建时直接建立连接并进入断线重连循环；
        //    配置不存在或未启用时 get_or_create 直接失败（保留路由，但不会收到消息）
        let Some(channel) = self.get_or_create(input.channel_id.as_str()).await else {
            return;
        };
        // 3. 已连接则立即 bind，未连接由连接循环在连接成功后统一 bind
        if channel.is_connected() {
            if let Err(e) = channel.bind(&input).await {
                warn!("bind 失败 {} / {}: {:?}", input.channel_id, input.user_id, e);
            }
        }
    }

    /// 运行期移除一个输入身份（启动装配不使用）：仅当前绑定者可以解除；非绑定者的清理不生效。
    pub async fn unbind_input(&self, input: &InputChannel, role_key: &RoleKey) {
        let owned = self.bindings.get(input)
            .map(|entry| entry.value().as_ref() == role_key)
            .unwrap_or(false);
        if !owned {
            return;
        }
        self.bindings.remove(input);
        let Some(channel) = self.channels.get(input.channel_id.as_str()) else {
            return;
        };
        if channel.is_connected() {
            if let Err(e) = channel.unbind(input).await {
                warn!("unbind 失败 {} / {}: {:?}", input.channel_id, input.user_id, e);
            }
        }
    }

    /// 按上行事件的 (channel_id, messenger_id, 接收方 user_id) 取绑定的 pipeline；未绑定返回 None
    pub fn input_pipeline(&self, channel_id: ArcStr, messenger_id: ArcStr, user_id: ArcStr) -> Option<Arc<RoleKey>> {
        let input = InputChannel { channel_id, messenger_id, user_id };
        self.bindings.get(&input).map(|entry| entry.value().clone())
    }

    /// 该 pipeline 当前绑定的全部输入身份（运行期替换/删除 pipeline 时使用；启动装配不使用）
    pub fn inputs_of(&self, role_key: &RoleKey) -> Vec<InputChannel> {
        self.bindings.iter()
            .filter(|entry| entry.value().as_ref() == role_key)
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// 某 channel 上全部输入身份（连接成功后逐个 bind）
    fn channel_inputs(&self, channel_id: &str) -> Vec<Arc<InputChannel>> {
        self.bindings.iter()
            .filter(|entry| entry.key().channel_id.as_str() == channel_id)
            .map(|entry| Arc::new(entry.key().clone()))
            .collect()
    }

    // ========== 发送 ==========

    /// 发送消息到 channel（取 Channel 后经 Channel::send）
    pub async fn send(&self, channel_id: &str, msg: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel_client::Error> {
        match self.channels.get(channel_id) {
            Some(ch) => ch.send(msg).await,
            None => {
                warn!("send: 未找到 channel client: {}", channel_id);
                Err(kissbot_channel_client::Error::NotConnected)
            }
        }
    }
}

// ==================== 业务消息处理（默认管理命令 / 输入路由） ====================

impl ChannelManager {
    /// 默认管理命令检测与执行（incoming_message 已校验管理员后调用）：内联解析命令参数 + 直接执行，返回回复文本。
    /// `Ok(None)` 表示不是默认命令，由调用方按输入路由交给 pipeline。
    /// 错误经 Result 返回：InvalidCommand（解析/格式错误）由调用方拼 "⚠️ {}"，其余拼 "❌ 命令执行失败: {}"。
    /// 仅保留 channel 级的管理权限命令；agent/role/pipeline/绑定命令已移除，配置只通过 nexus.json 修改。
    pub(crate) async fn execute_default_command(&self, content: &str, channel_id: ArcStr) -> Result<Option<ArcStr>> {
        let parts: Vec<ArcStr> = content.trim().split_whitespace().map(|s| {ArcStr::from(s)}).collect();
        if parts.is_empty() {
            return Err(Error::InvalidCommand("空命令".into()));
        }
        match parts[0].as_str() {
            "/admin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /admin <messenger_id> <user_id>".into()
                    ));
                }
                ConfigManager::get().add_admin(channel_id.as_str(), Arc::new(ChannelUser {
                    messenger_id: parts[1].clone(),
                    user_id: parts[2].clone(),
                })).await?;
                Ok(Some(ArcStr::from(format!("✅ 已添加管理权限: {} / {}", parts[1], parts[2]))))
            }
            "/unadmin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /unadmin <messenger_id> <user_id>".into()
                    ));
                }
                ConfigManager::get().remove_admin(channel_id.as_str(), &ChannelUser {
                    messenger_id: parts[1].clone(),
                    user_id: parts[2].clone()
                }).await?;
                Ok(Some(ArcStr::from(format!("✅ 已移除管理权限: {} / {}", parts[1], parts[2]))))
            }
            // 不是默认命令：按输入路由交给 pipeline 识别执行（/model、/mode、outgoing 等）
            _ => Ok(None),
        }
    }

    /// 系统命令回复：始终发回来源 channel（不走 out_channel）
    /// 命令与命令回复不入记忆（管理命令不产生 session 上下文）：不定位 pipeline、不推 ChannelRecord
    /// 身份：messenger_id = incoming.messenger_id；user_id/self_user_id = event.recipient_user_id（接收方即发声身份，且是群成员）
    async fn send_admin_reply(&self, channel_id: &str, event: Arc<IncomingMessageEvent>, content: ArcStr) {
        let msg = OutgoingMessage {
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.recipient_user_id.clone(),
            group_id: event.incoming_message.group_id.clone(),
            content: Content::Text(content),
        };

        // 发送经本结构（内部取 client + 记录 pending msg_id 供回显判定）
        if let Err(e) = self.send(channel_id, msg).await {
            warn!("send_admin_reply 失败: {:?}", e);
        }
    }

    /// 命令错误回复：参数/格式错误用 ⚠️（只展示命令提示），执行失败用 ❌
    fn command_error_reply(error: Error) -> ArcStr {
        match error {
            Error::InvalidCommand(message) => ArcStr::from(format!("⚠️ {}", message)),
            other => ArcStr::from(format!("❌ 命令执行失败: {}", other)),
        }
    }
}

// ==================== Terminal 回调（ChannelManager 实现：通道适配层） ====================

/// ChannelManager 即 Terminal 实现者：回显过滤在通道层完成（业务处理不见自身回显），
/// 有业务意义的事件（群组变更/用户移除等）已由服务端转化为 IncomingMessage 推送，其余回调不重复处理
#[async_trait]
impl Terminal for ChannelManager {
    /// 收到上行消息：先做回显过滤（通道层），再做业务处理
    /// 完整处理链：默认命令检测 → 按输入身份定位 pipeline → 系统事件过滤 → 交给该 pipeline
    /// （上行记忆写入由 pipeline 自己完成：default_memory 收到即写，memory_summary 在 locate 后写）
    async fn incoming_message(&self, channel_id: ArcStr, event: Arc<IncomingMessageEvent>) {
        // 1. msg_id 回显判定：命中（已发未回显）则消费并丢弃，不转发业务（无 channel 视为未命中）
        let echo = match self.channels.get(channel_id.as_str()) {
            Some(c) => c.consume_pending(event.incoming_message.msg_id.as_str()),
            None => false,
        };
        if echo {
            return;
        }

        // 2. 先取 channel 配置（命令分支 admin 判断与路由复用，避免重复查询）
        let Some(ch) = ConfigManager::get().channel(channel_id.as_str()).await else { return; };

        // 3. 判断是否管理员（admins HashSet O(1) contains，ch 已保存）；非管理员忽略不回复
        let is_admin = ch.admins.contains(&ChannelUser {
            messenger_id: event.incoming_message.messenger_id.clone(),
            user_id: event.incoming_message.user_id.clone(),
        });

        // 4. 管理命令：仅 Content::Text 且以 "/" 开头才判断（不 trim——前置空格可跳过命令检查）；
        //    命令与命令回复不进入 pipeline
        if let Content::Text(text) = &event.incoming_message.content {
            if is_admin && text.starts_with('/') {
                match self.execute_default_command(text.as_str(), channel_id.clone()).await {
                    Ok(None) => {},
                    Ok(Some(reply)) => {
                        self.send_admin_reply(channel_id.as_str(), event, reply).await;
                        return;
                    },
                    Err(e) => {
                        self.send_admin_reply(channel_id.as_str(), event, Self::command_error_reply(e)).await;
                        return;
                    }
                }
            }
        }

        // 5. 系统事件（群组变更/用户移除）不进 agentic loop
        match &event.incoming_message.content {
            Content::GroupJoin(_) | Content::GroupLeave(_) | Content::UserRemove(_) => return,
            _ => {}
        }

        // 6. 按输入身份取绑定的 pipeline（一个身份只绑定一条 pipeline）
        let Some(key) = self.input_pipeline(
            channel_id.clone(),
            event.incoming_message.messenger_id.clone(),
            event.recipient_user_id.clone(),
        ) else {
            return;
        };
        let Some(result) = Nexus::get().pipeline_manager
            .incoming_message(key, event.clone(), channel_id.clone(), is_admin).await else {
            return;
        };
        match result {
            Ok(reply) => self.send_admin_reply(channel_id.as_str(), event, reply).await,
            Err(e) => self.send_admin_reply(channel_id.as_str(), event, Self::command_error_reply(e)).await,
        }
    }

    async fn join_group(&self, _id: ArcStr, _notification: Arc<GroupChangeNotification>) {
        // 群组加入事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
    }

    async fn leave_group(&self, _id: ArcStr, _notification: Arc<GroupChangeNotification>) {
        // 群组离开事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
    }

    async fn user_removed(&self, _id: ArcStr, _notification: Arc<UserRemoveNotification>) {
        // 用户删除事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
    }

    async fn download_chunk(&self, _id: ArcStr, _info: Arc<AttachmentInfoResponse>, _pos: u64, _data: Bytes) -> std::result::Result<(), kissbot_channel_client::Error> {
        // 当前未使用附件下载
        Ok(())
    }

    async fn closed(&self, id: ArcStr) {
        info!("channel 连接关闭: {}，准备重连", id);
        // 通知重连循环（经 channels.get 取该 channel 的断线通知）
        if let Some(ch) = self.channels.get(id.as_str()) {
            ch.set_connected(false);
            ch.disconnect_notify.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 Channel：直接构造本地状态、不启动连接（manager 仅供 client 持有弱引用，随后即释放）
    fn test_channel() -> Arc<Channel> {
        let manager = Arc::downgrade(&ChannelManager::new());
        Arc::new(Channel {
            pending_outgoing: DashMap::new(),
            client: ChannelClient::new(ArcStr::from("test-channel"), manager.clone()),
            disconnect_notify: Arc::new(tokio::sync::Notify::new()),
            connected: AtomicBool::new(false),
            manager,
        })
    }

    #[test]
    fn channel_msg_id_consume() {
        let ctx = test_channel();
        // 直接写 pending 集合，构造「已发出未回显」状态
        ctx.pending_outgoing.insert(ArcStr::from("msg1"), Instant::now());
        // 加入后命中且消费移除
        assert!(ctx.consume_pending("msg1"));
        // 已消费，再次查询为 false
        assert!(!ctx.consume_pending("msg1"));
        // 未加入的 msg_id
        assert!(!ctx.consume_pending("nonexistent"));
    }

    #[test]
    fn channel_ttl_evict() {
        let ctx = test_channel();
        // TTL=0：插入即过期，下次操作即被淘汰
        ctx.pending_outgoing.insert(ArcStr::from("expired"), Instant::now());
        ctx.evict(Duration::from_secs(0));
        assert!(!ctx.consume_pending("expired"), "TTL=0 插入即过期，应被淘汰");
        // 正常 TTL：未过期条目保留
        ctx.pending_outgoing.insert(ArcStr::from("fresh"), Instant::now());
        ctx.evict(CHANNEL_CONTEXT_TTL);
        assert!(ctx.consume_pending("fresh"));
    }
}
