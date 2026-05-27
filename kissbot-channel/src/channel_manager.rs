use crate::{Error, error::Result};
use crate::messenger::Messenger;
use crate::channel::Channel;
use crate::data::*;
use crate::memory_store_client::MemoryStoreClient;
use async_trait::async_trait;
use dashmap::{DashMap, Entry};
use kai_ws::{CODE_ERROR, CODE_SUCCESS, TYPE_HEARTBEAT, TYPE_RESPONSE, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsJsonProcessor, WsMessage, WsProcessorInitializer, ws_handle_connection};
use kissbot_api::channel::{BindRequestDTO, TYPE_BIND_AGENT_USER, TYPE_INCOMING_MESSAGE, TYPE_JOIN_GROUP, TYPE_LEAVE_GROUP, TYPE_UNBIND_AGENT_USER};
use tracing::{Level, error, info, span};
use std::sync::{Arc, Weak, atomic::{AtomicU32, Ordering}};
use tokio::{net::TcpListener, time::{Duration}};

const MSG_QUEUE_SIZE: usize = 100;

static INTERVAL: Duration = Duration::from_secs(10);

struct MessengerContext {
    pub messenger: Arc<dyn Messenger>,
    pub bound_map: DashMap<String, (Arc<String>, Arc<String>)>,
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
        let agent_role = messenger_context.bound_map.get(channel_info.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(format!("User not bound: user_id {}", channel_info.user_id)))?;
        memory_store_client.push_messages(agent_role.0.clone(), agent_role.1.clone(), event.messages.clone()).await?;
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

struct AgentContext {
    connect_id: u32,
    agent_id: Arc<String>,
    messenger_user_group_channel_map: DashMap<String, DashMap<String, DashMap<String, Arc<ChannelContext>>>>,
}

struct ConnectContext {
    connect_id: u32,
    ws_context: Arc<WsContext>,
    agent_map: DashMap<String, Arc<AgentContext>>,
}

pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    agent_map: DashMap<String, Arc<AgentContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
    memory_store_client: Arc<MemoryStoreClient>,
}

struct ConnectCloseHandler {
    manager: Weak<ChannelManager>,
    connect_id: u32,
}

impl ConnectCloseHandler {
    pub fn close_connect(&self) -> Result<()> {
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        //移除连接记录
        let (_,connect_context) = manager.connect_map.remove(&self.connect_id)
        .ok_or_else(|| Error::InternalError(format!("Connect not found: connect_id {}", self.connect_id)))?;
        for agent_context in connect_context.agent_map.iter() {
            //移除agent记录
            manager.agent_map.remove(agent_context.agent_id.as_str());
            //把agent绑定记录从messenger中移除
            for messenger_map in agent_context.messenger_user_group_channel_map.iter() {
                //要求messenger必须存在
                let messenger_context = manager.messenger_map.get(messenger_map.key())
                .ok_or_else(|| Error::InternalError(format!("Messenger not found: connect_id {}, messenger_id {}", self.connect_id, messenger_map.key())))?;
                //移除所有绑定的user
                for user_map in messenger_map.iter() {
                    messenger_context.bound_map.remove(user_map.key());
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl WsCloseProcessor for ConnectCloseHandler {
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

struct BindAgentHandler {
    manager: Weak<ChannelManager>,
    connect_context: Weak<ConnectContext>,
}

#[async_trait]
impl JsonProcessorWrapper for BindAgentHandler {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
        let payload = data.payload
        .ok_or_else(|| Error::InternalError("payload is None".to_string()))?;
        
        let bind_request = serde_json::from_value::<BindRequestDTO>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        let connect_context = self.connect_context.upgrade()
        .ok_or_else(|| Error::InternalError("connect_context is None".to_string()))?;

        let agent_id = Arc::new(bind_request.agent_id);
        let role_name = Arc::new(bind_request.role_name);

        //检查agent是否已绑定
        let agent_context = match manager.agent_map.entry(agent_id.as_str().to_string()) {
            Entry::Vacant(entry) => {
                //没绑定过，新建
                let agent_context = Arc::new(AgentContext {
                    connect_id: connect_context.connect_id,
                    agent_id: agent_id.clone(),
                    messenger_user_group_channel_map: DashMap::new(),
                });
                //保存connect
                connect_context.agent_map.insert(agent_id.as_str().to_string(), agent_context.clone());
                //保存agent
                entry.insert(agent_context.clone());
                Ok(agent_context)
            }
            Entry::Occupied(entry) => {
                //已绑定，检查是否是当前connect
                //如果不是，报错
                if entry.get().connect_id == connect_context.connect_id {
                    //沿用当前
                    Ok(entry.get().clone())
                }
                else {
                    //被其他connect绑定，报错
                    Err(Error::AgentAlreadyBound(entry.key().to_string()))
                }
            }
        }?;

        let messenger_context = manager.messenger_map.get(bind_request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(bind_request.messenger_id.to_string()))?;
        
        let messenger_info = messenger_context.messenger.get_info().await?;

        let user_info = messenger_info.user_map.get(bind_request.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(bind_request.user_id.to_string()))?;

        let mut channel_infos = Vec::new();
        
        //绑定用户
        let bound_info = messenger_context.bound_map.entry(bind_request.user_id).or_insert_with(|| (agent_context.agent_id.clone(), role_name.clone()));
        
        if bound_info.0.as_str() == agent_context.agent_id.as_str() {
            //成功绑定到自身
            let mut inserted = false;
            let user_map = agent_context.messenger_user_group_channel_map.entry(messenger_info.messenger_id.as_str().to_string()).or_insert_with(|| DashMap::new());
            let group_map = user_map.entry(user_info.user_id.as_str().to_string()).or_insert_with(|| {
                inserted = true;
                DashMap::new()
            });
            if inserted {
                //是新绑定的user，为每个group创建channel
                for group_info in user_info.group_map.iter() {
                    let channel_context = manager.create_channel(messenger_context.clone(), user_info.user_id.clone(), group_info.group_id.clone(), connect_context.ws_context.clone()).await?;
                    let channel_info = channel_context.channel.get_info();
                    group_map.insert(group_info.group_id.as_str().to_string(), channel_context);
                    channel_infos.push(channel_info);
                }
            }
        }

        let channel_infos = serde_json::to_value(&channel_infos)?;
        Ok(Some(channel_infos))
    }
}

#[async_trait]
impl WsJsonProcessor for BindAgentHandler {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_json(data, context).await {
            error!("bind_agent_user error: {:?}", e);
        }
    }
}

struct UnbindAgentUserHandler {
    manager: Weak<ChannelManager>,
    connect_context: Weak<ConnectContext>,
}

#[async_trait]
impl JsonProcessorWrapper for UnbindAgentUserHandler {
    async fn raw_process_json(&self, data: WsMessage) -> Result<Option<serde_json::Value>> {
        let payload = data.payload
        .ok_or_else(|| Error::RequestError("payload is None".to_string()))?;
        
        let bind_request = serde_json::from_value::<BindRequestDTO>(payload)?;
        
        let manager = self.manager.upgrade()
        .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        let connect_context = self.connect_context.upgrade()
        .ok_or_else(|| Error::InternalError("connect_context is None".to_string()))?;
        
        //移除channel
        if let Some(agent_context) = connect_context.agent_map.get(bind_request.agent_id.as_str()) {
            if let Some(user_map) = agent_context.messenger_user_group_channel_map.get(bind_request.messenger_id.as_str()) {
                user_map.remove(bind_request.user_id.as_str());
            }
        }
        //解除绑定
        if let Some(messenger_context) = manager.messenger_map.get(bind_request.messenger_id.as_str()) {
            messenger_context.bound_map.remove(bind_request.user_id.as_str());
        }

        Ok(None)
    }
}

#[async_trait]
impl WsJsonProcessor for UnbindAgentUserHandler {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>){
        if let Err(e) = self.wrap_process_json(data, context).await {
            error!("unbind_agent_user error: {:?}", e);
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
            agent_map: DashMap::new(),
        });
        manager.connect_map.insert(connect_id, connect_context.clone());
        //处理心跳
        let heartbeat_handler = Arc::new(WsHeartbeatHandler::new(INTERVAL, ws_context.clone()));
        ws_context.set_bin_processor(TYPE_HEARTBEAT, heartbeat_handler.clone());
        tokio::spawn(async move { heartbeat_handler.start().await });
        //处理关闭
        let close_handler = Arc::new(ConnectCloseHandler {
            manager: Arc::downgrade(&manager),
            connect_id,
        });
        ws_context.set_close_processor(close_handler);
        //agent绑定
        let bind_agent_handler = Arc::new(BindAgentHandler {
            manager: Arc::downgrade(&manager),
            connect_context: Arc::downgrade(&connect_context),
        });
        ws_context.set_json_processor(TYPE_BIND_AGENT_USER, bind_agent_handler);
        //agent解绑
        let unbind_agent_handler = Arc::new(UnbindAgentUserHandler {
            manager: Arc::downgrade(&manager),
            connect_context: Arc::downgrade(&connect_context),
        });
        ws_context.set_json_processor(TYPE_UNBIND_AGENT_USER, unbind_agent_handler);
        Ok(())
    }
}

impl ChannelManager {
    pub fn new(memory_store_base_url: &str) -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
            agent_map: DashMap::new(),
            messenger_map: DashMap::new(),
            memory_store_client: Arc::new(MemoryStoreClient::new(memory_store_base_url)),
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
        while let Ok((stream, _)) = listener.accept().await {
            ws_handle_connection(stream, MSG_QUEUE_SIZE, manager.clone(), &initializer).await?;
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

    async fn create_channel(&self, messenger_context: Arc<MessengerContext>, user_id: Arc<String>, group_id: Arc<String>, ws_context: Arc<WsContext>) -> Result<Arc<ChannelContext>> {
        //新建channel
        let channel = messenger_context.messenger.create_channel(user_id.as_str(), group_id.as_str()).await?;
        //记录context
        let channel_context = Arc::new(ChannelContext {
            channel: channel.clone(),
            messenger_context: Arc::downgrade(&messenger_context),
            ws_context: Arc::downgrade(&ws_context),
            memory_store_client: Arc::downgrade(&self.memory_store_client),
        });
        //绑定消息事件
        let channel_context_weak = Arc::downgrade(&channel_context);
        channel.register_on_incoming_messages(channel_context_weak);
        //返回
        Ok(channel_context)
    }
}

impl ChannelManager {
    async fn handle_group_change_internal(&self, event: Arc<GroupChangeEvent>) -> Result<()>{
        //找到对应的group
        let messenger_context = self.messenger_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;

        let agent_role = messenger_context.bound_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(event.user_id.to_string()))?;

        let agent_context = self.agent_map.get(agent_role.0.as_str())
        .ok_or_else(|| Error::AgentNotFound(agent_role.0.to_string()))?;

        let connect_context = self.connect_map.get(&agent_context.connect_id)
        .ok_or_else(|| Error::ConnectNotFound(agent_context.connect_id.to_string()))?;

        let user_map = agent_context.messenger_user_group_channel_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;

        let group_map = user_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(event.user_id.to_string()))?;

        //处理group变更事件
        match event.change_type {
            GroupChangeType::Joined => {
                //新建channel
                let channel_context = self.create_channel(messenger_context.clone(), event.user_id.clone(), event.group_id.clone(), connect_context.ws_context.clone()).await?;
                group_map.insert(event.group_id.as_str().to_string(), channel_context.clone());
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
                let (_,channel_context) = group_map.remove(event.group_id.as_str())
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
