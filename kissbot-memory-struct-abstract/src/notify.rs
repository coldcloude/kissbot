//! 通知订阅：作为 WS 客户端连接 memory-store 的通知服务，维护长连接与重连。
//!
//! 帧格式为 kai-ws 信封：记录类型由 `payload_type` 承载（见
//! [`kissbot_api::RecordKind::payload_type`]），`payload` 为 `RecordNotification`。

use std::sync::{Arc, Weak};
use std::time::Duration;

use arcstr::ArcStr;
use async_trait::async_trait;
use kai_ws::{
    TYPE_HEARTBEAT, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsJsonProcessor, WsMessage,
    WsProcessorContext,
};
use kissbot_api::{RecordKind, RecordNotification};
use kissbot_security::HEADER_API_KEY;
use tokio::sync::{Notify, RwLock};
use tracing::{error, info, warn};

use crate::error::Result;
use crate::service::AbstractService;

const QUEUE_SIZE: usize = 100;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// 通知客户端：把新数据通知交给服务层
pub struct NotifyClient {
    service: Arc<AbstractService>,
    ws_context: RwLock<Option<Arc<WsContext>>>,
    disconnected: Arc<Notify>,
}

impl NotifyClient {
    pub fn new(service: Arc<AbstractService>) -> Arc<Self> {
        Arc::new(Self {
            service,
            ws_context: RwLock::new(None),
            disconnected: Arc::new(Notify::new()),
        })
    }

    /// 连接-重连循环（阻塞；调用方 spawn）
    pub async fn run(self: Arc<Self>, url: &str, api_key: &str, reconnect_interval_secs: u64) {
        let headers = [(HEADER_API_KEY.to_string(), api_key.to_string())];
        let interval = Duration::from_secs(reconnect_interval_secs.max(1));
        loop {
            match kai_ws::ws_connect(url, &headers, QUEUE_SIZE, self.clone()).await {
                Ok(()) => info!("[abstract] connected to memory-store notify server: {}", url),
                Err(e) => warn!("[abstract] connect notify server {} failed: {}", url, e),
            }
            // 连接建立后等待关闭通知（notify_one 的 permit 语义保证不丢唤醒）
            self.disconnected.notified().await;
            *self.ws_context.write().await = None;
            tokio::time::sleep(interval).await;
        }
    }
}

#[async_trait]
impl WsProcessorContext for NotifyClient {
    async fn init(self: Arc<Self>, ws_context: Arc<WsContext>) -> std::result::Result<(), kai_ws::Error> {
        *self.ws_context.write().await = Some(ws_context.clone());

        let heartbeat = Arc::new(WsHeartbeatHandler::new(HEARTBEAT_INTERVAL, ws_context.clone()));
        ws_context.set_bin_processor(TYPE_HEARTBEAT, heartbeat.clone());
        tokio::spawn(async move { let _ = heartbeat.start().await; });

        ws_context.set_close_processor(Arc::new(NotifyCloseProcessor { client: Arc::downgrade(&self) }));

        for kind in RecordKind::ALL {
            ws_context.set_json_processor(
                kind.payload_type(),
                Arc::new(RecordNotificationProcessor { service: self.service.clone(), kind }),
            );
        }

        // 每次（重）连接后把已加载角色的全部后缀标记为待处理：断线期间的变更没有通知，
        // 通道未知，查询时按该后缀全部通道处理
        self.service.mark_all_dirty().await;
        Ok(())
    }
}

struct NotifyCloseProcessor {
    client: Weak<NotifyClient>,
}

#[async_trait]
impl WsCloseProcessor for NotifyCloseProcessor {
    async fn process_close(&self, _context: Arc<WsContext>) {
        if let Some(client) = self.client.upgrade() {
            client.disconnected.notify_one();
        }
    }
}

struct RecordNotificationProcessor {
    service: Arc<AbstractService>,
    kind: RecordKind,
}

#[async_trait]
impl WsJsonProcessor for RecordNotificationProcessor {
    async fn process_json(&self, data: WsMessage, _context: Arc<WsContext>) {
        let Some(payload) = data.payload else {
            warn!("[abstract] notification frame without payload: {:?}", self.kind);
            return;
        };
        let notification = match serde_json::from_value::<RecordNotification>(payload) {
            Ok(notification) => notification,
            Err(e) => {
                error!("[abstract] parse notification failed: {}", e);
                return;
            }
        };
        if let Err(e) = self.service.on_notification(self.kind, &notification).await {
            error!(
                "[abstract] handle notification ({:?}, {}/{:?}) failed: {}",
                self.kind, notification.agent_id, notification.role_event, e
            );
        }
    }
}

/// 订阅器入场点：未配置通知地址时不订阅（仅手动触发/重建可用）
pub async fn subscribe(service: Arc<AbstractService>, notify_url: Option<ArcStr>, reconnect_interval_secs: u64) -> Result<()> {
    let Some(url) = notify_url else {
        warn!("[abstract] notify_url not configured, notification subscription disabled");
        return Ok(());
    };
    let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
    let client = NotifyClient::new(service);
    client.run(&url, api_key.as_str(), reconnect_interval_secs).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_role::{AgentRole, AgentRoleManager};
    use crate::service::AbstractService;
    use crate::stub::StubModel;
    use kissbot_api::RoleEvent;

    fn service(root: &std::path::Path) -> Arc<AbstractService> {
        let manager = Arc::new(AgentRoleManager::new(root));
        let model = Arc::new(StubModel::new());
        Arc::new(AbstractService::new(manager, model, 4))
    }

    fn role_event(role_name: &str, event_id: Option<&str>) -> RoleEvent {
        RoleEvent {
            role_name: ArcStr::from(role_name),
            event_id: event_id.map(ArcStr::from),
        }
    }

    #[tokio::test]
    async fn processor_routes_channel_notification_to_archive() {
        let temp = tempfile::tempdir().unwrap();
        let service = service(temp.path());
        // 写共享记忆文件：本人消息 + 非本人消息
        let dir = temp.path().join("0").join("memory-store").join("2026-dev");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let mut lines = String::new();
        for (sn, is_self) in [(1u64, 1usize), (2, 0)] {
            let record = serde_json::json!({
                "user_id": if is_self == 1 { "self" } else { "u1" }, "self_user_id": "self",
                "messenger_id": "web", "group_id": "g1", "is_self": is_self,
                "messenger_name": "", "user_name": "alice", "group_name": "",
                "content": {"msg_type": "Text", "data": "内容"},
                "time": format!("2026-09-22 10:0{}:00", sn), "sn": sn
            });
            lines.push_str(&serde_json::to_string(&record).unwrap());
            lines.push('\n');
        }
        tokio::fs::write(dir.join("channel-records-2026-09-22.jsonl"), lines).await.unwrap();

        let processor = RecordNotificationProcessor { service: service.clone(), kind: RecordKind::Channel };
        let ws_context = Arc::new(WsContext::new(4));
        let frame = WsMessage {
            sn: 0,
            payload_type: RecordKind::Channel.payload_type(),
            status_code: kai_ws::CODE_SUCCESS,
            payload: Some(serde_json::json!({
                "agent_id": "0", "role_event": {"role_name": "dev"}, "time": "2026-09-22 10:02:00", "is_self": 0,
                "messenger_id": "web", "self_user_id": "self", "group_id": "g1"
            })),
        };
        processor.process_json(frame, ws_context).await;

        // 懒处理：通知只登记待处理标记，不做任何计算
        let channel = kissbot_api::ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"));
        let role = role_event("dev", None);
        let handle = service.manager().get(&AgentRole::new(ArcStr::from("0"), ArcStr::from("dev"))).unwrap();
        {
            let state = handle.state.lock().await;
            assert!(state.is_dirty(&role), "通知登记待处理标记");
            assert!(state.tree.is_empty(), "通知本身不归档");
            assert!(state.dirty_channels.contains(&(role.clone(), channel.clone())), "通道通知按通道登记");
        }

        // 查询只读：不触发归档
        let tree = service.tree_get(&AgentRole::new(ArcStr::from("0"), ArcStr::from("dev")), None).await.unwrap();
        assert!(tree.nodes.is_empty(), "查询不触发归档");
        {
            let state = handle.state.lock().await;
            assert!(state.is_dirty(&role), "查询不清除待处理标记");
        }

        // 手动触发归档：该通道水位之后的全部记录落成一个 case
        let outcome = service.manual_trigger(&AgentRole::new(ArcStr::from("0"), ArcStr::from("dev")), None).await.unwrap();
        assert_eq!(outcome.archived, 2, "手动触发处理该通道全部待处理记录");
        let tree = service.tree_get(&AgentRole::new(ArcStr::from("0"), ArcStr::from("dev")), None).await.unwrap();
        assert_eq!(tree.nodes.len(), 1, "手动触发归档出 case");
        assert_eq!(tree.nodes[0].record_count, 2, "该通道全部待处理记录一起处理");
        assert_eq!(tree.current_cases.len(), 1);
        assert_eq!(tree.current_cases[0].channel, channel);
        assert_eq!(tree.current_cases[0].case_name.as_str(), "role-case");

        let state = handle.state.lock().await;
        assert_eq!(state.watermarks.get(&(role.clone(), channel.clone())).unwrap().channel.sn, 2);
        assert!(!state.is_dirty(&role), "处理成功后清除标记");
    }

    #[tokio::test]
    async fn processor_ignores_bad_payload() {
        let temp = tempfile::tempdir().unwrap();
        let service = service(temp.path());
        let processor = RecordNotificationProcessor { service, kind: RecordKind::Think };
        let frame = WsMessage {
            sn: 0,
            payload_type: RecordKind::Think.payload_type(),
            status_code: kai_ws::CODE_SUCCESS,
            payload: Some(serde_json::json!({"unexpected": true})),
        };
        // 缺字段不 panic
        processor.process_json(frame, Arc::new(WsContext::new(4))).await;
    }

    #[tokio::test]
    async fn processor_without_payload_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let service = service(temp.path());
        let processor = RecordNotificationProcessor { service, kind: RecordKind::ToolCall };
        let frame = WsMessage {
            sn: 0,
            payload_type: RecordKind::ToolCall.payload_type(),
            status_code: kai_ws::CODE_SUCCESS,
            payload: None,
        };
        processor.process_json(frame, Arc::new(WsContext::new(4))).await;
    }

    #[tokio::test]
    async fn subscribe_without_url_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let service = service(temp.path());
        subscribe(service, None, 5).await.unwrap();
    }

    #[test]
    fn notification_ids_map_to_agent_role_and_event() {
        // 通知里的 agent/role_event 解析出「角色 + 事件标识」：AgentRole 决定 case 树，event_id 决定模式与事件 case
        let role_event = role_event("dev", Some("e1"));
        assert_eq!(AgentRole::new(ArcStr::from("0"), ArcStr::from(role_event.role_name.as_str())), AgentRole::new(ArcStr::from("0"), ArcStr::from("dev")), "dev-e1 属于角色 dev");
        assert_eq!(
            crate::tree::event_case_name(role_event.event_id.as_deref().unwrap()).as_str(),
            "event-e1"
        );
    }
}
