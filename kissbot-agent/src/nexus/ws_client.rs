use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use flume::{Receiver, Sender, bounded};
use futures_util::StreamExt;
use kai_ws::{
    WsContext, WsMessage, WsJsonProcessor,
    WsHeartbeatHandler, CODE_SUCCESS,
};
use serde_json::json;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tracing::{error, info, warn};

use crate::nexus::types::{Result, Error};
use kissbot_api::channel::{
    IncomingMessage, OutgoingMessage,
    TYPE_MESSENGER_INFO_REQUEST, TYPE_BIND_AGENT_USER,
    TYPE_INCOMING_MESSAGE, TYPE_OUTGOING_MESSAGE,
};

// ========== 转发到 Coordinator 的消息 ==========

#[derive(Debug)]
pub enum ExternalMessage {
    Incoming(IncomingMessage),
}

// ========== 连接上下文 ==========

struct ConnectionCtx {
    messenger_id: String,
    /// 上行消息 → Coordinator
    to_coordinator: Sender<ExternalMessage>,
}

#[async_trait]
impl WsJsonProcessor for ConnectionCtx {
    async fn process_json(&self, msg: WsMessage, _context: Arc<WsContext>) {
        match msg.payload_type {
            TYPE_MESSENGER_INFO_REQUEST | TYPE_BIND_AGENT_USER => {
                info!("通道 {} 响应: {:?}", self.messenger_id, msg);
            }
            TYPE_INCOMING_MESSAGE => {
                if let Some(payload) = msg.payload {
                    if let Ok(incoming) = serde_json::from_value::<IncomingMessage>(payload) {
                        if let Err(e) = self.to_coordinator.try_send(ExternalMessage::Incoming(incoming)) {
                            error!("转发上行消息失败: {:?}", e);
                        }
                    }
                }
            }
            _ => {
                // 其他响应类型
            }
        }
    }
}

// ========== WSClient ==========

pub struct WSClient {
    connections: Arc<DashMap<String, Arc<WsContext>>>,
    /// 上行消息 → Coordinator 的通道
    inbox_tx: Sender<ExternalMessage>,
}

impl WSClient {
    pub fn new() -> (Self, Receiver<ExternalMessage>) {
        let (in_tx, in_rx) = bounded(256);

        let client = Self {
            connections: Arc::new(DashMap::new()),
            inbox_tx: in_tx,
        };

        (client, in_rx)
    }

    /// 连接单个消息通道
    pub async fn connect_channel(
        &self,
        url: &str,
        messenger_id: &str,
        user_id: &str,
        agent_id: &str,
        role_name: &str,
    ) {
        let messenger_id = messenger_id.to_string();
        let user_id = user_id.to_string();
        let agent_id = agent_id.to_string();
        let role_name = role_name.to_string();
        let connections = self.connections.clone();
        let in_tx = self.inbox_tx.clone();

        loop {
            match Self::connect_inner(
                url, &messenger_id, &user_id, &agent_id, &role_name, &connections, &in_tx,
            ).await {
                Ok(ctx) => {
                    info!("已连接通道: {}", messenger_id);
                    connections.insert(messenger_id.clone(), ctx);
                    // 连接保持直到断开，断开后重连
                    // 断开后自动进入循环底部，等待重连
                }
                Err(e) => {
                    warn!("连接通道 {} 失败: {:?}，5秒后重连", messenger_id, e);
                }
            }
            sleep(Duration::from_secs(5)).await;
        }
    }

    async fn connect_inner(
        url: &str,
        messenger_id: &str,
        user_id: &str,
        agent_id: &str,
        role_name: &str,
        _connections: &DashMap<String, Arc<WsContext>>,
        in_tx: &Sender<ExternalMessage>,
    ) -> Result<Arc<WsContext>> {
        let (ws_stream, _) = connect_async(url).await
            .map_err(|e| Error::WsConnectionError(e.to_string()))?;

        let context = Arc::new(WsContext::new(64));

        // 注册消息处理器
        let processor = Arc::new(ConnectionCtx {
            messenger_id: messenger_id.to_string(),
            to_coordinator: in_tx.clone(),
        });

        // 注册 JSON 消息处理器
        context.set_json_processor(TYPE_MESSENGER_INFO_REQUEST, processor.clone());
        context.set_json_processor(TYPE_BIND_AGENT_USER, processor.clone());
        context.set_json_processor(TYPE_INCOMING_MESSAGE, processor);

        // 心跳
        let heartbeat = Arc::new(WsHeartbeatHandler::new(
            Duration::from_secs(10),
            context.clone(),
        ));
        context.set_bin_processor(kai_ws::TYPE_HEARTBEAT, heartbeat.clone());
        tokio::spawn(async move {
            let _ = heartbeat.start().await;
        });

        // 发送 MessengerInfo 请求
        let info_req = WsMessage {
            sn: context.next_request_sn(),
            payload_type: TYPE_MESSENGER_INFO_REQUEST,
            status_code: CODE_SUCCESS,
            payload: Some(json!({
                "messenger_id": messenger_id,
            })),
        };
        context.send_json(info_req).await?;

        // 短暂等待后发送 BindRequest
        sleep(Duration::from_millis(100)).await;

        let bind_req = WsMessage {
            sn: context.next_request_sn(),
            payload_type: TYPE_BIND_AGENT_USER,
            status_code: CODE_SUCCESS,
            payload: Some(json!({
                "agent_id": agent_id,
                "role_name": role_name,
                "messenger_id": messenger_id,
                "user_id": user_id,
            })),
        };
        context.send_json(bind_req).await?;

        // 保持 WebSocket 连接存活：拆分流并生成后台任务持有 writer
        let (ws_writer, _ws_reader) = ws_stream.split();
        tokio::spawn(async move {
            // 持有 ws_writer 防止连接关闭，永不返回
            let _keep = ws_writer;
            std::future::pending::<()>().await;
        });

        Ok(context)
    }

    /// 发送下行消息到指定通道
    pub async fn send_reply(&self, messenger_id: &str, msg: OutgoingMessage) -> Result<()> {
        if let Some(ctx) = self.connections.get(messenger_id) {
            let ws_msg = WsMessage {
                sn: ctx.next_request_sn(),
                payload_type: TYPE_OUTGOING_MESSAGE,
                status_code: CODE_SUCCESS,
                payload: Some(serde_json::to_value(msg).map_err(|e| Error::SerializationError(e.to_string()))?),
            };
            ctx.send_json(ws_msg).await?;
        }
        Ok(())
    }
}
