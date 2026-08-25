//! channel 运行态管理：Channel（单 channel 运行态）+ ChannelManager（全部 channel 的集合管理）
//! Channel 维护「已发出但尚未收到回显」的 msg_id 集合 + 运行态 mode + 运行时绑定的 client；
//! ChannelManager 持有全部 Channel（DashMap 无锁并发），coordinator 经 ChannelManager 访问各 channel 运行态。

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use arc_swap::{ArcSwap, ArcSwapOption};
use bytes::Bytes;
use dashmap::DashMap;
use tracing::{info, warn};

use kissbot_api::channel::{BindRequest, ChannelUser, IncomingMessageEvent, OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};

use crate::config_manager::ConfigManager;
use crate::nexus::Nexus;
use crate::types::{Mode, Result};

/// 每 channel 运行时：已发未回显的 outgoing msg_id 集合的 TTL（秒）
const CHANNEL_CONTEXT_TTL: Duration = Duration::from_secs(60);

/// 每 channel 运行时上下文：维护「已发出但尚未收到回显」的 msg_id 集合；
/// client 为运行时绑定（ArcSwapOption 无锁读写，未绑定为 None）
pub struct Channel {
    /// 已发出未回显的 msg_id -> 记录时间（DashMap 无锁并发访问）
    pending_outgoing: DashMap<String, Instant>,
    /// 运行态模式（ArcSwap 无锁读写；/mode 切换不回写，重启回 Role）
    mode: ArcSwap<Mode>,
    /// 本 channel 的 ChannelClient（connect_channel 时绑定；消息/回复路径从本字段取 client，
    /// ArcSwapOption 无锁读写——连接与回调并发访问安全）
    client: ArcSwapOption<ChannelClient>,
}

impl Channel {
    fn new() -> Self {
        Self {
            pending_outgoing: DashMap::new(),
            mode: ArcSwap::from_pointee(Mode::Role),
            client: ArcSwapOption::new(None),
        }
    }

    /// 发送消息（通道适配层：取 client + 发送 + 记录 pending msg_id 供回显判定；client/add_pending 内联）
    pub async fn send(&self, msg: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel_client::Error> {
        let Some(client) = self.client.load_full() else {
            warn!("send: 未找到 channel client");
            return Err(kissbot_channel_client::Error::NotConnected);
        };
        let response = client.send_message(msg).await?;
        // 记录已发出未回显的 msg_id（入站回显命中时 consume_pending 消费丢弃）
        self.evict(CHANNEL_CONTEXT_TTL);
        self.pending_outgoing.insert(response.msg_id.as_str().to_string(), Instant::now());
        Ok(response)
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

    /// 设置运行态模式（/mode 切换，不回写，重启回 Role）
    fn set_mode(&self, mode: Arc<Mode>) {
        self.mode.store(mode);
    }

    /// 取运行态模式（未绑定/缺失回退角色模式）
    fn mode(&self) -> Arc<Mode> {
        self.mode.load_full()
    }

    /// TTL 懒清理：先遍历收集过期条目，再逐个删除（DashMap 迭代期间不可修改，两步防死锁）
    fn evict(&self, ttl: Duration) {
        let expired: Vec<String> = self.pending_outgoing.iter()
            .filter(|e| e.value().elapsed() >= ttl)
            .map(|e| e.key().clone())
            .collect();
        for key in expired {
            self.pending_outgoing.remove(&key);
        }
    }
}

/// channel 集合管理器：通道适配层——持有全部 channel 运行态（Channel）与断线通知；
/// 实现 Terminal（回显过滤 + 转发业务）；连接/重连/发送封装（connect_channel/send）
/// 内部 DashMap 无锁并发；coordinator 持 Arc<ChannelManager>（connect_channel 需要 Arc<Self> 作为 Arc<dyn Terminal>）
pub struct ChannelManager {
    channels: DashMap<String, Arc<Channel>>,
    /// 断线通知：channel_id → Notify，closed() 回调通知重连循环
    disconnect_notify: DashMap<String, Arc<tokio::sync::Notify>>,
}

impl ChannelManager {
    pub fn new() -> Self {
        Self {
            channels: DashMap::new(),
            disconnect_notify: DashMap::new(),
        }
    }

    /// 取 channel 运行态，不存在则懒建（全部访问入口统一经此，缺省态一致）
    pub fn get_or_create(&self, channel_id: &str) -> Arc<Channel> {
        self.channels
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(Channel::new()))
            .clone()
    }

    /// 连接单个 channel（Nexus 调度：启动遍历 enabled 逐个调用；将来运行时新建 channel 也由 Nexus 经此调度）
    /// 设置 channel 运行态模式（/mode 切换，不回写，重启回 Role）
    pub fn set_mode(&self, channel_id: &str, mode: Arc<Mode>) {
        self.get_or_create(channel_id).set_mode(mode);
    }

    /// 取 channel 运行态模式（未绑定/缺失回退角色模式）
    pub fn mode(&self, channel_id: &str) -> Arc<Mode> {
        match self.channels.get(channel_id) {
            Some(c) => c.mode(),
            None => Arc::new(Mode::Role),
        }
    }

    /// 绑定 channel 用户（/bind：bind_users 追加，HashSet 天然去重幂等）
    pub async fn bind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            Arc::make_mut(&mut c.bind_users).insert(user.clone());
        }).await
    }

    /// 解绑 channel 用户（/unbind：移除 bind_users；若 outgoing 引用该身份则清空，避免悬空引用）
    /// 解绑 channel 用户（/unbind：移除 bind_users）
    pub async fn unbind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            Arc::make_mut(&mut c.bind_users).remove(user);
        }).await
    }

    /// 连接单个 channel（Nexus 调度：启动遍历 enabled 逐个调用；将来运行时新建 channel 也由 Nexus 经此调度）
    /// 连接与绑定统一由 ChannelConfig 描述：ws_url/bind_users 按 channel_id 读配置，bind_users 为绑定身份（逐个绑定）
    pub async fn connect_channel(self: Arc<Self>, channel_id: &str) {
        let Some(ch) = ConfigManager::get().channel(channel_id).await else {
            warn!("connect_channel: 未找到 channel 配置: {}", channel_id);
            return;
        };
        let reconnect_secs = ConfigManager::get().ws_reconnect_interval_secs();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        // Terminal 即 ChannelManager 自身（全局唯一）：循环外建一次 Terminal 视图，
        // 所有 channel client 的 Weak<dyn Terminal> 指向同一目标；强引用由 coordinator 的 Arc<ChannelManager> 保活
        let terminal: Arc<dyn Terminal> = self.clone();
        let channel_id = ch.channel_id.to_string();
        let ws_url = ch.ws_url.to_string();

        let client = ChannelClient::new(channel_id.clone(), Arc::downgrade(&terminal));

        // 断线通知
        let notify = Arc::new(tokio::sync::Notify::new());
        self.disconnect_notify.insert(channel_id.clone(), notify.clone());
        // ChannelClient 归入该 channel（懒建后 bind；消息/回复路径从 manager 取 client；bind_client 内联）
        self.get_or_create(&channel_id).client.store(Some(client.clone()));

        let client_clone = client.clone();
        let api_key = api_key.clone();

        tokio::spawn(async move {
            loop {
                match client_clone.clone().connect(&ws_url, &api_key).await {
                    Ok(()) => {
                        info!("已连接 channel: {}", channel_id);
                        // 绑定身份实时读取（/bind 回写后重连即生效；bind_users 逐个绑定；BindRequest.messenger_id 用绑定身份的 messenger 标识，如 "web"）
                        let bind_users = ConfigManager::get().channel(&channel_id).await
                            .map(|c| c.bind_users.clone());
                        if let Some(bus) = bind_users {
                            for bu in bus.iter() {
                                let _ = client_clone.bind(BindRequest {
                                    messenger_id: Arc::new(bu.messenger_id.clone()),
                                    user_id: Arc::new(bu.user_id.clone()),
                                }).await;
                            }
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

    /// 发送消息到 channel（取 Channel 后经 Channel::send；client/add_pending 已内联进 Channel::send）
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

// ==================== Terminal 回调（ChannelManager 实现：通道适配层） ====================

/// ChannelManager 即 Terminal 实现者：回显过滤在通道层完成（Coordinator 不见自身回显），
/// 有业务意义的事件（群组变更/用户移除等）已由服务端转化为 IncomingMessage 推送，其余回调不重复处理
#[async_trait]
impl Terminal for ChannelManager {
    /// 收到上行消息：先做回显过滤（通道层），再转发 Coordinator 业务处理
    async fn incoming_message(&self, channel_id: &str, event: Arc<IncomingMessageEvent>) {
        // 1. msg_id 回显判定：命中（已发未回显）则消费并丢弃，不转发业务（consume_pending 内联：
        //    channels.get 定位 channel 后直接调 Channel::consume_pending，无 channel 恒 false）
        let echo = match self.channels.get(channel_id) {
            Some(c) => c.consume_pending(event.incoming_message.msg_id.as_str()),
            None => false,
        };
        if echo {
            return;
        }
        // 2. 转发业务处理
        Nexus::get().incoming_message(channel_id, event).await;
    }

    async fn join_group(&self, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组加入事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
    }

    async fn leave_group(&self, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组离开事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
    }

    async fn user_removed(&self, _id: &str, _notification: Arc<UserRemoveNotification>) {
        // 用户删除事件，当前暂不处理（服务端已转化为 IncomingMessage 推送）
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_msg_id_consume() {
        let ctx = Channel::new();
        // 直接写 pending 集合（add_pending 已内联进 Channel::send，此处构造等价状态）
        ctx.pending_outgoing.insert("msg1".to_string(), Instant::now());
        // 加入后命中且消费移除
        assert!(ctx.consume_pending("msg1"));
        // 已消费，再次查询为 false
        assert!(!ctx.consume_pending("msg1"));
        // 未加入的 msg_id
        assert!(!ctx.consume_pending("nonexistent"));
    }

    #[test]
    fn channel_ttl_evict() {
        let ctx = Channel::new();
        // TTL=0：插入即过期（直接写 pending 集合，等价 add_pending 内联体），下次操作即被淘汰
        ctx.pending_outgoing.insert("expired".to_string(), Instant::now());
        ctx.evict(Duration::from_secs(0));
        assert!(!ctx.consume_pending("expired"), "TTL=0 插入即过期，应被淘汰");
        // 正常 TTL：未过期条目保留
        ctx.pending_outgoing.insert("fresh".to_string(), Instant::now());
        ctx.evict(CHANNEL_CONTEXT_TTL);
        assert!(ctx.consume_pending("fresh"));
    }

    #[test]
    fn channel_mode_state() {
        let ctx = Channel::new();
        // mode 默认 Role，设置后读回
        assert_eq!(ctx.mode().as_ref(), &Mode::Role);
        ctx.set_mode(Arc::new(Mode::Event("e1".into())));
        assert_eq!(ctx.mode().as_ref(), &Mode::Event("e1".into()));
    }
}
