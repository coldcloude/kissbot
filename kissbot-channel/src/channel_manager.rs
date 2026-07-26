use crate::{Error, error::Result};
use crate::messenger::{Messenger, MessengerCreator};
use crate::data::*;
use crate::memory_store_client::MemoryStoreClient;
use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use dashmap::{DashMap, Entry};
use kai_ws::{CODE_ERROR, CODE_SUCCESS, TYPE_HEARTBEAT, TYPE_RESPONSE, WsBinaryProcessor, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsJsonProcessor, WsJsonProcessorMut, WsMessage, WsProcessorInitializer, parse_bin_sn, ws_handle_connection_with_filter};
use kissbot_api::{IncomingMessage, TYPE_ATTACHMENT_DOWNLOAD_REQUEST, TYPE_ATTACHMENT_PAYLOAD, parse_attachment_payload_header};
use kissbot_api::channel::{AttachmentDownloadRequest, BindRequest, AttachmentPayloadResponse, MessengerInfoRequest, OFFSET_ATT_DATA, OutgoingMessage, TYPE_BIND_AGENT_USER, TYPE_INCOMING_MESSAGE, TYPE_JOIN_GROUP, TYPE_LEAVE_GROUP, TYPE_MESSENGER_INFO_REQUEST, TYPE_OUTGOING_MESSAGE, TYPE_UNBIND_AGENT_USER, TYPE_USER_REMOVED};
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content};
use tokio::sync::oneshot::Sender;
use tracing::{Level, error, info, span};
use std::sync::{Arc, Weak, atomic::{AtomicU32, Ordering}};
use tokio::{net::TcpListener, time::{Duration}};

const MSG_QUEUE_SIZE: usize = 100;

static INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct BoundInfo {
    pub connect_id: u32,
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
}

struct MessengerContext {
    pub messenger: Arc<dyn Messenger>,
    pub bound_map: DashMap<String, BoundInfo>,
}

struct ConnectContext {
    connect_id: u32,
    ws_context: Arc<WsContext>,
}

struct AttachmentReceiverContext {
    pub messenger: Weak<dyn Messenger>,
    pub info: Arc<AttachmentInfo>,
}

struct AttachmentSenderContext {
    pub connect_context: Weak<ConnectContext>,
    pub info: Arc<AttachmentInfo>,
}

pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
    memory_store_client: Arc<MemoryStoreClient>,
    // 上传方向：transfer_id → AttachmentReceiverContext
    attachment_receiver_map: DashMap<u32, AttachmentReceiverContext>,
    // 下载方向：transfer_id → AttachmentSenderContext
    attachment_sender_map: DashMap<u32, AttachmentSenderContext>,
}

struct ConnectCloseProcessor {
    manager: Weak<ChannelManager>,
    connect_id: u32,
}

impl ConnectCloseProcessor {
    pub fn close_connect(&self) -> Result<()> {
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        //移除连接记录
        let (_,connect_context) = manager.connect_map.remove(&self.connect_id)
        .ok_or_else(|| Error::ConnectNotFound(self.connect_id))?;
        //把绑定记录从messenger中移除
        for messenger_context in manager.messenger_map.iter() {
            //为防止死锁，将遍历和删除分开
            let mut finished = false;
            while !finished {
                let mut candidate_user_ids = Vec::new();
                for bound_info in messenger_context.bound_map.iter() {
                    if bound_info.connect_id == connect_context.connect_id {
                        candidate_user_ids.push(bound_info.key().clone());
                    }
                }
                finished = candidate_user_ids.is_empty();
                if !finished {
                    for user_id in candidate_user_ids.iter() {
                        messenger_context.bound_map.remove(user_id);
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl WsCloseProcessor for ConnectCloseProcessor {
    async fn process_close(&self, _: Arc<WsContext>) {
        if let Err(e) = self.close_connect() {
            error!("Close connect error: {}", e);
        }
    }
}

#[async_trait]
trait JsonProcessorWrapper {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>>;
    
    async fn wrap_process_json(&self, data: WsMessage, context: Arc<WsContext>) -> Result<()> {
        let sn = data.sn;
        match self.raw_process_json(data).await {
            Ok(payload) => {
                context.send_json(WsMessage {
                    sn,
                    status_code: CODE_SUCCESS,
                    payload_type: TYPE_RESPONSE,
                    payload,
                }).await?;
                Ok(())
            }
            Err(e) => {
                context.send_json(WsMessage {
                    sn,
                    status_code: CODE_ERROR,
                    payload_type: TYPE_RESPONSE,
                    payload: None,
                }).await?;
                Err(e)
            }
        }
    }
}

#[async_trait]
trait BinaryProcessorWrapper {
    async fn raw_process_bin(&self, data: Bytes) -> Result<Option<serde_json::Value>>;
    
    async fn wrap_process_bin(&self, data: Bytes, context: Arc<WsContext>) -> Result<()> {
        let sn = parse_bin_sn(data.as_ref())?;
        match self.raw_process_bin(data).await {
            Ok(payload) => {
                context.send_json(WsMessage {
                    sn,
                    status_code: CODE_SUCCESS,
                    payload_type: TYPE_RESPONSE,
                    payload,
                }).await?;
                Ok(())
            }
            Err(e) => {
                context.send_json(WsMessage {
                    sn,
                    status_code: CODE_ERROR,
                    payload_type: TYPE_RESPONSE,
                    payload: None,
                }).await?;
                Err(e)
            }
        }
    }
}

struct MessengerInfoRequestProcessor {
    manager: Weak<ChannelManager>,
}

#[async_trait]
impl JsonProcessorWrapper for MessengerInfoRequestProcessor {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
        let payload = data.payload
        .ok_or_else(|| Error::InvalidMessage("payload is None".to_string()))?;
        
        let messenger_info_request = serde_json::from_value::<MessengerInfoRequest>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        
        let messenger_context = manager.messenger_map.get(messenger_info_request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(messenger_info_request.messenger_id.to_string()))?;
        
        let messenger_info = messenger_context.messenger.get_info().await?;

        let response = serde_json::to_value(messenger_info)?;
        Ok(Some(response))
    }
}

#[async_trait]
impl WsJsonProcessor for MessengerInfoRequestProcessor {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_json(data, context).await {
            error!("Messenger info request error: {:?}", e);
        }
    }
}

struct BindAgentUserProcessor {
    manager: Weak<ChannelManager>,
    connect_context: Weak<ConnectContext>,
}

#[async_trait]
impl JsonProcessorWrapper for BindAgentUserProcessor {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
        let payload = data.payload
        .ok_or_else(|| Error::InvalidMessage("payload is None".to_string()))?;
        
        let bind_request = serde_json::from_value::<BindRequest>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        let connect_context = self.connect_context.upgrade()
        .ok_or_else(|| Error::InternalError("connect_context is None".to_string()))?;

        let agent_id = bind_request.agent_id;
        let role_name = bind_request.role_name;

        let messenger_context = manager.messenger_map.get(bind_request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(bind_request.messenger_id.to_string()))?;
        
        let messenger_info = messenger_context.messenger.get_info().await?;

        if !messenger_info.user_map.contains_key(bind_request.user_id.as_str()) {
            return Err(Error::UserNotFound(bind_request.user_id.to_string()));
        }

        //绑定用户
        let bound_info = messenger_context.bound_map.entry(bind_request.user_id.to_string()).or_insert_with(|| BoundInfo {
            connect_id: connect_context.connect_id,
            agent_id: agent_id.clone(),
            role_name: role_name.clone(),
        });
        
        if bound_info.connect_id != connect_context.connect_id {
            return Err(Error::UserAlreadyBound(bound_info.connect_id.to_string()));
        }

        Ok(None)
    }
}

#[async_trait]
impl WsJsonProcessor for BindAgentUserProcessor {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_json(data, context).await {
            error!("bind_agent_user error: {:?}", e);
        }
    }
}

struct UnbindAgentUserProcessor {
    manager: Weak<ChannelManager>,
    connect_context: Weak<ConnectContext>,
}

#[async_trait]
impl JsonProcessorWrapper for UnbindAgentUserProcessor {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
        let payload = data.payload
        .ok_or_else(|| Error::InvalidMessage("payload is None".to_string()))?;
        
        let bind_request = serde_json::from_value::<BindRequest>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        let connect_context = self.connect_context.upgrade()
        .ok_or_else(|| Error::InternalError("connect_context is None".to_string()))?;

        //解除绑定
        let messenger_context = manager.messenger_map.get(bind_request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(bind_request.messenger_id.to_string()))?;

        //检查是否为本连接绑定（先取出 connect_id 再释放 Ref，后续 remove 避免 DashMap 死锁）
        let bound_connect_id = {
            let bound_info = messenger_context.bound_map.get(bind_request.user_id.as_str())
                .ok_or_else(|| Error::UserNotBound(bind_request.user_id.to_string()))?;
            bound_info.connect_id
        };
        if bound_connect_id != connect_context.connect_id {
            return Err(Error::UserAlreadyBound(bound_connect_id.to_string()));
        }

        //解除绑定
        messenger_context.bound_map.remove(bind_request.user_id.as_str());

        Ok(None)
    }
}

#[async_trait]
impl WsJsonProcessor for UnbindAgentUserProcessor {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_json(data, context).await {
            error!("unbind_agent_user error: {:?}", e);
        }
    }
}

struct OutgoingMessageProcessor {
    connect_id: u32,
    manager: Weak<ChannelManager>,
}

/// 遍历 content 中的 AttachmentInfoResponse，注册已有的 transfer_id 到 receiver_map
fn register_attachment_receivers(
    content: &Content,
    manager: &ChannelManager,
    messenger: Weak<dyn Messenger>,
) {
    match content {
        Content::AttachmentInfoResponse(resp) => {
            manager.attachment_receiver_map.insert(resp.transfer_id, AttachmentReceiverContext {
                messenger: messenger.clone(),
                info: resp.info.clone(),
            });
        }
        Content::Multi(items) => {
            for item in items.iter() {
                register_attachment_receivers(&item.content, manager, messenger.clone());
            }
        }
        _ => {}
    }
}

#[async_trait]
impl JsonProcessorWrapper for OutgoingMessageProcessor {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
        let payload = data.payload
        .ok_or_else(|| Error::InvalidMessage("payload is None".to_string()))?;

        let outgoing_message = serde_json::from_value::<OutgoingMessage>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;

        let messenger_context = manager.messenger_map.get(outgoing_message.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(outgoing_message.messenger_id.to_string()))?;

        let bound_info = messenger_context.bound_map.get(outgoing_message.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(outgoing_message.user_id.to_string()))?;

        if bound_info.connect_id != self.connect_id {
            return Err(Error::UserNotBound(outgoing_message.user_id.to_string()));
        }

        let response = messenger_context.messenger.send_message(outgoing_message).await?;

        // 遍历 content 中的 AttachmentInfoResponse，注册 transfer_id 到 receiver_map
        let messenger_weak = Arc::downgrade(&messenger_context.messenger);
        register_attachment_receivers(&response.content, &manager, messenger_weak);

        let response = serde_json::to_value(response)?;
        Ok(Some(response))
    }
}

#[async_trait]
impl WsJsonProcessor for OutgoingMessageProcessor {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_json(data, context).await {
            error!("outgoing_message error: {:?}", e);
        }
    }
}

struct AttachmentPayloadProcessor {
    manager: Weak<ChannelManager>,
}

#[async_trait]
impl BinaryProcessorWrapper for AttachmentPayloadProcessor {
    async fn raw_process_bin(&self, data: Bytes) -> Result<Option<serde_json::Value>> {
        let header = parse_attachment_payload_header(data.as_ref())?;

        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;

        // 通过 transfer_id 找到 messenger 和 info
        let receiver = manager.attachment_receiver_map.get(&header.id)
            .ok_or_else(|| Error::AttachmentNotFound(header.id.to_string()))?;
        let messenger = receiver.messenger.upgrade()
            .ok_or_else(|| Error::InternalError("messenger is None".to_string()))?;
        let file_size = receiver.info.size_bytes;
        drop(receiver);

        let mut success = false;
        let payload = data.slice(OFFSET_ATT_DATA..);
        let result = match messenger.send_attachment_payload(header.id, header.size, header.pos, payload).await {
            Ok(response) => {
                match serde_json::to_value(&response) {
                    Ok(value) => {
                        success = response.error_code == 0;
                        Ok(Some(value))
                    }
                    Err(e) => Err(Error::from(e))
                }
            }
            Err(e) => Err(e)
        };

        // 根据 pos+size 判断是否最后一块，或错误时清理
        if header.pos as u64 + header.size as u64 >= file_size || !success {
            manager.attachment_receiver_map.remove(&header.id);
        }

        result
    }
}

#[async_trait]
impl WsBinaryProcessor for AttachmentPayloadProcessor {
    async fn process_bin(&self, data: Bytes, context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_bin(data, context).await {
            error!("attachment_payload error: {:?}", e);
        }
    }
}

struct DownloadResponseHandler {
    response_tx: Sender<Result<AttachmentPayloadResponse>>,
}

impl DownloadResponseHandler {
    fn parse_response(&self, data: WsMessage) -> Result<AttachmentPayloadResponse> {
        let payload = data.payload
        .ok_or_else(|| Error::ReponseError("download response payload is None".to_string()))?;
        let response = serde_json::from_value::<AttachmentPayloadResponse>(payload)?;
        Ok(response)
    }
}

#[async_trait]
impl WsJsonProcessorMut for DownloadResponseHandler {
    async fn process_json(mut self: Box<Self>, data: WsMessage, _context: Arc<WsContext>) {
        let result = self.parse_response(data);
        let _ = self.response_tx.send(result);
    }
}

struct AttachmentDownloadRequestProcessor {
    connect_context: Weak<ConnectContext>,
    manager: Weak<ChannelManager>,
}

impl AttachmentDownloadRequestProcessor {
    async fn process_download_request_header(&self, data: WsMessage) -> Result<(Arc<AttachmentInfoResponse>, Arc<dyn Messenger>)> {
        let payload = data.payload
            .ok_or_else(|| Error::InvalidMessage("payload is None".to_string()))?;
        let request = serde_json::from_value::<AttachmentDownloadRequest>(payload)?;

        let manager = self.manager.upgrade()
            .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        let connect_context = self.connect_context.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

        let messenger_context = manager.messenger_map.get(request.messenger_id.as_str())
            .ok_or_else(|| Error::MessengerNotFound(request.messenger_id.to_string()))?;

        let bound_info = messenger_context.bound_map.get(request.user_id.as_str())
            .ok_or_else(|| Error::UserNotFound(request.user_id.to_string()))?;
        if bound_info.connect_id != connect_context.connect_id {
            return Err(Error::UserNotBound(request.user_id.to_string()));
        }
        let messenger = messenger_context.messenger.clone();

        let att_info_response = messenger.download_attachment_header(request).await?;
        let transfer_id = att_info_response.transfer_id;
        manager.attachment_sender_map.insert(transfer_id, AttachmentSenderContext {
            connect_context: Arc::downgrade(&connect_context),
            info: att_info_response.info.clone(),
        });

        Ok((att_info_response, messenger))
    }

    fn stop_download(&self, transfer_id: u32) -> Result<()> {
        let manager = self.manager.upgrade()
            .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        manager.attachment_sender_map.remove(&transfer_id);
        Ok(())
    }
}

#[async_trait]
impl WsJsonProcessor for AttachmentDownloadRequestProcessor {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>) {
        let sn = data.sn;
        let mut result: Option<Error> = None;
        match self.process_download_request_header(data).await {
            Ok((response,messenger)) => {
                let mut send_succeed = false;
                match serde_json::to_value(&response) {
                    Ok(response_header_value) => {
                        // 先返回 header，再启动 payload 推送
                        match context.send_json(WsMessage {
                            sn: sn,
                            status_code: CODE_SUCCESS,
                            payload_type: TYPE_RESPONSE,
                            payload: Some(response_header_value),
                        }).await {
                            Ok(_) => {
                                match messenger.start_send_download_attachment_payload(response.transfer_id).await {
                                    Ok(_) => {
                                        send_succeed = true;
                                    }
                                    Err(e) => {
                                        error!("start_send_download_attachment_payload error: {:?}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                error!("attachment_download_request {} error: {:?}", response.transfer_id, e);
                            }
                        };
                    }
                    Err(e) => {
                        result = Some(Error::JsonError(e));
                    }
                }
                if !send_succeed {
                    if let Err(e) = self.stop_download(response.transfer_id) {
                        error!("stop_download {} error: {:?}", response.transfer_id, e);
                    }
                }
            }
            Err(e) => {
                result = Some(e);
            }
        };
        if let Some(e) = result {
            let _ = context.send_json(WsMessage {
                sn,
                status_code: CODE_ERROR,
                payload_type: TYPE_RESPONSE,
                payload: None,
            }).await;
            error!("attachment_download_request error: {:?}", e);
        }
    }
}

struct ChannelManagerInitializer;

#[async_trait]
impl WsProcessorInitializer<ChannelManager> for ChannelManagerInitializer {
    async fn init(&self, ws_context: Arc<WsContext>, manager: Arc<ChannelManager>) -> std::result::Result<(), kai_ws::Error> {
        //保存context
        let connect_id = manager.global_connect_id.fetch_add(1, Ordering::Relaxed);
        let connect_context = Arc::new(ConnectContext {
            connect_id,
            ws_context: ws_context.clone(),
        });
        manager.connect_map.insert(connect_id, connect_context.clone());
        //处理心跳
        let heartbeat_handler = Arc::new(WsHeartbeatHandler::new(INTERVAL, ws_context.clone()));
        ws_context.set_bin_processor(TYPE_HEARTBEAT, heartbeat_handler.clone());
        tokio::spawn(async move { heartbeat_handler.start().await });
        //处理关闭
        let close_handler = Arc::new(ConnectCloseProcessor {
            manager: Arc::downgrade(&manager),
            connect_id,
        });
        ws_context.set_close_processor(close_handler);
        //messenger info request
        let messenger_info_request_handler = Arc::new(MessengerInfoRequestProcessor {
            manager: Arc::downgrade(&manager),
        });
        ws_context.set_json_processor(TYPE_MESSENGER_INFO_REQUEST, messenger_info_request_handler);
        //agent绑定
        let bind_agent_handler = Arc::new(BindAgentUserProcessor {
            manager: Arc::downgrade(&manager),
            connect_context: Arc::downgrade(&connect_context),
        });
        ws_context.set_json_processor(TYPE_BIND_AGENT_USER, bind_agent_handler);
        //agent解绑
        let unbind_agent_handler = Arc::new(UnbindAgentUserProcessor {
            manager: Arc::downgrade(&manager),
            connect_context: Arc::downgrade(&connect_context),
        });
        ws_context.set_json_processor(TYPE_UNBIND_AGENT_USER, unbind_agent_handler);
        //outgoing message
        let outgoing_message_handler = Arc::new(OutgoingMessageProcessor {
            connect_id,
            manager: Arc::downgrade(&manager),
        });
        ws_context.set_json_processor(TYPE_OUTGOING_MESSAGE, outgoing_message_handler);
        //attachment payload
        let attachment_payload_handler = Arc::new(AttachmentPayloadProcessor {
            manager: Arc::downgrade(&manager),
        });
        ws_context.set_bin_processor(TYPE_ATTACHMENT_PAYLOAD, attachment_payload_handler);
        //attachment download request
        let attachment_download_request_handler = Arc::new(AttachmentDownloadRequestProcessor {
            connect_context: Arc::downgrade(&connect_context),
            manager: Arc::downgrade(&manager),
        });
        ws_context.set_json_processor(TYPE_ATTACHMENT_DOWNLOAD_REQUEST, attachment_download_request_handler);
        Ok(())

    }
}

impl ChannelManager {
    pub fn new() -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
            messenger_map: DashMap::new(),
            memory_store_client: Arc::new(MemoryStoreClient::new()),
            attachment_receiver_map: DashMap::new(),
            attachment_sender_map: DashMap::new(),
        }
    }

    pub async fn start(self: &Arc<Self>, addr: &str) -> Result<()> {
        //start ws server
        let span = span!(Level::INFO, "ws server start");
        let _enter = span.enter();
        let listener = TcpListener::bind(addr).await?;
        info!("WS Server listening on: {}", addr);
        let initializer = ChannelManagerInitializer {};
        let filter = kissbot_security::ApiKeyWsFilter::new(std::sync::Arc::new(kissbot_security::SimpleApiKeyValidator::new(kissbot_security::SecurityConfig::get().api_key.clone())));
        while let Ok((stream, _)) = listener.accept().await {
            ws_handle_connection_with_filter(stream, MSG_QUEUE_SIZE, self.clone(), &initializer, &[&filter]).await?;
        }
        Ok(())
    }
    
    pub async fn register_messenger<M,MC>(self: &Arc<Self>, messenger_id: &str, messenger_creator: MC) -> Result<Arc<M>>
    where
        M: Messenger,
        MC: MessengerCreator<M>
    {
        match self.messenger_map.entry(messenger_id.to_string()) {
            Entry::Vacant(entry) => {
                let group_change_handler = Arc::downgrade(self);
                let incoming_messages_handler = Arc::downgrade(self);
                let download_attachment_payload_handler = Arc::downgrade(self);
                let user_remove_handler = Arc::downgrade(self);
                let messenger = messenger_creator.create(
                    group_change_handler,
                    incoming_messages_handler,
                    download_attachment_payload_handler,
                    user_remove_handler,
                ).await?;
                let messenger_context = Arc::new(MessengerContext {
                    messenger: messenger.clone() as Arc<dyn Messenger>,
                    bound_map: DashMap::new(),
                });
                entry.insert(messenger_context);
                Ok(messenger)
            }
            Entry::Occupied(entry) => {
                Err(Error::MessengerAlreadyRegistered(entry.key().to_string()))
            }
        }
    }

    async fn handle_group_change_internal(&self, event: Arc<GroupChangeEvent>) -> Result<()>{
        //找到对应的group
        let messenger_context = self.messenger_map.get(event.notification.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.notification.messenger_id.to_string()))?;

        let bound_info = messenger_context.bound_map.get(event.notification.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(event.notification.user_id.to_string()))?;

        let connect_context = self.connect_map.get(&bound_info.connect_id)
        .ok_or_else(|| Error::ConnectNotFound(bound_info.connect_id))?;

        //处理group变更事件
        match event.change_type {
            GroupChangeType::Joined => {
                let span = span!(Level::INFO, "handle join group");
                let _enter = span.enter();
                //通知agent新建channel
                let payload = serde_json::to_value(event.notification.as_ref())?;
                connect_context.ws_context.send_json(WsMessage {
                    sn: connect_context.ws_context.next_request_sn(),
                    status_code: CODE_SUCCESS,
                    payload_type: TYPE_JOIN_GROUP,
                    payload: Some(payload),
                }).await?;
                //发channel变更消息
                let msg_event = group_change_to_incoming_message(event.clone());
                self.handle_incoming_message(msg_event).await;
            }
            GroupChangeType::Left => {
                let span = span!(Level::INFO, "handle leave group");
                let _enter = span.enter();
                //发channel变更消息
                let msg_event = group_change_to_incoming_message(event.clone());
                self.handle_incoming_message(msg_event).await;
                //通知agent退出channel
                let payload = serde_json::to_value(event.notification.as_ref())?;
                connect_context.ws_context.send_json(WsMessage {
                    sn: connect_context.ws_context.next_request_sn(),
                    status_code: CODE_SUCCESS,
                    payload_type: TYPE_LEAVE_GROUP,
                    payload: Some(payload),
                }).await?;
            }
        }
        Ok(())
    }
        
    async fn send_to_agent(&self, event: Arc<IncomingMessage>) -> Result<()>{
        //找到对应的connect
        let messenger_context = self.messenger_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;

        let bound_info = messenger_context.bound_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(event.user_id.to_string()))?;

        let connect_context = self.connect_map.get(&bound_info.connect_id)
        .ok_or_else(|| Error::ConnectNotFound(bound_info.connect_id))?;

        let payload = serde_json::to_value(event)?;
        let sn = connect_context.ws_context.next_request_sn();
        connect_context.ws_context.send_json(WsMessage {
            sn,
            status_code: CODE_SUCCESS,
            payload_type: TYPE_INCOMING_MESSAGE,
            payload: Some(payload),
        }).await?;
        Ok(())
    }

    async fn send_to_memory_store(&self, event: Arc<IncomingMessage>) -> Result<()>{
        //找到对应的agent和role
        let messenger_context = self.messenger_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;
        
        let bound_info = messenger_context.bound_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(format!("User not bound: user_id {}", event.user_id)))?;

        self.memory_store_client.push_messages(bound_info.agent_id.clone(), bound_info.role_name.clone(), event).await?;
        Ok(())
    }

    async fn process_user_remove(&self, event: Arc<UserRemoveEvent>) -> Result<()> {
        let messenger_context = self.messenger_map.get(event.notification.messenger_id.as_str())
            .ok_or_else(|| Error::MessengerNotFound(event.notification.messenger_id.to_string()))?;

        let (_,bound_info) = messenger_context.bound_map.remove(event.notification.user_id.as_str())
            .ok_or_else(|| Error::UserNotFound(event.notification.user_id.to_string()))?;

        let connect_context = self.connect_map.get(&bound_info.connect_id)
            .ok_or_else(|| Error::ConnectNotFound(bound_info.connect_id))?;

        //通知 agent 用户已删除
        let payload = serde_json::to_value(event.notification.as_ref())?;
        let msg = WsMessage {
            sn: connect_context.ws_context.next_request_sn(),
            status_code: CODE_SUCCESS,
            payload_type: TYPE_USER_REMOVED,
            payload: Some(payload),
        };
        connect_context.ws_context.send_json(msg).await?;

        Ok(())
    }

    async fn send_download_attachment_payload(&self, sn: u32, buf: BytesMut, connect_context: Arc<ConnectContext>) -> Result<AttachmentPayloadResponse> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handler = Box::new(DownloadResponseHandler {
            response_tx: tx,
        });

        connect_context.ws_context.send_bin_with_json_response(sn, buf.freeze(), handler).await?;

        let response =  rx.await??;
        Ok(response)
    }
}

#[async_trait]
impl GroupChangeHandler for ChannelManager {
    async fn handle_group_change(&self, event: Arc<GroupChangeEvent>){
        let span = span!(Level::INFO, "channel_manager handle group change");
        let _enter = span.enter();
        if let Err(e) = self.handle_group_change_internal(event).await {
            error!("Failed to handle group change: {:?}", e);
        }
    }
}

#[async_trait]
impl IncomingMessageHandler for ChannelManager {
    async fn handle_incoming_message(&self, event: Arc<IncomingMessage>) {
        let span = span!(Level::INFO, "channel_manager handle incoming message");
        let _enter = span.enter();
        let results = tokio::join!(
            self.send_to_agent(event.clone()),
            self.send_to_memory_store(event.clone()),
        );
        for result in vec![results.0, results.1] {
            if let Err(e) = result {
                error!("Error processing incoming message: {:?}", e);
            }
        }
    }
}

#[async_trait]
impl UserRemoveHandler for ChannelManager {
    async fn handle_user_remove(&self, event: Arc<UserRemoveEvent>) {
        let span = span!(Level::INFO, "channel_manager handle user remove");
        let _enter = span.enter();
        if let Err(e) = self.process_user_remove(event).await {
            error!("handle_user_remove error: {:?}", e);
        }
    }
}

#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    fn prepare_send(&self, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)> {
        let sender_info = self.attachment_sender_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?;
        let connect_context = sender_info.connect_context.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;
        drop(sender_info);

        let sn = connect_context.ws_context.next_request_sn();
        let capacity = OFFSET_ATT_DATA + size as usize;
        let mut buf = BytesMut::with_capacity(capacity);
        buf.put_u32(sn);
        buf.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        buf.put_u32(CODE_SUCCESS);
        buf.put_u32(transfer_id);
        buf.put_u32(size);
        buf.put_u64(pos);
        Ok((sn, buf))
    }

    async fn send(&self, sn: u32, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse> {
        let sender_info = self.attachment_sender_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?;
        let connect_context = sender_info.connect_context.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;
        let file_size = sender_info.info.size_bytes;
        drop(sender_info);

        // 判断是否为最后一块
        let is_last = pos + size as u64 >= file_size;

        let result = self.send_download_attachment_payload(sn, buf, connect_context).await;

        let is_error = match result.as_ref() { Ok(res) => res.error_code != 0, Err(_) => true };

        // 错误时清理，最后一块时清理过
        if is_error || is_last {
            self.attachment_sender_map.remove(&transfer_id);
        }

        result
    }
}
