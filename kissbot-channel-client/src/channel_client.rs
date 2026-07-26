use std::sync::{Arc, RwLock, Weak};
use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use dashmap::DashMap;
use kai_ws::*;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_security::HEADER_API_KEY;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tracing::error;

use crate::error::{Error, Result};
use crate::terminal::*;

const QUEUE_SIZE: usize = 100;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ChannelClient {
    ws_context: RwLock<Option<Arc<WsContext>>>,
    terminal: RwLock<Option<Arc<dyn Terminal>>>,
    // 下载方向：transfer_id → 下载头信息
    download_transfer_map: DashMap<u32, Arc<AttachmentInfoResponse>>,
}

/// 通用 JSON 请求-响应处理器：收到响应后经 oneshot 返回
struct JsonResponseHandler {
    tx: Option<oneshot::Sender<kai_ws::Result<WsMessage>>>,
}

#[async_trait]
impl WsJsonProcessorMut for JsonResponseHandler {
    async fn process_json(mut self: Box<Self>, data: WsMessage, _context: Arc<WsContext>) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Ok(data));
        }
    }
}

impl ChannelClient {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ws_context: RwLock::new(None),
            terminal: RwLock::new(None),
            download_transfer_map: DashMap::new(),
        })
    }

    fn get_ws_context(&self) -> Result<Arc<WsContext>> {
        self.ws_context.read().unwrap().clone().ok_or(Error::NotConnected)
    }

    fn get_terminal(&self) -> Result<Arc<dyn Terminal>> {
        self.terminal.read().unwrap().clone()
            .ok_or_else(|| Error::InternalError("terminal is None".to_string()))
    }

    /// 连接 channel 的 ws 服务：创建 Terminal、注入 handler、建立连接。
    pub async fn connect<T, TC>(self: &Arc<Self>, url: &str, api_key: &str, creator: TC) -> Result<Arc<T>>
    where
        T: Terminal,
        TC: TerminalCreator<T>,
    {
        let bind_handler: Arc<dyn BindHandler> = self.clone();
        let messenger_info_handler: Arc<dyn MessengerInfoHandler> = self.clone();
        let outgoing_message_handler: Arc<dyn OutgoingMessageHandler> = self.clone();
        let attachment_upload_handler: Arc<dyn AttachmentUploadHandler> = self.clone();
        let attachment_download_handler: Arc<dyn AttachmentDownloadHandler> = self.clone();
        let terminal = creator.create(
            Arc::downgrade(&bind_handler),
            Arc::downgrade(&messenger_info_handler),
            Arc::downgrade(&outgoing_message_handler),
            Arc::downgrade(&attachment_upload_handler),
            Arc::downgrade(&attachment_download_handler),
        ).await?;
        *self.terminal.write().unwrap() = Some(terminal.clone());

        let headers = [(HEADER_API_KEY.to_string(), api_key.to_string())];
        kai_ws::ws_connect(url, &headers, QUEUE_SIZE, self.clone(), &ChannelClientInitializer).await?;
        Ok(terminal)
    }

    /// 主动断开连接
    pub async fn disconnect(&self) -> Result<()> {
        self.get_ws_context()?.send_close().await?;
        Ok(())
    }

    /// 发送 JSON 请求并等待响应（带超时）
    async fn request_json(&self, payload_type: u32, payload: serde_json::Value) -> Result<Option<serde_json::Value>> {
        let context = self.get_ws_context()?;
        let (tx, rx) = oneshot::channel();
        let msg = WsMessage {
            sn: context.next_request_sn(),
            payload_type,
            status_code: CODE_SUCCESS,
            payload: Some(payload),
        };
        context.send_json_with_json_response(msg, Box::new(JsonResponseHandler { tx: Some(tx) })).await?;
        let response = tokio::time::timeout(REQUEST_TIMEOUT, rx).await
            .map_err(|_| Error::Timeout(format!("request type {:#x} timeout", payload_type)))?
            .map_err(|_| Error::InternalError("response channel closed".to_string()))?
            .map_err(|e| Error::WsError(e))?;
        if response.status_code != CODE_SUCCESS {
            return Err(Error::ResponseError(response.status_code));
        }
        Ok(response.payload)
    }
}

#[async_trait]
impl BindHandler for ChannelClient {
    async fn bind(&self, request: BindRequest) -> Result<()> {
        self.request_json(TYPE_BIND_AGENT_USER, serde_json::to_value(request)?).await?;
        Ok(())
    }

    async fn unbind(&self, request: BindRequest) -> Result<()> {
        self.request_json(TYPE_UNBIND_AGENT_USER, serde_json::to_value(request)?).await?;
        Ok(())
    }
}

#[async_trait]
impl MessengerInfoHandler for ChannelClient {
    async fn get_info(&self, messenger_id: Arc<String>) -> Result<Arc<MessengerInfo>> {
        let payload = self.request_json(
            TYPE_MESSENGER_INFO_REQUEST,
            serde_json::to_value(MessengerInfoRequest { messenger_id })?,
        ).await?
        .ok_or_else(|| Error::InvalidMessage("messenger info response payload is None".to_string()))?;
        Ok(Arc::new(serde_json::from_value(payload)?))
    }
}

#[async_trait]
impl OutgoingMessageHandler for ChannelClient {
    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>> {
        let payload = self.request_json(TYPE_OUTGOING_MESSAGE, serde_json::to_value(message)?).await?
            .ok_or_else(|| Error::InvalidMessage("outgoing message response payload is None".to_string()))?;
        Ok(Arc::new(serde_json::from_value(payload)?))
    }
}

#[async_trait]
impl AttachmentUploadHandler for ChannelClient {
    async fn send_upload_chunk(&self, transfer_id: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse> {
        let context = self.get_ws_context()?;
        let sn = context.next_request_sn();
        // 二进制帧：sn + payload_type + status_code + transfer_id + size + pos + data
        let mut buf = BytesMut::with_capacity(OFFSET_ATT_DATA + data.len());
        buf.put_u32(sn);
        buf.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        buf.put_u32(CODE_SUCCESS);
        buf.put_u32(transfer_id);
        buf.put_u32(data.len() as u32);
        buf.put_u64(pos);
        buf.extend_from_slice(&data);
        let (tx, rx) = oneshot::channel();
        context.send_bin_with_json_response(sn, buf.freeze(), Box::new(JsonResponseHandler { tx: Some(tx) })).await?;
        let response = tokio::time::timeout(REQUEST_TIMEOUT, rx).await
            .map_err(|_| Error::Timeout("upload chunk timeout".to_string()))?
            .map_err(|_| Error::InternalError("response channel closed".to_string()))?
            .map_err(|e| Error::WsError(e))?;
        if response.status_code != CODE_SUCCESS {
            return Err(Error::ResponseError(response.status_code));
        }
        let payload = response.payload
            .ok_or_else(|| Error::InvalidMessage("upload chunk response payload is None".to_string()))?;
        Ok(serde_json::from_value(payload)?)
    }
}

#[async_trait]
impl AttachmentDownloadHandler for ChannelClient {
    async fn request_download(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>> {
        let payload = self.request_json(TYPE_ATTACHMENT_DOWNLOAD_REQUEST, serde_json::to_value(request)?).await?
            .ok_or_else(|| Error::InvalidMessage("download response payload is None".to_string()))?;
        let info = Arc::new(serde_json::from_value::<AttachmentInfoResponse>(payload)?);
        // 注册 transfer 映射，之后服务端推送的分块按 transfer_id 找到下载头
        self.download_transfer_map.insert(info.transfer_id, info.clone());
        Ok(info)
    }
}

/// 服务端推送的 JSON 消息（上行消息、群组变化、用户删除）转接到 Terminal
struct TerminalJsonProcessor {
    client: Weak<ChannelClient>,
}

#[async_trait]
impl WsJsonProcessor for TerminalJsonProcessor {
    async fn process_json(&self, data: WsMessage, _context: Arc<WsContext>) {
        let Some(client) = self.client.upgrade() else { return };
        let Ok(terminal) = client.get_terminal() else { return };
        let Some(payload) = data.payload else {
            error!("TerminalJsonProcessor: payload is None, type {:#x}", data.payload_type);
            return;
        };
        match data.payload_type {
            TYPE_INCOMING_MESSAGE => match serde_json::from_value::<IncomingMessage>(payload) {
                Ok(m) => terminal.incoming_message(Arc::new(m)).await,
                Err(e) => error!("parse incoming message error: {:?}", e),
            },
            TYPE_JOIN_GROUP => match serde_json::from_value::<GroupChangeNotification>(payload) {
                Ok(n) => terminal.join_group(Arc::new(n)).await,
                Err(e) => error!("parse join group error: {:?}", e),
            },
            TYPE_LEAVE_GROUP => match serde_json::from_value::<GroupChangeNotification>(payload) {
                Ok(n) => terminal.leave_group(Arc::new(n)).await,
                Err(e) => error!("parse leave group error: {:?}", e),
            },
            TYPE_USER_REMOVED => match serde_json::from_value::<UserRemoveNotification>(payload) {
                Ok(n) => terminal.user_removed(Arc::new(n)).await,
                Err(e) => error!("parse user removed error: {:?}", e),
            },
            _ => {}
        }
    }
}

/// 连接关闭时通知 Terminal
struct TerminalCloseProcessor {
    client: Weak<ChannelClient>,
}

#[async_trait]
impl WsCloseProcessor for TerminalCloseProcessor {
    async fn process_close(&self, _context: Arc<WsContext>) {
        let Some(client) = self.client.upgrade() else { return };
        if let Ok(terminal) = client.get_terminal() {
            terminal.closed().await;
        }
    }
}

const TRANSFER_WAIT_INTERVAL: Duration = Duration::from_millis(50);
const TRANSFER_WAIT_MAX: Duration = Duration::from_secs(2);

/// 下载分块处理器：服务端推送的附件分块转接到 Terminal::download_chunk，并按结果确认
struct DownloadChunkProcessor {
    client: Weak<ChannelClient>,
}

impl DownloadChunkProcessor {
    // 下载头响应与首个分块的派发存在并发竞争（kai-ws 按消息 spawn 处理任务），
    // transfer 映射可能尚未注册，此处做有限等待
    async fn wait_transfer_info(client: &ChannelClient, transfer_id: u32) -> Option<Arc<AttachmentInfoResponse>> {
        let deadline = tokio::time::Instant::now() + TRANSFER_WAIT_MAX;
        loop {
            if let Some(info) = client.download_transfer_map.get(&transfer_id) {
                return Some(info.clone());
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(TRANSFER_WAIT_INTERVAL).await;
        }
    }
}

#[async_trait]
impl WsBinaryProcessor for DownloadChunkProcessor {
    async fn process_bin(&self, data: Bytes, context: Arc<WsContext>) {
        let sn = match parse_bin_sn(data.as_ref()) {
            Ok(sn) => sn,
            Err(e) => {
                error!("download chunk parse sn error: {:?}", e);
                return;
            }
        };
        let Some(client) = self.client.upgrade() else { return };
        let header = match parse_attachment_payload_header(data.as_ref()) {
            Ok(h) => h,
            Err(e) => {
                error!("download chunk parse header error: {:?}", e);
                return;
            }
        };
        // 检查 payload_type 是否为下载分块（而非心跳等其他二进制消息）
        let info = Self::wait_transfer_info(&client, header.id).await;
        let result = match &info {
            Some(info) => match client.get_terminal() {
                Ok(terminal) => terminal.download_chunk(info.clone(), header.pos, data.slice(OFFSET_ATT_DATA..)).await,
                Err(e) => Err(e),
            },
            None => Err(Error::InvalidMessage(format!("unknown transfer_id {}", header.id))),
        };
        let response = match &result {
            Ok(()) => AttachmentPayloadResponse {
                current_pos: header.pos + header.size as u64,
                error_code: PAYLOAD_ERRCODE_OK,
                error_msg: None,
            },
            Err(e) => AttachmentPayloadResponse {
                current_pos: header.pos,
                error_code: 1,
                error_msg: Some(Arc::new(e.to_string())),
            },
        };
        // 最后一块或出错时清理映射
        if let Some(info) = &info {
            if result.is_err() || header.pos + header.size as u64 >= info.info.size_bytes {
                client.download_transfer_map.remove(&header.id);
            }
        }
        let payload = match serde_json::to_value(&response) {
            Ok(p) => Some(p),
            Err(e) => {
                error!("serialize download chunk response error: {:?}", e);
                None
            }
        };
        if let Err(e) = context.send_json(WsMessage {
            sn,
            status_code: CODE_SUCCESS,
            payload_type: TYPE_RESPONSE,
            payload,
        }).await {
            error!("send download chunk response error: {:?}", e);
        }
    }
}

struct ChannelClientInitializer;

#[async_trait]
impl WsProcessorInitializer<ChannelClient> for ChannelClientInitializer {
    async fn init(&self, ws_context: Arc<WsContext>, client: Arc<ChannelClient>) -> std::result::Result<(), kai_ws::Error> {
        *client.ws_context.write().unwrap() = Some(ws_context.clone());
        // 心跳
        let heartbeat = Arc::new(WsHeartbeatHandler::new(HEARTBEAT_INTERVAL, ws_context.clone()));
        ws_context.set_bin_processor(TYPE_HEARTBEAT, heartbeat.clone());
        tokio::spawn(async move { let _ = heartbeat.start().await; });
        // 关闭通知
        ws_context.set_close_processor(Arc::new(TerminalCloseProcessor { client: Arc::downgrade(&client) }));
        // 服务端推送的 JSON 消息
        let json_processor = Arc::new(TerminalJsonProcessor { client: Arc::downgrade(&client) });
        ws_context.set_json_processor(TYPE_INCOMING_MESSAGE, json_processor.clone());
        ws_context.set_json_processor(TYPE_JOIN_GROUP, json_processor.clone());
        ws_context.set_json_processor(TYPE_LEAVE_GROUP, json_processor.clone());
        ws_context.set_json_processor(TYPE_USER_REMOVED, json_processor);
        // 下载分块
        ws_context.set_bin_processor(TYPE_ATTACHMENT_PAYLOAD, Arc::new(DownloadChunkProcessor { client: Arc::downgrade(&client) }));
        Ok(())
    }
}
