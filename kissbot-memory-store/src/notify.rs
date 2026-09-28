//! 新数据通知服务：接受记忆结构实现模块的 WS 连接，写入完成后广播新数据。
//!
//! 帧格式复用 kai-ws 信封（`WsMessage`）：记录类型由 `payload_type` 承载
//! （见 [`kissbot_api::RecordKind::payload_type`]），`payload` 为 [`RecordNotification`]。
//! 客户端按自身水位从共享记忆文件读取，通知只传达位置信息。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use kai_ws::{
    CODE_SUCCESS, TYPE_HEARTBEAT, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsMessage,
    WsProcessorContext, ws_handle_connection_with_filter,
};
use kissbot_api::{ChannelKey, ChannelRecord, RecordKey, RecordKind, RecordNotification, ThinkRecord, ToolCallRecord, ToolResultRecord};
use kissbot_security::{ApiKeyWsFilter, SecurityConfig, SimpleApiKeyValidator};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tracing::{error, info, warn};

use crate::error::Result;

const MSG_QUEUE_SIZE: usize = 100;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// 广播事件：记录类型 + 通知负载
#[derive(Debug, Clone)]
pub struct NotifyEvent {
    pub kind: RecordKind,
    pub payload: RecordNotification,
}

static SENDER: OnceLock<UnboundedSender<NotifyEvent>> = OnceLock::new();

/// 发布一条新数据通知（写入完成后由 FileHook 调用；未启动通知服务时静默丢弃）
pub fn publish(kind: RecordKind, payload: RecordNotification) {
    let Some(sender) = SENDER.get() else {
        return;
    };
    if sender.send(NotifyEvent { kind, payload }).is_err() {
        warn!("[memory-store] notify receiver gone, drop notification: {:?}", kind);
    }
}

/// 通道消息通知：按通道分组，每个通道一条（订阅方据此只评估该通道）
pub fn publish_channel(key: &RecordKey, records: &[ChannelRecord]) {
    for notification in channel_notifications(key, records) {
        publish(RecordKind::Channel, notification);
    }
}

/// 生成通道消息通知：同一个文件按 agent+role+日期共用，一批写入可能混合多个通道，
/// 因此按通道分组，每通道取该通道在本批中的最后一条记录（时间 + 发送者身份）。
pub fn channel_notifications(key: &RecordKey, records: &[ChannelRecord]) -> Vec<RecordNotification> {
    let mut last_by_channel: BTreeMap<ChannelKey, &ChannelRecord> = BTreeMap::new();
    for record in records {
        last_by_channel.insert(ChannelKey::from(record), record);
    }
    last_by_channel
        .into_iter()
        .map(|(channel, last)| RecordNotification {
            agent_id: key.agent_id.clone(),
            role_event: key.role_event.clone(),
            time: last.time.clone(),
            is_self: Some(last.is_self),
            channel: Some(channel),
        })
        .collect()
}

pub fn publish_think(key: &RecordKey, records: &[ThinkRecord]) {
    if let Some(last) = records.last() {
        publish(RecordKind::Think, RecordNotification {
            agent_id: key.agent_id.clone(),
            role_event: key.role_event.clone(),
            time: last.time.clone(),
            is_self: None,
            channel: None,
        });
    }
}

pub fn publish_tool_call(key: &RecordKey, records: &[ToolCallRecord]) {
    if let Some(last) = records.last() {
        publish(RecordKind::ToolCall, RecordNotification {
            agent_id: key.agent_id.clone(),
            role_event: key.role_event.clone(),
            time: last.time.clone(),
            is_self: None,
            channel: None,
        });
    }
}

pub fn publish_tool_result(key: &RecordKey, records: &[ToolResultRecord]) {
    if let Some(last) = records.last() {
        publish(RecordKind::ToolResult, RecordNotification {
            agent_id: key.agent_id.clone(),
            role_event: key.role_event.clone(),
            time: last.time.clone(),
            is_self: None,
            channel: None,
        });
    }
}

/// 通知服务：维护已连接客户端，新数据到达时广播
pub struct NotifyServer {
    clients: DashMap<u32, Arc<WsContext>>,
    next_id: AtomicU32,
}

impl NotifyServer {
    /// 启动通知服务：绑定监听地址并进入接受循环（阻塞；调用方 spawn）。
    /// 首次调用时注册全局广播通道，此后 `publish*` 生效。
    pub async fn start(addr: &str) -> Result<()> {
        let (sender, mut receiver) = unbounded_channel::<NotifyEvent>();
        if SENDER.set(sender).is_err() {
            warn!("[memory-store] notify server already started, ignore duplicate start");
        }

        let server = Arc::new(Self {
            clients: DashMap::new(),
            next_id: AtomicU32::new(0),
        });

        // 广播任务：串行消费事件，避免写入路径上的同步回调阻塞
        let broadcaster = server.clone();
        tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                let payload = match serde_json::to_value(&event.payload) {
                    Ok(payload) => payload,
                    Err(e) => {
                        error!("[memory-store] serialize notification failed: {}", e);
                        continue;
                    }
                };
                let message = WsMessage {
                    sn: 0,
                    payload_type: event.kind.payload_type(),
                    status_code: CODE_SUCCESS,
                    payload: Some(payload),
                };
                let mut dead = Vec::new();
                for client in broadcaster.clients.iter() {
                    if client.value().send_json(message.clone()).await.is_err() {
                        dead.push(*client.key());
                    }
                }
                for id in dead {
                    broadcaster.clients.remove(&id);
                }
            }
        });

        let filter = ApiKeyWsFilter::new(Arc::new(SimpleApiKeyValidator::new(
            SecurityConfig::get().api_key.clone(),
        )));
        let listener = TcpListener::bind(addr).await?;
        info!("[memory-store] notify WS server listening on: {}", addr);
        loop {
            let (stream, _) = listener.accept().await?;
            // 单个连接的握手/读写失败不影响服务：记录后继续接受下一个连接
            if let Err(e) = ws_handle_connection_with_filter(stream, MSG_QUEUE_SIZE, server.clone(), &[&filter]).await {
                warn!("[memory-store] notify connection failed: {}", e);
            }
        }
    }
}

#[async_trait]
impl WsProcessorContext for NotifyServer {
    async fn init(self: Arc<Self>, ws_context: Arc<WsContext>) -> std::result::Result<(), kai_ws::Error> {
        let client_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.clients.insert(client_id, ws_context.clone());
        // 心跳：客户端心跳刷新超时，本端定期发送心跳
        let heartbeat = Arc::new(WsHeartbeatHandler::new(HEARTBEAT_INTERVAL, ws_context.clone()));
        ws_context.set_bin_processor(TYPE_HEARTBEAT, heartbeat.clone());
        tokio::spawn(async move { let _ = heartbeat.start().await; });
        // 断开时移除客户端
        ws_context.set_close_processor(Arc::new(NotifyClientCloseProcessor {
            server: Arc::downgrade(&self),
            client_id,
        }));
        Ok(())
    }
}

struct NotifyClientCloseProcessor {
    server: Weak<NotifyServer>,
    client_id: u32,
}

#[async_trait]
impl WsCloseProcessor for NotifyClientCloseProcessor {
    async fn process_close(&self, _context: Arc<WsContext>) {
        if let Some(server) = self.server.upgrade() {
            server.clients.remove(&self.client_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use arcstr::ArcStr;

    use super::*;
    use kissbot_api::RoleEvent;

    fn key() -> RecordKey {
        RecordKey {
            agent_id: ArcStr::from("0"),
            role_event: RoleEvent::new("dev"),
            date: ArcStr::from("2026-09-22"),
        }
    }

    #[test]
    fn test_publish_without_server_is_noop() {
        // 未启动通知服务时发布不 panic（SENDER 未注册）
        publish(RecordKind::Think, RecordNotification {
            agent_id: ArcStr::from("0"),
            role_event: RoleEvent::new("dev"),
            time: ArcStr::from("2026-09-22 13:48:37"),
            is_self: None,
            channel: None,
        });
    }

    fn record(user: &str, self_user: &str, group: &str, is_self: usize, time: &str, sn: u64) -> ChannelRecord {
        ChannelRecord {
            user_id: ArcStr::from(user),
            self_user_id: ArcStr::from(self_user),
            messenger_id: ArcStr::from("web"),
            group_id: ArcStr::from(group),
            is_self,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::new(),
            group_name: ArcStr::new(),
            content: kissbot_api::Content::Text(ArcStr::from("内容")),
            time: ArcStr::from(time),
            sn,
        }
    }

    #[test]
    fn test_channel_notifications_group_by_channel() {
        // 同一批可能混合多个通道（同 agent+role+日期共用一个文件）：每个通道各一条通知，
        // 取该通道在本批中的最后一条记录
        let records = vec![
            record("u1", "selfA", "gA", 0, "2026-09-22 13:00:00", 1),
            record("u2", "selfB", "gB", 0, "2026-09-22 13:00:01", 2),
            record("selfA", "selfA", "gA", 1, "2026-09-22 13:48:37", 3),
        ];
        let notifications = channel_notifications(&key(), &records);
        assert_eq!(notifications.len(), 2, "按通道分组，不丢同批的其他通道");
        let of = |group: &str| {
            notifications
                .iter()
                .find(|n| n.channel.as_ref().unwrap().group_id.as_str() == group)
                .unwrap()
        };
        let a = of("gA");
        assert_eq!(a.time.as_str(), "2026-09-22 13:48:37", "取该通道的最后一条记录");
        assert_eq!(a.is_self, Some(1));
        assert_eq!(a.channel.as_ref().unwrap().self_user_id.as_str(), "selfA");
        let b = of("gB");
        assert_eq!(b.time.as_str(), "2026-09-22 13:00:01");
        assert_eq!(b.is_self, Some(0));

        // 无接收端时发布不 panic
        publish_channel(&key(), &records);
    }
}
