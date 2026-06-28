use crate::{Error, error::Result};
use crate::messenger::{Messenger, MessengerCreator};
use crate::data::*;
use crate::memory_store_client::MemoryStoreClient;
use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use dashmap::{DashMap, DashSet, Entry};
use kai_ws::{CODE_ERROR, CODE_SUCCESS, TYPE_HEARTBEAT, TYPE_RESPONSE, WsBinaryProcessor, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsJsonProcessor, WsMessage, WsProcessorInitializer, parse_bin_sn, ws_handle_connection_with_filter};
use kissbot_api::{DataWriter, GroupChangeNotification, UserRemoveNotification, TYPE_ATTACHMENT_DOWNLOAD_REQUEST, TYPE_ATTACHMENT_PAYLOAD, parse_attachment_payload_header};
use kissbot_api::channel::{AttachmentDownloadRequest, BindRequest, MessengerInfoRequest, OutgoingMessage, TYPE_BIND_AGENT_USER, TYPE_INCOMING_MESSAGE, TYPE_JOIN_GROUP, TYPE_LEAVE_GROUP, TYPE_MESSENGER_INFO_REQUEST, TYPE_OUTGOING_MESSAGE, TYPE_UNBIND_AGENT_USER, TYPE_USER_REMOVED, WsOutgoingMessageResponse, WsAttachmentDownloadResponseHeader};
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
    messenger_users_map: DashMap<String, DashSet<String>>,
}

pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
    memory_store_client: Arc<MemoryStoreClient>,
    global_attachment_sn: Arc<AtomicU32>,
    // 上传方向：key → (internal_upload_id, Weak<Messenger>)
    attachment_receiver_map: DashMap<String, (u32, Weak<dyn Messenger>)>,
    // upload_id → key（WS 二进制帧按 id 查找后转 key）
    receiver_id_to_key: DashMap<u32, String>,
    // 下载方向：key → (internal_download_id, Weak<ConnectContext>)
    attachment_sender_map: DashMap<String, (u32, Weak<ConnectContext>)>,
    // download_id → key
    sender_id_to_key: DashMap<u32, String>,
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
        .ok_or_else(|| Error::ConnectNotFound(self.connect_id.to_string()))?;
        //把绑定记录从messenger中移除
        for messenger_users in connect_context.messenger_users_map.iter() {
            //messenger存在时，移除user绑定记录
            if let Some(messenger_context) = manager.messenger_map.get(messenger_users.key()) {
                for user in messenger_users.iter() {
                    messenger_context.bound_map.remove(user.key());
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
    async fn raw_process_bin(&self, data: &[u8]) -> Result<Option<serde_json::Value>>;
    
    async fn wrap_process_bin(&self, data: &[u8], context: Arc<WsContext>) -> Result<()> {
        let sn = parse_bin_sn(data)?;
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

        let user_info = messenger_info.user_map.get(bind_request.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(bind_request.user_id.to_string()))?;
        
        //绑定用户
        let bound_info = messenger_context.bound_map.entry(bind_request.user_id.to_string()).or_insert_with(|| BoundInfo {
            connect_id: connect_context.connect_id,
            agent_id: agent_id.clone(),
            role_name: role_name.clone(),
        });
        
        if bound_info.connect_id != connect_context.connect_id {
            return Err(Error::UserAlreadyBound(bound_info.connect_id.to_string()));
        }

        //connect绑定
        let messenger_users = connect_context.messenger_users_map.entry(messenger_info.messenger_id.as_str().to_string()).or_insert_with(|| DashSet::new());
        messenger_users.insert(user_info.user_id.as_str().to_string());

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

        //检查是否为本连接绑定
        let bound_info = messenger_context.bound_map.get(bind_request.user_id.as_str())
        .ok_or_else(|| Error::UserNotBound(bind_request.user_id.to_string()))?;        
        if bound_info.connect_id != connect_context.connect_id {
            return Err(Error::UserAlreadyBound(bound_info.connect_id.to_string()));
        }
        
        //移除connect绑定
        if let Some(messenger_users) = connect_context.messenger_users_map.get(bind_request.messenger_id.as_str()) {
            messenger_users.remove(bind_request.user_id.as_str());
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

        let response = messenger_context.messenger.send_message(outgoing_message, manager.global_attachment_sn.clone()).await?;

        // 从 response 中获取 key_map，分配内部 upload_id，构造 WsOutgoingMessageResponse
        let ws_response = WsOutgoingMessageResponse {
            msg_id: response.msg_id.clone(),
            time: response.time.clone(),
            attachment_upload_id_map: Arc::new(DashMap::new()),
            attachment_key_map: response.attachment_key_map.clone(),
        };

        for entry in response.attachment_key_map.iter() {
            let att_id = entry.key().clone();
            let key = entry.value().clone();
            let internal_id = manager.global_attachment_sn.fetch_add(1, Ordering::SeqCst);
            ws_response.attachment_upload_id_map.insert(att_id, internal_id);
            manager.attachment_receiver_map.insert(key.to_string(), (internal_id, Arc::downgrade(&messenger_context.messenger)));
            manager.receiver_id_to_key.insert(internal_id, key.to_string());
        }

        let response = serde_json::to_value(ws_response)?;
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
    async fn raw_process_bin(&self, data: &[u8]) -> Result<Option<serde_json::Value>> {
        let header = parse_attachment_payload_header(data)?;

        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;

        // 通过 id 找到 key
        let key = manager.receiver_id_to_key.get(&header.id)
            .ok_or_else(|| Error::AttachmentNotFound(header.id.to_string()))?
            .clone();

        // 通过 key 找到 messenger
        let messenger_entry = manager.attachment_receiver_map.get(&key)
            .ok_or_else(|| Error::AttachmentNotFound(key.clone()))?;
        let (_, ref messenger) = *messenger_entry;
        let messenger = messenger.upgrade()
        .ok_or_else(|| Error::InternalError("messenger is None".to_string()))?;

        if header.size == 0 {
            //最后传个size=0的，表示结尾
            manager.attachment_receiver_map.remove(&key);
            manager.receiver_id_to_key.remove(&header.id);
        }

        // 拷贝数据以便构造 owned SliceDataWriter（摆脱生命周期约束）
        let owned_data = data.to_vec();
        struct SliceDataWriter(Vec<u8>);

        impl DataWriter<crate::Error> for SliceDataWriter {
            fn write_to(&self, buf: &mut BytesMut) -> std::result::Result<(), crate::Error> {
                buf.extend_from_slice(&self.0);
                Ok(())
            }
        }

        let writer: Arc<dyn DataWriter<crate::Error>> = Arc::new(SliceDataWriter(owned_data));
        messenger.send_attachment_payload(&key, header.size, header.pos, writer).await?;

        Ok(None)
    }
}

#[async_trait]
impl WsBinaryProcessor for AttachmentPayloadProcessor {
    async fn process_bin(&self, data: &[u8], context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_bin(data, context).await {
            error!("attachment_payload error: {:?}", e);
        }
    }
}

struct AttachmentDownloadRequestProcessor {
    connect_context: Weak<ConnectContext>,
    manager: Weak<ChannelManager>,
}

#[async_trait]
impl JsonProcessorWrapper for AttachmentDownloadRequestProcessor {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
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
        
        let att_info_response = messenger_context.messenger.download_attachment_header(request, manager.global_attachment_sn.clone()).await?;
        let key = att_info_response.key.clone();
        let internal_id = manager.global_attachment_sn.fetch_add(1, Ordering::SeqCst);
        manager.attachment_sender_map.insert(key.to_string(), (internal_id, Arc::downgrade(&connect_context)));
        manager.sender_id_to_key.insert(internal_id, key.to_string());

        // 构造 WsAttachmentDownloadResponseHeader
        let ws_response = WsAttachmentDownloadResponseHeader {
            download_id: internal_id,
            response: att_info_response,
        };
        let responce = serde_json::to_value(ws_response)?;
        Ok(Some(responce))
    }
}

#[async_trait]
impl WsJsonProcessor for AttachmentDownloadRequestProcessor {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_json(data, context).await {
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
            messenger_users_map: DashMap::new(),
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
            global_attachment_sn: Arc::new(AtomicU32::new(0)),
            attachment_receiver_map: DashMap::new(),
            receiver_id_to_key: DashMap::new(),
            attachment_sender_map: DashMap::new(),
            sender_id_to_key: DashMap::new(),
        }
    }

    pub async fn start(manager: Arc<Self>, addr: &str) -> Result<()> {
        //start memory store client
        let manager_for_memory_store = manager.clone();
        tokio::spawn(async move {
            manager_for_memory_store.memory_store_client.start_send_messages().await
        });
        //start ws server
        let span = span!(Level::INFO, "ws server start");
        let _enter = span.enter();
        let listener = TcpListener::bind(addr).await?;
        info!("WS Server listening on: {}", addr);
        let initializer = ChannelManagerInitializer {};
        let filter = kissbot_security::ApiKeyWsFilter::new(std::sync::Arc::new(kissbot_security::SimpleApiKeyValidator::new(kissbot_security::SecurityConfig::get().api_key.clone())));
        while let Ok((stream, _)) = listener.accept().await {
            ws_handle_connection_with_filter(stream, MSG_QUEUE_SIZE, manager.clone(), &initializer, &[&filter]).await?;
        }
        Ok(())
    }
    
    pub async fn register_messenger<M,MC>(manager: Arc<Self>, messenger_id: &str, messenger_creator: MC) -> Result<Arc<M>>
    where
        M: Messenger,
        MC: MessengerCreator<M>
    {
        match manager.messenger_map.entry(messenger_id.to_string()) {
            Entry::Vacant(entry) => {
                let group_change_handler = Arc::downgrade(&manager);
                let incoming_messages_handler = Arc::downgrade(&manager);
                let download_attachment_payload_handler = Arc::downgrade(&manager);
                let user_remove_handler = Arc::downgrade(&manager);
                let messenger = messenger_creator.create(
                    group_change_handler,
                    incoming_messages_handler,
                    download_attachment_payload_handler,
                    user_remove_handler,
                    manager.global_attachment_sn.clone(),  // 新增
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
}

impl ChannelManager {
    async fn handle_group_change_internal(&self, event: Arc<GroupChangeEvent>) -> Result<()>{
        //找到对应的group
        let messenger_context = self.messenger_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;

        let bound_info = messenger_context.bound_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(event.user_id.to_string()))?;

        let connect_context = self.connect_map.get(&bound_info.connect_id)
        .ok_or_else(|| Error::ConnectNotFound(bound_info.connect_id.to_string()))?;

        //处理group变更事件
        match event.change_type {
            GroupChangeType::Joined => {
                let span = span!(Level::INFO, "handle join group");
                let _enter = span.enter();
                //通知agent新建channel
                let notif = GroupChangeNotification {
                    messenger_id: event.messenger_id.clone(),
                    user_id: event.user_id.clone(),
                    group_id: event.group_id.clone()
                };
                let payload = serde_json::to_value(notif)?;
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
                let notif = GroupChangeNotification {
                    messenger_id: event.messenger_id.clone(),
                    user_id: event.user_id.clone(),
                    group_id: event.group_id.clone()
                };
                let payload = serde_json::to_value(notif)?;
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
        
    async fn send_agent(&self, event: Arc<IncomingMessageEvent>) -> Result<()>{
        //找到对应的connect
        let messenger_context = self.messenger_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;

        let bound_info = messenger_context.bound_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(event.user_id.to_string()))?;

        let connect_context = self.connect_map.get(&bound_info.connect_id)
        .ok_or_else(|| Error::ConnectNotFound(bound_info.connect_id.to_string()))?;

        let payload = serde_json::to_value(event.messages.clone())?;
        let sn = connect_context.ws_context.next_request_sn();
        connect_context.ws_context.send_json(WsMessage {
            sn,
            status_code: CODE_SUCCESS,
            payload_type: TYPE_INCOMING_MESSAGE,
            payload: Some(payload),
        }).await?;
        Ok(())
    }

    async fn send_memory_store(&self, event: Arc<IncomingMessageEvent>) -> Result<()>{
        //找到对应的agent和role
        let messenger_context = self.messenger_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;
        
        let bound_info = messenger_context.bound_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(format!("User not bound: user_id {}", event.user_id)))?;

        self.memory_store_client.push_messages(bound_info.agent_id.clone(), bound_info.role_name.clone(), event.messages.clone()).await?;
        Ok(())
    }

    async fn handle_user_remove_internal(&self, event: Arc<UserRemoveEvent>) -> Result<()> {
        let messenger_context = self.messenger_map.get(event.messenger_id.as_str())
            .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;

        let (_,bound_info) = messenger_context.bound_map.remove(event.user_id.as_str())
            .ok_or_else(|| Error::UserNotFound(event.user_id.to_string()))?;

        let connect_context = self.connect_map.get(&bound_info.connect_id)
            .ok_or_else(|| Error::ConnectNotFound(bound_info.connect_id.to_string()))?;

        //从 connect 中移除 user 记录
        if let Some(messenger_users) = connect_context.messenger_users_map.get(event.messenger_id.as_str()) {
            messenger_users.remove(event.user_id.as_str());
        }

        //通知 agent 用户已删除
        let notif = UserRemoveNotification {
            messenger_id: event.messenger_id.clone(),
            user_id: event.user_id.clone(),
        };
        let payload = serde_json::to_value(notif)?;
        let msg = WsMessage {
            sn: connect_context.ws_context.next_request_sn(),
            status_code: CODE_SUCCESS,
            payload_type: TYPE_USER_REMOVED,
            payload: Some(payload),
        };
        connect_context.ws_context.send_json(msg).await?;

        Ok(())
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
    async fn handle_incoming_message(&self, event: Arc<IncomingMessageEvent>) {
        let span = span!(Level::INFO, "channel_manager handle incoming message");
        let _enter = span.enter();
        let results = tokio::join!(
            self.send_agent(event.clone()),
            self.send_memory_store(event.clone()),
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
        if let Err(e) = self.handle_user_remove_internal(event).await {
            error!("handle_user_remove error: {:?}", e);
        }
    }
}

#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, writer: Arc<dyn DataWriter<Error>>) -> Result<()> {
        let sender_entry = self.attachment_sender_map.get(key)
            .ok_or_else(|| Error::AttachmentNotFound(key.to_string()))?;
        let (internal_id, ref connect_weak) = *sender_entry;
        let connect_context = connect_weak.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

        // 从 writer 读取数据后立即释放 writer（避免 writer 跨 await 的 Send 限制）
        let mut payload_buf = BytesMut::new();
        writer.write_to(&mut payload_buf)?;
        let payload_data = payload_buf.freeze();
        // writer 在这里 drop，不再跨 await
        drop(writer);

        // 构造 kai-ws 二进制帧: [sn(4) + payload_type(4) + status_code(4)] + [id(4) + size(4) + pos(8)] + [data]
        let mut frame = BytesMut::new();
        frame.put_u32(0);  // sn
        frame.put_u32(TYPE_ATTACHMENT_PAYLOAD);  // payload_type
        frame.put_u32(CODE_SUCCESS);  // status_code
        frame.put_u32(internal_id);
        frame.put_u32(size);
        frame.put_u64(pos);
        frame.extend_from_slice(&payload_data);

        if size == 0 {
            //最后传个size=0的，表示结尾
            self.attachment_sender_map.remove(key);
            self.sender_id_to_key.remove(&internal_id);
        }

        drop(sender_entry);

        connect_context.ws_context.send_bin(frame.freeze()).await?;

        Ok(())
    }
}
