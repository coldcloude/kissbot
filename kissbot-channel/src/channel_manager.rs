use crate::{Error, error::Result};
use crate::messenger::Messenger;
use crate::channel::Channel;
use crate::data::*;
use crate::memory_store_client::MemoryStoreClient;
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::{DashMap, Entry};
use kai_ws::{CODE_ERROR, CODE_SUCCESS, TYPE_HEARTBEAT, TYPE_RESPONSE, WsBinaryProcessor, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsJsonProcessor, WsMessage, WsProcessorInitializer, parse_bin_sn, ws_handle_connection_with_filter};
use kissbot_api::{AttachmentDownloadRequestDTO, AttachmentProcessor, TYPE_ATTACHMENT_DOWNLOAD_REQUEST, TYPE_ATTACHMENT_PAYLOAD, process_attachment};
use kissbot_api::channel::{BindRequestDTO, MessengerInfoRequestDTO, OutgoingMessageDTO, TYPE_BIND_AGENT_USER, TYPE_INCOMING_MESSAGE, TYPE_JOIN_GROUP, TYPE_LEAVE_GROUP, TYPE_MESSENGER_INFO_REQUEST, TYPE_OUTGOING_MESSAGE, TYPE_UNBIND_AGENT_USER};
use tracing::{Level, error, info, span};
use std::sync::{Arc, Weak, atomic::{AtomicU32, Ordering}};
use tokio::{net::TcpListener, time::{Duration}};

const MSG_QUEUE_SIZE: usize = 100;

static INTERVAL: Duration = Duration::from_secs(10);

struct BoundInfo {
    pub connect_id: u32,
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
}

struct MessengerContext {
    pub messenger: Arc<dyn Messenger>,
    pub bound_map: DashMap<String, BoundInfo>,
}

struct ChannelContext {
    channel: Arc<dyn Channel>,
    messenger_context: Weak<MessengerContext>,
    ws_context: Weak<WsContext>,
    memory_store_client: Weak<MemoryStoreClient>,
}

impl ChannelContext {
    async fn send_agent(&self, event: Arc<IncomingMessageEvent>) -> Result<()>{
        let ws_context = self.ws_context.upgrade()
        .ok_or_else(|| Error::InternalError("ws_context is None".to_string()))?;
        let payload = serde_json::to_value(event.messages.clone())?;
        ws_context.send_json(WsMessage {
            sn: ws_context.next_request_sn(),
            status_code: CODE_SUCCESS,
            payload_type: TYPE_INCOMING_MESSAGE,
            payload: Some(payload),
        }).await?;
        Ok(())
    }

    async fn send_memory_store(&self, event: Arc<IncomingMessageEvent>) -> Result<()>{
        let memory_store_client = self.memory_store_client.upgrade()
        .ok_or_else(|| Error::InternalError("memory_store_client is None".to_string()))?;
        let messenger_context = self.messenger_context.upgrade()
        .ok_or_else(|| Error::InternalError("messenger_context is None".to_string()))?;
        
        let channel_info = self.channel.get_info();
        let bound_info = messenger_context.bound_map.get(channel_info.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(format!("User not bound: user_id {}", channel_info.user_id)))?;
        memory_store_client.push_messages(bound_info.agent_id.clone(), bound_info.role_name.clone(), event.messages.clone()).await?;
        Ok(())
    }
}

#[async_trait]
impl IncomingMessageHandler for ChannelContext {
    async fn handle_incoming_message(&self, event: Arc<IncomingMessageEvent>) {
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

struct ConnectContext {
    connect_id: u32,
    ws_context: Arc<WsContext>,
    messenger_user_group_channel_map: DashMap<String, DashMap<String, DashMap<String, Arc<ChannelContext>>>>,
    global_attachment_sn: Arc<AtomicU32>,
    attachment_receiver_map: DashMap<u32, Arc<ChannelContext>>,
}

#[async_trait]
impl AttachmentDownloadPayloadSender for ConnectContext {
    async fn send_attachment_payload(&self, data: Bytes) -> Result<()> {
        self.ws_context.send_bin(data).await?;
        Ok(())
    }
}

pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
    memory_store_client: Arc<MemoryStoreClient>,
    api_key: Arc<String>,
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
        for messenger_map in connect_context.messenger_user_group_channel_map.iter() {
            //messenger存在时，移除user绑定记录
            if let Some(messenger_context) = manager.messenger_map.get(messenger_map.key()) {
                for user_map in messenger_map.iter() {
                    messenger_context.bound_map.remove(user_map.key());
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
        
        let messenger_info_request = serde_json::from_value::<MessengerInfoRequestDTO>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        
        let messenger_context = manager.messenger_map.get(&messenger_info_request.messenger_id)
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
        
        let bind_request = serde_json::from_value::<BindRequestDTO>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        let connect_context = self.connect_context.upgrade()
        .ok_or_else(|| Error::InternalError("connect_context is None".to_string()))?;

        let agent_id = Arc::new(bind_request.agent_id);
        let role_name = Arc::new(bind_request.role_name);

        let messenger_context = manager.messenger_map.get(bind_request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(bind_request.messenger_id.to_string()))?;
        
        let messenger_info = messenger_context.messenger.get_info().await?;

        let user_info = messenger_info.user_map.get(bind_request.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(bind_request.user_id.to_string()))?;

        let mut channel_infos = Vec::new();
        
        //绑定用户
        let bound_info = messenger_context.bound_map.entry(bind_request.user_id).or_insert_with(|| BoundInfo {
            connect_id: connect_context.connect_id,
            agent_id: agent_id.clone(),
            role_name: role_name.clone(),
        });
        
        if bound_info.connect_id != connect_context.connect_id {
            return Err(Error::UserAlreadyBound(bound_info.connect_id.to_string()));
        }

        //成功绑定到自身
        let mut inserted = false;
        let user_group_channel_map = connect_context.messenger_user_group_channel_map.entry(messenger_info.messenger_id.as_str().to_string()).or_insert_with(|| DashMap::new());
        let group_channel_map = user_group_channel_map.entry(user_info.user_id.as_str().to_string()).or_insert_with(|| {
            inserted = true;
            DashMap::new()
        });
        if inserted {
            //是新绑定的user，为每个group创建channel
            for group_info in user_info.group_map.iter() {
                let channel_context = manager.create_channel(messenger_context.clone(), user_info.user_id.clone(), group_info.group_id.clone(), connect_context.clone()).await?;
                let channel_info = channel_context.channel.get_info();
                group_channel_map.insert(group_info.group_id.as_str().to_string(), channel_context);
                channel_infos.push(channel_info);
            }
        }

        let channel_infos = serde_json::to_value(&channel_infos)?;
        Ok(Some(channel_infos))
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
        
        let bind_request = serde_json::from_value::<BindRequestDTO>(payload)?;
        
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
        
        //移除channel
        if let Some(user_group_channel_map) = connect_context.messenger_user_group_channel_map.get(bind_request.messenger_id.as_str()) {
            user_group_channel_map.remove(bind_request.user_id.as_str());
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
    manager: Weak<ChannelManager>,
}

#[async_trait]
impl JsonProcessorWrapper for OutgoingMessageProcessor {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
        let payload = data.payload
        .ok_or_else(|| Error::InvalidMessage("payload is None".to_string()))?;

        let outgoing_message = serde_json::from_value::<OutgoingMessageDTO>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;

        let messenger_context = manager.messenger_map.get(outgoing_message.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(outgoing_message.messenger_id.clone()))?;

        let bound_info = messenger_context.bound_map.get(outgoing_message.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(outgoing_message.user_id.clone()))?;

        let connect_context = manager.connect_map.get(&bound_info.connect_id)
        .ok_or_else(|| Error::ConnectNotFound(bound_info.connect_id.to_string()))?;

        let user_group_channel_map = connect_context.messenger_user_group_channel_map.get(outgoing_message.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(outgoing_message.messenger_id.clone()))?;

        let group_channel_map = user_group_channel_map.get(outgoing_message.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(outgoing_message.user_id.clone()))?;

        let channel_context = group_channel_map.get(outgoing_message.group_id.as_str())
        .ok_or_else(|| Error::GroupNotFound(outgoing_message.group_id.clone()))?;
        
        let response = channel_context.channel.send_message(outgoing_message, connect_context.global_attachment_sn.clone()).await?;
        for id in response.attachment_upload_id_map.iter() {
            connect_context.attachment_receiver_map.insert(*id, channel_context.clone());
        }

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
    connect_context: Weak<ConnectContext>,
}

#[async_trait]
impl AttachmentProcessor<Error> for AttachmentPayloadProcessor {
    async fn process_attachment(&self, id: u32, size: u32, pos: u64, data: &[u8]) -> Result<()> {
        let connect_context = self.connect_context.upgrade()
        .ok_or_else(|| Error::InternalError("connect_context is None".to_string()))?;

        let channel_context = connect_context.attachment_receiver_map.get(&id)
        .ok_or_else(|| Error::AttachmentNotFound(id.to_string()))?;

        if size == 0 {
            //最后传个size=0的，表示结尾
            connect_context.attachment_receiver_map.remove(&id);
        }
        else {
            channel_context.channel.send_attachment_payload(id, size, pos, data).await?;
        }

        Ok(())
    }
}

#[async_trait]
impl BinaryProcessorWrapper for AttachmentPayloadProcessor {
    async fn raw_process_bin(&self, data: &[u8]) -> Result<Option<serde_json::Value>> {
        process_attachment(data, self).await?;
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
    manager: Weak<ChannelManager>,
}

#[async_trait]
impl JsonProcessorWrapper for AttachmentDownloadRequestProcessor {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
        let payload = data.payload
        .ok_or_else(|| Error::InvalidMessage("payload is None".to_string()))?;

        let request = serde_json::from_value::<AttachmentDownloadRequestDTO>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;

        let messenger_context = manager.messenger_map.get(request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(request.messenger_id.clone()))?;

        let bound_info = messenger_context.bound_map.get(request.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(request.user_id.clone()))?;

        let connect_context = manager.connect_map.get(&bound_info.connect_id)
        .ok_or_else(|| Error::ConnectNotFound(bound_info.connect_id.to_string()))?;

        let user_group_channel_map = connect_context.messenger_user_group_channel_map.get(request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(request.messenger_id.clone()))?;

        let group_channel_map = user_group_channel_map.get(request.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(request.user_id.clone()))?;

        let channel_context = group_channel_map.get(request.group_id.as_str())
        .ok_or_else(|| Error::GroupNotFound(request.group_id.clone()))?;
        
        let response = channel_context.channel.download_attachment_header(request, connect_context.global_attachment_sn.clone()).await?;

        let responce = serde_json::to_value(response)?;
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
            messenger_user_group_channel_map: DashMap::new(),
            global_attachment_sn: Arc::new(AtomicU32::new(0)),
            attachment_receiver_map: DashMap::new(),
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
            manager: Arc::downgrade(&manager),
        });
        ws_context.set_json_processor(TYPE_OUTGOING_MESSAGE, outgoing_message_handler);
        //attachment payload
        let attachment_payload_handler = Arc::new(AttachmentPayloadProcessor {
            connect_context: Arc::downgrade(&connect_context),
        });
        ws_context.set_bin_processor(TYPE_ATTACHMENT_PAYLOAD, attachment_payload_handler);
        //attachment download request
        let attachment_download_request_handler = Arc::new(AttachmentDownloadRequestProcessor {
            manager: Arc::downgrade(&manager),
        });
        ws_context.set_json_processor(TYPE_ATTACHMENT_DOWNLOAD_REQUEST, attachment_download_request_handler);
        Ok(())

    }
}

impl ChannelManager {
    pub fn new(memory_store_base_url: &str, api_key: Arc<String>) -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
            messenger_map: DashMap::new(),
            memory_store_client: Arc::new(MemoryStoreClient::new(memory_store_base_url, api_key.clone())),
            api_key,
        }
    }

    pub async fn start(manager: Arc<Self>, addr: &str) -> Result<()> {
        //start memory store client
        let manager_for_memory_store = manager.clone();
        tokio::spawn(async move {
            manager_for_memory_store.memory_store_client.start_send_messages().await
        });
        //start wss server
        let span = span!(Level::INFO, "wss serverstart");
        let _enter = span.enter();
        let listener = TcpListener::bind(addr).await?;
        info!("WSS Server listening on: {}", addr);
        let initializer = ChannelManagerInitializer {};
        let filter = kissbot_security::ApiKeyWsFilter::new(std::sync::Arc::new(kissbot_security::SimpleApiKeyValidator::new(manager.api_key.clone())));
        while let Ok((stream, _)) = listener.accept().await {
            ws_handle_connection_with_filter(stream, MSG_QUEUE_SIZE, manager.clone(), &initializer, &[&filter]).await?;
        }
        Ok(())
    }
    
    pub fn register_messenger(manager: Arc<Self>, messenger_id: &str, messenger: Arc<dyn Messenger>) -> Result<()> {
        match manager.messenger_map.entry(messenger_id.to_string()) {
            Entry::Vacant(entry) => {
                let messenger_context = Arc::new(MessengerContext {
                    messenger: messenger.clone(),
                    bound_map: DashMap::new(),
                });
                entry.insert(messenger_context);
                let manager_weak = Arc::downgrade(&manager);
                messenger.register_on_group_change(manager_weak);
                Ok(())
            }
            Entry::Occupied(entry) => {
                Err(Error::MessengerAlreadyRegistered(entry.key().to_string()))
            }
        }
    }

    async fn create_channel(&self, messenger_context: Arc<MessengerContext>, user_id: Arc<String>, group_id: Arc<String>, connect_context: Arc<ConnectContext>) -> Result<Arc<ChannelContext>> {
        //新建channel
        let channel = messenger_context.messenger.create_channel(user_id.as_str(), group_id.as_str()).await?;
        //记录context
        let channel_context = Arc::new(ChannelContext {
            channel: channel.clone(),
            messenger_context: Arc::downgrade(&messenger_context),
            ws_context: Arc::downgrade(&connect_context.ws_context),
            memory_store_client: Arc::downgrade(&self.memory_store_client),
        });
        //绑定消息事件
        let channel_context_weak = Arc::downgrade(&channel_context);
        channel.register_on_incoming_messages(channel_context_weak);
        channel.register_on_download_attachment_payload(connect_context.clone());
        //返回
        Ok(channel_context)
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

        let user_group_channel_map = connect_context.messenger_user_group_channel_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;

        let group_channel_map = user_group_channel_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(event.user_id.to_string()))?;

        //处理group变更事件
        match event.change_type {
            GroupChangeType::Joined => {
                //新建channel
                let channel_context = self.create_channel(messenger_context.clone(), event.user_id.clone(), event.group_id.clone(), connect_context.clone()).await?;
                group_channel_map.insert(event.group_id.as_str().to_string(), channel_context.clone());
                let channel_info = channel_context.channel.get_info();
                //通知agent新建channel
                let channel_info = serde_json::to_value(channel_info)?;
                connect_context.ws_context.send_json(WsMessage {
                    sn: connect_context.ws_context.next_request_sn(),
                    status_code: CODE_SUCCESS,
                    payload_type: TYPE_JOIN_GROUP,
                    payload: Some(channel_info),
                }).await?;
                //发channel变更消息
                let msg_event = channel_context.channel.group_change_to_incoming_message(event.clone());
                channel_context.handle_incoming_message(msg_event).await;
            }
            GroupChangeType::Left => {
                let span = span!(Level::INFO, "channel_manager handle leave group");
                let _enter = span.enter();
                //退出group
                let (_,channel_context) = group_channel_map.remove(event.group_id.as_str())
                .ok_or_else(|| Error::GroupNotFound(event.group_id.to_string()))?;
                //发channel变更消息
                let msg_event = channel_context.channel.group_change_to_incoming_message(event.clone());
                channel_context.handle_incoming_message(msg_event).await;
                //通知agent退出channel
                let channel_info = channel_context.channel.get_info();
                let channel_info = serde_json::to_value(channel_info)?;
                connect_context.ws_context.send_json(WsMessage {
                    sn: connect_context.ws_context.next_request_sn(),
                    status_code: CODE_SUCCESS,
                    payload_type: TYPE_LEAVE_GROUP,
                    payload: Some(channel_info),
                }).await?;
            }
        }
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
