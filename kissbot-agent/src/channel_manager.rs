//! channel 运行态管理：Channel（单 channel 运行态）+ ChannelManager（全部 channel 的集合管理）
//! Channel 维护「已发出但尚未收到回显」的 msg_id 集合 + 运行态 agent_id/mode + 运行时绑定的 client/producer；
//! ChannelManager 持有全部 Channel（DashMap 无锁并发），coordinator 经 ChannelManager 访问各 channel 运行态。

use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::{ArcSwap, ArcSwapOption};
use dashmap::DashMap;

use kissbot_channel_client::ChannelClient;

use crate::session_manager::BatchProducer;
use crate::types::Mode;

/// 每 channel 运行时：已发未回显的 outgoing msg_id 集合的 TTL（秒）
const CHANNEL_CONTEXT_TTL_SECS: u64 = 60;
/// 上述 TTL 的 Duration 形式（evict 入参）
const CHANNEL_CONTEXT_TTL: Duration = Duration::from_secs(CHANNEL_CONTEXT_TTL_SECS);

/// 每 channel 运行时上下文：维护「已发出但尚未收到回显」的 msg_id 集合 + 运行态 agent_id；
/// client/producer 为运行时绑定（ArcSwapOption 无锁读写，未绑定为 None）
/// 运行态 agent_id（UUID）在启动绑定/切换 agent 时确定并自主保存；
/// 解析失败回退保留 agent_id（"0"，等同 agent_name="" 的保留语义）
pub struct Channel {
    /// 已发出未回显的 msg_id -> 记录时间（DashMap 无锁并发访问）
    pending_outgoing: DashMap<String, Instant>,
    /// 运行态 agent_id（ArcSwapOption 无锁读写；未绑定为 None，channel_agent 懒绑定）
    agent_id: ArcSwapOption<String>,
    /// 运行态模式（ArcSwap 无锁读写；/mode 切换不回写，重启回 Role）
    mode: ArcSwap<Mode>,
    /// 本 channel 的 ChannelClient（connect_channels 时绑定；消息/回复路径从本字段取 client，
    /// ArcSwapOption 无锁读写——连接与回调并发访问安全）
    client: ArcSwapOption<ChannelClient>,
    /// 合批生产侧（绑定会话时从 session.batch_producer 取 clone；会话重定位后刷新，None 时 enqueue 懒绑定）
    /// BatchProducer 字段全 Clone/Arc，无需锁——ArcSwapOption 原子替换/读取（与 agent_id 同模式）
    producer: ArcSwapOption<BatchProducer>,
}

impl Channel {
    fn new() -> Self {
        Self {
            pending_outgoing: DashMap::new(),
            agent_id: ArcSwapOption::new(None),
            mode: ArcSwap::from_pointee(Mode::Role),
            client: ArcSwapOption::new(None),
            producer: ArcSwapOption::new(None),
        }
    }

    /// 绑定 channel client（connect_channels 时绑定；每次重连循环启动更新）
    fn bind_client(&self, client: Arc<ChannelClient>) {
        self.client.store(Some(client));
    }

    /// 取 channel client（未连接/未绑定为 None）
    fn client(&self) -> Option<Arc<ChannelClient>> {
        self.client.load_full()
    }

    fn add_pending(&self, msg_id: String) {
        self.evict(CHANNEL_CONTEXT_TTL);
        self.pending_outgoing.insert(msg_id, Instant::now());
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

    /// 写入运行态 agent_id（启动绑定/切换 agent 时由 ChannelManager 调用）
    fn set_agent_id(&self, agent_id: Arc<String>) {
        self.agent_id.store(Some(agent_id));
    }

    /// 读运行态 agent_id（未绑定为 None，channel_agent 懒绑定）
    fn agent_id(&self) -> Option<Arc<String>> {
        self.agent_id.load_full()
    }

    /// 绑定合批生产侧（绑定会话时调用；None 时 enqueue 懒绑定）
    fn bind_producer(&self, producer: Arc<BatchProducer>) {
        self.producer.store(Some(producer));
    }

    /// 取合批生产侧（未绑定为 None）
    fn producer(&self) -> Option<Arc<BatchProducer>> {
        self.producer.load_full()
    }

    /// 设置运行态模式（/mode 切换，不回写，重启回 Role）
    fn set_mode(&self, mode: Mode) {
        self.mode.store(Arc::new(mode));
    }

    /// 取运行态模式（未绑定/缺失回退角色模式）
    fn mode(&self) -> Mode {
        (*self.mode.load_full()).clone()
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

/// channel 集合管理器：持有全部 channel 的运行态（Channel），按 channel_id 懒建/读取；
/// 内部 DashMap 无锁并发；coordinator 持 Arc<ChannelManager>，
/// 消息/回复/合批/命令路径统一经其访问各 channel 运行态
pub struct ChannelManager {
    channels: DashMap<String, Arc<Channel>>,
}

impl ChannelManager {
    pub fn new() -> Self {
        Self {
            channels: DashMap::new(),
        }
    }

    /// 取 channel 运行态，不存在则懒建（全部访问入口统一经此，缺省态一致）
    pub fn get_or_create(&self, channel_id: &str) -> Arc<Channel> {
        self.channels
            .entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(Channel::new()))
            .clone()
    }

    /// 绑定 channel client（connect_channels 时绑定；每次重连循环启动更新）
    pub fn bind_client(&self, channel_id: &str, client: Arc<ChannelClient>) {
        self.get_or_create(channel_id).bind_client(client);
    }

    /// 取 channel client（未连接/未绑定为 None）
    pub fn client(&self, channel_id: &str) -> Option<Arc<ChannelClient>> {
        self.channels.get(channel_id).and_then(|c| c.client())
    }

    /// 记录已发出的 outgoing msg_id 到该 channel 的 pending 集合（回显判定用）
    pub fn add_pending(&self, channel_id: &str, msg_id: String) {
        self.get_or_create(channel_id).add_pending(msg_id);
    }

    /// 按 msg_id 判定是否为自身发出的回显；命中则消费（移除）并返回 true
    pub fn consume_pending(&self, channel_id: &str, msg_id: &str) -> bool {
        match self.channels.get(channel_id) {
            Some(c) => c.consume_pending(msg_id),
            None => false,
        }
    }

    /// 写入 channel 运行态 agent_id（启动绑定/切换 agent 时由 coordinator 调用）
    pub fn set_agent_id(&self, channel_id: &str, agent_id: Arc<String>) {
        self.get_or_create(channel_id).set_agent_id(agent_id);
    }

    /// 读 channel 运行态 agent_id（未绑定为 None，channel_agent 懒绑定）
    pub fn agent_id(&self, channel_id: &str) -> Option<Arc<String>> {
        self.channels.get(channel_id).and_then(|c| c.agent_id())
    }

    /// 绑定合批生产侧（绑定会话后刷新；None 时 enqueue 懒绑定）
    pub fn bind_producer(&self, channel_id: &str, producer: Arc<BatchProducer>) {
        self.get_or_create(channel_id).bind_producer(producer);
    }

    /// 取合批生产侧（未绑定为 None）
    pub fn producer(&self, channel_id: &str) -> Option<Arc<BatchProducer>> {
        self.channels.get(channel_id).and_then(|c| c.producer())
    }

    /// 设置 channel 运行态模式（/mode 切换，不回写，重启回 Role）
    pub fn set_mode(&self, channel_id: &str, mode: Mode) {
        self.get_or_create(channel_id).set_mode(mode);
    }

    /// 取 channel 运行态模式（未绑定/缺失回退角色模式）
    pub fn mode(&self, channel_id: &str) -> Mode {
        match self.channels.get(channel_id) {
            Some(c) => c.mode(),
            None => Mode::Role,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_msg_id_consume() {
        let ctx = Channel::new();
        // 加入后命中且消费移除
        ctx.add_pending("msg1".to_string());
        assert!(ctx.consume_pending("msg1"));
        // 已消费，再次查询为 false
        assert!(!ctx.consume_pending("msg1"));
        // 未加入的 msg_id
        assert!(!ctx.consume_pending("nonexistent"));
    }

    #[test]
    fn channel_ttl_evict() {
        let ctx = Channel::new();
        // TTL=0：插入即过期（走真实 add_pending 路径），下次操作即被淘汰
        ctx.add_pending("expired".to_string());
        ctx.evict(Duration::from_secs(0));
        assert!(!ctx.consume_pending("expired"), "TTL=0 插入即过期，应被淘汰");
        // 正常 TTL：未过期条目保留
        ctx.add_pending("fresh".to_string());
        ctx.evict(Duration::from_secs(CHANNEL_CONTEXT_TTL_SECS));
        assert!(ctx.consume_pending("fresh"));
    }

    #[test]
    fn channel_manager_lazy_create_and_isolated_state() {
        let mgr = ChannelManager::new();
        // 未创建 channel：懒建后读写分离，互不干扰
        mgr.add_pending("c1", "msg1".to_string());
        assert!(mgr.consume_pending("c1", "msg1"));
        assert!(!mgr.consume_pending("c1", "msg1"));
        // mode 默认 Role，设置后读回
        assert_eq!(mgr.mode("c1"), Mode::Role);
        mgr.set_mode("c1", Mode::Event("e1".into()));
        assert_eq!(mgr.mode("c1"), Mode::Event("e1".into()));
        // 未创建 channel mode 回退 Role
        assert_eq!(mgr.mode("c2"), Mode::Role);
        // agent_id 未绑定 None，绑定后读回
        assert!(mgr.agent_id("c1").is_none());
        mgr.set_agent_id("c1", Arc::new("aid".into()));
        assert_eq!(mgr.agent_id("c1").unwrap().as_str(), "aid");
    }
}
