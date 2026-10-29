//! channel 运行态管理：Channel（单 channel 运行态）+ ChannelManager（全部 channel 的集合管理）
//! Channel 维护「已发出但尚未收到回显」的 msg_id 集合 + 运行态事件 + 运行时绑定的 client；
//! ChannelManager 持有全部 Channel（DashMap 无锁并发），并作为 Terminal 实现者处理上行消息
//! （回显过滤 → 默认管理命令 → pipeline 命令 → 普通消息分发）。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arcstr::ArcStr;
use async_trait::async_trait;
use arc_swap::{ArcSwap, ArcSwapOption};
use bytes::Bytes;
use dashmap::DashMap;
use kissbot_api::{RESERVED_AGENT_ID, RESERVED_ROLE_NAME, RoleEvent};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use kissbot_api::channel::{BindRequest, ChannelUser, IncomingMessageEvent, OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfoResponse, Content, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};

use crate::config_manager::ConfigManager;
use crate::nexus::Nexus;
use crate::types::{ChannelCommand, Error, MemoryKey, Result};

/// 每 channel 运行时：已发未回显的 outgoing msg_id 集合的 TTL（秒）
const CHANNEL_CONTEXT_TTL: Duration = Duration::from_secs(60);

/// agent/role/event 变更任务（mpsc 队列串行处理，避免写-写竞态；读无需外部加锁）
/// 统一为「应用新的 pipeline 三元组」：写 config + 运行态 event + pipeline 重定位
struct ApplyChannelMemoryKey {
    channel_id: ArcStr,
    agent_id: Option<ArcStr>,
    role_name: Option<ArcStr>,
    event_id: Option<Option<ArcStr>>,
    done: oneshot::Sender<Result<()>>
}

/// channel 配置变更任务（排队调 ChannelManager 方法执行；与 ApplyChannelMemoryKey 同一消费者串行，写-写无竞态）
struct ChannelTask {
    cmd: ChannelCommand,
    done: oneshot::Sender<Result<ArcStr>>,
}

/// 每 channel 运行时上下文：维护「已发出但尚未收到回显」的 msg_id 集合；
/// client 为运行时绑定（ArcSwapOption 无锁读写，未绑定为 None）
pub struct Channel {
    /// 已发出未回显的 msg_id -> 记录时间（DashMap 无锁并发访问）
    pending_outgoing: DashMap<ArcStr, Instant>,
    /// 运行态事件（ArcSwap 无锁读写；None = 角色模式，Some(id) = 事件模式；
    /// /mode 切换不回写，重启回角色模式）
    event: ArcSwap<Option<ArcStr>>,
    /// 本 channel 的 ChannelClient（connect_channel 时绑定；消息/回复路径从本字段取 client，
    /// ArcSwapOption 无锁读写——连接与回调并发访问安全）
    client: ArcSwapOption<ChannelClient>,
    /// 断线通知（connect_channel 时绑定；closed() 回调经本字段 notify_one 通知重连循环，
    /// ArcSwapOption 无锁读写——连接与回调并发访问安全）
    disconnect_notify: ArcSwapOption<tokio::sync::Notify>,
}

impl Channel {
    fn new() -> Self {
        Self {
            pending_outgoing: DashMap::new(),
            event: ArcSwap::from_pointee(None),
            client: ArcSwapOption::new(None),
            disconnect_notify: ArcSwapOption::new(None),
        }
    }

    /// 发送消息（通道适配层：取 client + 发送 + 记录 pending msg_id 供回显判定）
    pub async fn send(&self, msg: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel_client::Error> {
        let Some(client) = self.client.load_full() else {
            warn!("send: 未找到 channel client");
            return Err(kissbot_channel_client::Error::NotConnected);
        };
        let response = client.send_message(msg).await?;
        // 记录已发出未回显的 msg_id（入站回显命中时 consume_pending 消费丢弃）
        self.evict(CHANNEL_CONTEXT_TTL);
        self.pending_outgoing.insert(response.msg_id.clone(), Instant::now());
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

/// channel 集合管理器：通道适配层——持有全部 channel 运行态（Channel）与断线通知；
/// 实现 Terminal（回显过滤 + 业务消息处理：默认命令 / pipeline 命令 / 普通消息）；
/// 连接/重连/发送封装（connect_channel/send）
/// 内部 DashMap 无锁并发；Nexus 持 Arc<ChannelManager>（connect_channel 需要 Arc<Self> 作为 Arc<dyn Terminal>）
/// 另持 channel 变更串行队列（写-写竞态防护；读无需外部加锁）：入队与执行都在本结构
/// （change_channel_key / channel_command → apply_channel_key / apply_channel_command），
/// 其中 pipeline 重定位（解绑旧 pipeline + 清理无绑定 + 装配新 pipeline）在 apply_channel_key 内联完成
pub struct ChannelManager {
    channels: DashMap<ArcStr, Arc<Channel>>,
    /// agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
    apply_channel_memory_key_tx: mpsc::UnboundedSender<ApplyChannelMemoryKey>,
    /// channel 配置变更串行队列（/bind、/unbind；与 ApplyChannelMemoryKey 同一消费者 select! 等待）
    channel_task_tx: mpsc::UnboundedSender<ChannelTask>,
}

impl ChannelManager {
    /// 构造并启动变更消费者（内含 tokio::spawn，需在 runtime 内调用）；
    /// 两队列经 select! 合并到同一消费者，所有 channel 配置写全局串行
    /// （消费者持一份 Arc<Self> 以便执行 apply_*；单例进程内长期存活，不构成引用环）
    pub fn new() -> Arc<Self> {
        // agent/role/event 变更串行队列（写-写竞态防护；读无需外部加锁）
        let (apply_channel_memory_key_tx, mut apply_channel_memory_key_rx) = mpsc::unbounded_channel::<ApplyChannelMemoryKey>();
        // channel 配置变更串行队列（/bind、/unbind；与 ApplyChannelMemoryKey 同一消费者 select! 等待）
        let (channel_task_tx, mut channel_task_rx) = mpsc::unbounded_channel::<ChannelTask>();

        let this = Arc::new(Self {
            channels: DashMap::new(),
            apply_channel_memory_key_tx,
            channel_task_tx,
        });

        // 启动变更消费者：agent/role/event 变更 + channel 配置变更串行处理（避免写-写竞态；读不受影响）
        let consumer = this.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    change = apply_channel_memory_key_rx.recv() => {
                        match change {
                            Some(ApplyChannelMemoryKey { channel_id, agent_id, role_name, event_id, done }) => {
                                let rst = consumer.apply_channel_key(&channel_id, agent_id, role_name, event_id).await;
                                let _ = done.send(rst);
                            }
                            // 任一队列关闭则消费者退出（tx 由 Nexus 单例持有的本结构保存，不会关闭，break 仅防御）
                            None => break,
                        }
                    }
                    task = channel_task_rx.recv() => {
                        match task {
                            Some(ChannelTask { cmd, done }) => {
                                let rst = consumer.apply_channel_command(cmd).await;
                                let _ = done.send(rst);
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        this
    }

    /// agent/role/mode 变更统一入口：三个字段独立 Option，None = 保持当前值；排队串行执行，返回时已生效
    pub async fn change_channel_key(
        &self,
        channel_id: ArcStr,
        agent_id: Option<ArcStr>,
        role_name: Option<ArcStr>,
        event_id: Option<Option<ArcStr>>,
    ) -> Result<()> {
        let (done_tx, done_rx) = oneshot::channel();
        self.apply_channel_memory_key_tx.send(ApplyChannelMemoryKey {
            channel_id,
            agent_id,
            role_name,
            event_id,
            done: done_tx,
        }).map_err(|_| Error::InternalError(ArcStr::from("变更队列已关闭")))?;
        done_rx.await.map_err(|_| Error::InternalError(ArcStr::from("变更处理中断")))?
    }

    /// channel 配置变更统一入口（/bind、/unbind）：排队串行执行，返回时已生效
    pub async fn channel_command(&self, cmd: ChannelCommand) -> Result<ArcStr> {
        let (done_tx, done_rx) = oneshot::channel();
        self.channel_task_tx.send(ChannelTask { cmd, done: done_tx })
            .map_err(|_| Error::InternalError("变更队列已关闭".into()))?;
        done_rx.await.map_err(|_| Error::InternalError("变更处理中断".into()))?
    }

    // ---- 变更执行体（队列消费者内串行调用，不对外） ----

    /// 来源 channel 绑定信息变化后重定位 pipeline：写 config + 运行态 event + 清理无绑定 pipeline +
    /// 为新三元组装配 pipeline
    /// 运行态 event 写 Channel.event（/mode 切换不回写，重启回角色模式）
    /// None 字段 = 保持当前值：队列内结合当前状态合成新三元组（写-写串行，读-改-写无竞态）
    async fn apply_channel_key(
        &self,
        channel_id: &str,
        mut agent_id: Option<ArcStr>,
        mut role_name: Option<ArcStr>,
        mut event_id: Option<Option<ArcStr>>,
    ) -> Result<()> {
        let cm = ConfigManager::get();
        // 变化前的 pipeline 标识与绑定身份：写完新配置后旧 pipeline/旧 channel 的批次状态需要按它们清理
        let c = self.get_or_create(channel_id);
        let old_key = self.memory_key(channel_id).await;
        let old_bind_users = cm.channel(channel_id).await
            .map(|channel| channel.bind_users.clone())
            .unwrap_or_default();

        // 设置 agent 和 role
        let cc = if agent_id.is_some() || role_name.is_some() {
            cm.update_channel(channel_id, |c| {
                if let Some(agent_id) = agent_id.take() {
                    c.agent_id = agent_id.clone();
                }
                if let Some(role_name) = role_name.take() {
                    c.role_name = role_name.clone();
                }
            }).await?
        } else {
            cm.channel(channel_id).await.ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id).into()))?
        };
        // 设置 event
        if let Some(event_id) = event_id.take() {
            c.event.store(Arc::new(event_id));
        }
        // pipeline 重定位：解绑旧 channel + 清理无绑定 pipeline/session + 装配新 pipeline
        let new_key = Arc::new(MemoryKey {
            agent_id: cc.agent_id.clone(),
            role_event: Arc::new(RoleEvent {
                role_name: cc.role_name.clone(),
                event_id: c.event.load().as_ref().clone(),
            }),
        });
        let nexus = Nexus::get();
        // 旧 pipeline 不再绑定该 channel：清理该 channel（messenger + 绑定用户）的批次生产者与待处理消息
        if let Some(old_key) = old_key {
            if old_key != new_key {
                for user in old_bind_users.iter() {
                    nexus.pipeline_manager
                        .channel_unbound(&old_key, user.messenger_id.as_str(), user.user_id.as_str())
                        .await;
                }
            }
        }
        // 清理无任何 channel 绑定的 pipeline（session 随 pipeline 一并清理）
        let channels = ConfigManager::get().channels().await;
        let mut keys = HashSet::new();
        for (_, ch) in &channels {
            if let Some(key) = self.memory_key(ch.channel_id.as_str()).await {
                keys.insert(key);
            }
        }
        nexus.pipeline_manager.retain(&keys);
        // 为新三元组装配 pipeline
        if let Err(e) = nexus.pipeline_manager.ensure_pipeline(new_key).await {
            warn!("pipeline 装配失败: {:?}", e);
        }
        Ok(())
    }

    /// channel 配置变更执行：/bind、/unbind 回写 bind_users，并生成回复文本
    async fn apply_channel_command(&self, cmd: ChannelCommand) -> Result<ArcStr> {
        match cmd {
            ChannelCommand::BindUser { channel_id, user } => {
                self.bind_user(channel_id.as_str(), &user).await?;
                Ok(ArcStr::from(format!("✅ 已绑定 channel 用户: {} / {}", user.messenger_id, user.user_id)))
            },
            ChannelCommand::UnbindUser { channel_id, user } => {
                self.unbind_user(channel_id.as_str(), &user).await?;
                Ok(ArcStr::from(format!("✅ 已移除 channel 用户: {} / {}", user.messenger_id, user.user_id)))
            },
        }
    }

    /// 取 channel 运行态，不存在则懒建（全部访问入口统一经此，缺省态一致）
    pub fn get_or_create(&self, channel_id: &str) -> Arc<Channel> {
        self.channels
            .entry(ArcStr::from(channel_id))
            .or_insert_with(|| Arc::new(Channel::new()))
            .clone()
    }

    /// 按 channel 定位 pipeline 标识（config agent_id/role_name + 运行态事件；channel 不存在返回 None）。
    /// 运行态条目缺失时按角色模式（event = None）合成，启动阶段尚未建立运行态也能定位 pipeline。
    pub async fn memory_key(&self, channel_id: &str) -> Option<Arc<MemoryKey>> {
        let ch = ConfigManager::get().channel(channel_id).await?;
        // 运行态事件（未绑定/缺失回退角色模式，即 None）
        let event_id = self.channels.get(channel_id)
            .map(|c| c.event.load().as_ref().clone())
            .unwrap_or(None);
        Some(Arc::new(MemoryKey {
            agent_id: ch.agent_id.clone(),
            role_event: Arc::new(RoleEvent {
                role_name: ch.role_name.clone(),
                event_id,
            }),
        }))
    }

    /// 绑定 channel 用户（/bind：bind_users 追加，HashSet 天然去重幂等）
    pub async fn bind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            Arc::make_mut(&mut c.bind_users).insert(Arc::new(user.clone()));
        }).await?;
        Ok(())
    }

    /// 解绑 channel 用户（/unbind：移除 bind_users）
    pub async fn unbind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            Arc::make_mut(&mut c.bind_users).remove(user);
        }).await?;
        Ok(())
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
        // 所有 channel client 的 Weak<dyn Terminal> 指向同一目标；强引用由 Nexus 的 Arc<ChannelManager> 保活
        let terminal: Arc<dyn Terminal> = self.clone();
        let channel_id = ch.channel_id.clone();
        let ws_url = ch.ws_url.clone();

        let client = ChannelClient::new(channel_id.clone(), Arc::downgrade(&terminal));

        // Channel 懒建一次，用于挂 client 与断线通知（消息/回复路径从 manager 取 client）
        let channel = self.get_or_create(&channel_id);
        // 断线通知：closed() 回调从 Channel 取到它并 notify_one
        let notify = Arc::new(tokio::sync::Notify::new());
        channel.disconnect_notify.store(Some(notify.clone()));
        // ChannelClient 归入该 channel（发送路径从 Channel 取）
        channel.client.store(Some(client.clone()));

        tokio::spawn(async move {
            loop {
                match client.clone().connect(&ws_url, &api_key).await {
                    Ok(()) => {
                        info!("已连接 channel: {}", channel_id);
                        // 绑定身份实时读取（/bind 回写后重连即生效；bind_users 逐个绑定；BindRequest.messenger_id 用绑定身份的 messenger 标识，如 "web"）
                        let bind_users = ConfigManager::get().channel(&channel_id).await
                            .map(|c| c.bind_users.clone());
                        if let Some(bus) = bind_users {
                            for bu in bus.iter() {
                                let _ = client.bind(BindRequest {
                                    messenger_id: bu.messenger_id.clone(),
                                    user_id: bu.user_id.clone(),
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

// ==================== 业务消息处理（默认管理命令 / pipeline 命令 / 普通消息） ====================

impl ChannelManager {
    /// 默认管理命令检测与执行（incoming_message 已校验管理员后调用）：内联解析命令参数 + 直接执行，返回回复文本。
    /// `Ok(None)` 表示不是默认命令，由调用方交给来源 channel 的 pipeline 识别执行。
    /// 错误经 Result 返回：InvalidCommand（解析/格式错误）由调用方拼 "⚠️ {}"，其余拼 "❌ 命令执行失败: {}"。
    /// bind/unbind 走本结构 channel_command 队列串行（bind_users 回写）；
    /// agent/role/事件 走 change_channel_key 队列。Nexus 一律从单例取（不传参数）
    async fn execute_default_command(&self, content: &str, channel_id: ArcStr) -> Result<Option<ArcStr>> {
        let parts: Vec<ArcStr> = content.trim().split_whitespace().map(|s| {ArcStr::from(s)}).collect();
        if parts.is_empty() {
            return Err(Error::InvalidCommand("空命令".into()));
        }
        match parts[0].as_str() {
            "/bind" => {
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /bind messenger <messenger_id> <user_id>".into()
                    ));
                }
                let cu = ChannelUser { messenger_id: parts[2].clone(), user_id: parts[3].clone() };
                // 统一走串行队列应用（防写-写竞态；bind_users 追加，HashSet 天然去重幂等）
                self.channel_command(ChannelCommand::BindUser { channel_id, user: cu }).await.map(Some)
            }
            "/unbind" => {
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /unbind messenger <messenger_id> <user_id>".into()
                    ));
                }
                let cu = ChannelUser { messenger_id: parts[2].clone(), user_id: parts[3].clone() };
                // 统一走串行队列应用（防写-写竞态；移除 bind_users；回复文本队列内生成）
                self.channel_command(ChannelCommand::UnbindUser { channel_id, user: cu }).await.map(Some)
            }
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
            "/agent" => {
                // /agent [agent_id] [role]：缺省 agent_id 用保留 agent（"0"），缺省 role 用保留 role（空）
                let agent_id = parts.get(1);
                let role = parts.get(2);
                let new_agent_id = agent_id.filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| RESERVED_AGENT_ID);
                let new_role = role.cloned().unwrap_or_else(|| RESERVED_ROLE_NAME);
                // 校验 agent 存在（空/保留 id 直接通过）；role 非空时一并校验（agent+role 联合校验）
                if !new_agent_id.is_empty() && new_agent_id != RESERVED_AGENT_ID {
                    if Nexus::get().memory_ego_client.get_agent(new_agent_id.clone()).await?.is_none() {
                        return Err(Error::MemoryEgoError(format!("agent 不存在: {}", new_agent_id).into()));
                    }
                }
                if !new_role.is_empty()
                    && Nexus::get().memory_ego_client.get_role(new_agent_id.clone(), new_role.clone()).await?.is_none()
                {
                    return Err(Error::MemoryEgoError(format!("role 不存在: {}", new_role).into()));
                }
                // None = 保持当前值（mode 保持当前运行态；Arc clone 浅拷贝入队）
                self.change_channel_key(channel_id, Some(new_agent_id.clone()), Some(new_role.clone()), None).await?;
                Ok(Some(ArcStr::from(format!("✅ 已设置 agent: {} / role: {}", new_agent_id, new_role))))
            }
            "/role" => {
                // /role [name]：缺省用保留 role（空）
                let role = parts.get(1);
                let new_role = role.cloned().unwrap_or_else(|| RESERVED_ROLE_NAME);
                // 经来源 channel 取当前 agent_id 校验 role（role 变更保持 agent 不变；channel 不存在报错）
                let Some(ch) = ConfigManager::get().channel(channel_id.as_str()).await else {
                    return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id).into()));
                };
                if !new_role.is_empty()
                    && Nexus::get().memory_ego_client.get_role(ch.agent_id.clone(), new_role.clone()).await?.is_none()
                {
                    return Err(Error::MemoryEgoError(format!("role 不存在: {}", new_role).into()));
                }
                // None = 保持当前值（agent/mode 不变；Arc clone 浅拷贝入队）
                self.change_channel_key(channel_id, None, Some(new_role.clone()), None).await?;
                Ok(Some(ArcStr::from(format!("✅ 已设置 role: {}", new_role))))
            }
            "/mode" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand(
                        "格式: /mode event [event-id] 或 /mode role".into()
                    ));
                }
                match parts[1].as_str() {
                    "event" => {
                        // 缺省 event-id 生成新事件 ID；agent/role 保持当前值
                        let id = parts.get(2).cloned().unwrap_or_else(|| ArcStr::from(uuid::Uuid::new_v4().to_string()));
                        self.change_channel_key(channel_id, None, None, Some(Some(id.clone()))).await?;
                        Ok(Some(ArcStr::from(format!("✅ 新事件 ID: {}", id))))
                    }
                    "role" => {
                        // agent/role 保持当前值
                        self.change_channel_key(channel_id, None, None, Some(None)).await?;
                        Ok(Some(ArcStr::from("✅ 已切换为角色模式")))
                    }
                    _ => Err(Error::InvalidCommand(format!("未知模式: {}", parts[1]).into())),
                }
            }
            // 不是默认命令：交给来源 channel 的 pipeline 识别执行（/model、outgoing 等）
            _ => Ok(None),
        }
    }

    /// pipeline 级命令：默认命令检测未命中后，交给来源 channel 所属 pipeline 识别执行。
    /// pipeline 不认识该命令 → InvalidCommand("未知命令: {命令}")（与默认层同文案）；
    /// pipeline 尚未装配时按 agent/role 配置创建（/model 等命令可能先于普通消息到达）
    async fn execute_pipeline_command(&self, channel_id: ArcStr, content: ArcStr) -> Result<ArcStr> {
        let Some(key) = self.memory_key(channel_id.as_str()).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id).into()));
        };
        let unknown = ArcStr::from(content.split_whitespace().next().unwrap_or_default());
        let pipeline = Nexus::get().pipeline_manager.ensure_pipeline(key).await?;
        match pipeline.execute_command(channel_id, content).await {
            Some(result) => result,
            None => Err(Error::InvalidCommand(format!("未知命令: {}", unknown).into())),
        }
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

        // 发送经本结构（内部取 client + 记录 pending msg_id 供回显判定）
        if let Err(e) = self.send(channel_id, msg).await {
            warn!("send_admin_reply 失败: {:?}", e);
        }
    }
}

// ==================== Terminal 回调（ChannelManager 实现：通道适配层） ====================

/// ChannelManager 即 Terminal 实现者：回显过滤在通道层完成（业务处理不见自身回显），
/// 有业务意义的事件（群组变更/用户移除等）已由服务端转化为 IncomingMessage 推送，其余回调不重复处理
#[async_trait]
impl Terminal for ChannelManager {
    /// 收到上行消息：先做回显过滤（通道层），再做业务处理
    /// 完整处理链：默认命令检测 → pipeline 命令检测 → pipeline 定位 → 系统事件过滤 → 普通消息进 pipeline
    /// （上行记忆写入由 pipeline 自己完成：default_memory 收到即写，abstract_memory 在 locate 后写）
    async fn incoming_message(&self, channel_id: ArcStr, event: Arc<IncomingMessageEvent>) {
        // 1. msg_id 回显判定：命中（已发未回显）则消费并丢弃，不转发业务（无 channel 视为未命中）
        let echo = match self.channels.get(channel_id.as_str()) {
            Some(c) => c.consume_pending(event.incoming_message.msg_id.as_str()),
            None => false,
        };
        if echo {
            return;
        }

        // 2. 先取 channel 配置（命令分支 admin 判断与普通分支 pipeline 定位复用，避免重复查询）
        let Some(ch) = ConfigManager::get().channel(channel_id.as_str()).await else { return; };

        // 3. 管理命令：仅 Content::Text 且以 "/" 开头才判断（不 trim——前置空格可跳过命令检查）；
        //    命令与命令回复不找 memory_key、不入记忆（管理命令独立处理）
        if let Content::Text(text) = &event.incoming_message.content {
            if text.starts_with('/') {
                // 4. 判断是否管理员（admins HashSet O(1) contains，ch 已保存）；非管理员忽略不回复
                let is_admin = ch.admins.contains(&ChannelUser {
                    messenger_id: event.incoming_message.messenger_id.clone(),
                    user_id: event.incoming_message.user_id.clone(),
                });
                if is_admin {
                    // 5. 先做默认命令检测，未命中再交给来源 channel 的 pipeline 识别执行；
                    //    回复始终发回来源 channel（不走 out_channel）
                    let result = match self.execute_default_command(text.as_str(), channel_id.clone()).await {
                        Ok(Some(reply)) => Ok(reply),
                        Ok(None) => self.execute_pipeline_command(channel_id.clone(), text.clone()).await,
                        Err(e) => Err(e),
                    };
                    match result {
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

        // 6. pipeline 定位（config agent_id/role_name + 运行态事件；channel 已存在，不重复查询）
        let Some(key) = self.memory_key(channel_id.as_str()).await else {
            return;
        };

        // 7. 系统事件（群组变更/用户移除）不进 agentic loop
        match &event.incoming_message.content {
            Content::GroupJoin(_) | Content::GroupLeave(_) | Content::UserRemove(_) => return,
            _ => {}
        }

        // 8. 普通消息：运行 pipeline（记忆写入由各 pipeline 自行完成；pipeline 内部按需创建 session）
        let _ = Nexus::get().pipeline_manager.incoming_message(key, event).await;
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
            if let Some(notify) = ch.disconnect_notify.load_full() {
                notify.notify_one();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_msg_id_consume() {
        let ctx = Channel::new();
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
        let ctx = Channel::new();
        // TTL=0：插入即过期，下次操作即被淘汰
        ctx.pending_outgoing.insert(ArcStr::from("expired"), Instant::now());
        ctx.evict(Duration::from_secs(0));
        assert!(!ctx.consume_pending("expired"), "TTL=0 插入即过期，应被淘汰");
        // 正常 TTL：未过期条目保留
        ctx.pending_outgoing.insert(ArcStr::from("fresh"), Instant::now());
        ctx.evict(CHANNEL_CONTEXT_TTL);
        assert!(ctx.consume_pending("fresh"));
    }

    #[tokio::test]
    async fn model_command_is_handed_to_pipeline() {
        let channel_manager = ChannelManager::new();
        // /model 与未知命令都不是默认命令：返回 Ok(None)，由 incoming_message 交给 pipeline 识别执行
        for content in ["/model", "/model deepseek", "/model deepseek deepseek-chat", "/send-channel on"] {
            let reply = channel_manager.execute_default_command(content, ArcStr::from("no-such-channel")).await.unwrap();
            assert!(reply.is_none(), "{} 应交由 pipeline 处理", content);
        }
    }
}
