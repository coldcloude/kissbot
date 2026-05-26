use crate::{Error, error::Result};
use crate::messenger::Messenger;
use crate::channel::Channel;
use crate::data::*;
use crate::memory_store_client::MemoryStoreClient;
use async_trait::async_trait;
use dashmap::{DashMap, Entry};
use flume::{Receiver, Sender, bounded};
use kai_ws::{TYPE_HEARTBEAT, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsMessage, WsProcessorInitializer, ws_handle_connection};
use kissbot_api::channel::{BindRequestDTO, TYPE_JOINT_GROUP};
use tracing::{Level, error, info, span};
use std::sync::{Arc, Weak, atomic::{AtomicU32, Ordering}};
use base64::Engine;
use tokio::{net::TcpListener, time::{Duration}};

const MSG_QUEUE_SIZE: usize = 100;

static INTERVAL: Duration = Duration::from_secs(10);

struct MessengerContext {
    pub messenger_id: Arc<String>,
    pub messenger: Arc<dyn Messenger>,
    pub bound_map: DashMap<String, Arc<String>>,
}

struct ChannelContext {
    channel: Arc<dyn Channel>,
    memory_store_queue: (Sender<Vec<IncomingMessage>>, Receiver<Vec<IncomingMessage>>),
    agent_queue: (Sender<Vec<IncomingMessage>>, Receiver<Vec<IncomingMessage>>),
}

struct AgentContext {
    connect_id: u32,
    agent_id: Arc<String>,
    messenger_user_group_channel_map: DashMap<String, DashMap<String, DashMap<String, ChannelContext>>>,
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
    messenger_map: DashMap<String, MessengerContext>,
    memory_store_client: MemoryStoreClient,
}

struct ConnectCloseHandler {
    manager: Weak<ChannelManager>,
    connect_id: u32,
}

#[async_trait]
impl WsCloseProcessor for ConnectCloseHandler {
    async fn process_close(&self, _: Arc<WsContext>) -> std::result::Result<(), kai_ws::Error> {
        if let Some(manager) = self.manager.upgrade() {
            manager.close_connect(self.connect_id);
        }
        Ok(())
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
        manager.connect_map.insert(connect_id, connect_context);
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
        Ok(())
    }
}

async fn create_channel(messenger: &dyn Messenger, user_id: &str, group_id: &str, group_map: &DashMap<String, ChannelContext>) -> Result<Arc<dyn Channel>> {
    //新建channel
    let channel = messenger.create_channel(user_id, group_id).await?;
    //记录context
    group_map.insert(group_id.to_string(), ChannelContext {
        channel: channel.clone(),
        agent_queue: bounded(MSG_QUEUE_SIZE),
        memory_store_queue: bounded(MSG_QUEUE_SIZE),
    });
    //返回
    Ok(channel)
}

impl ChannelManager {
    pub fn new(memory_store_base_url: &str) -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
            agent_map: DashMap::new(),
            messenger_map: DashMap::new(),
            memory_store_client: MemoryStoreClient::new(memory_store_base_url),
        }
    }

    pub async fn start(manager: Arc<Self>, addr: &str) -> Result<()> {
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

    pub fn close_connect(&self, connect_id: u32) {
        let span = span!(Level::INFO, "channel_manager close_connect");
        let _enter = span.enter();
        //移除连接记录
        if let Some((_,connect_context)) = self.connect_map.remove(&connect_id) {
            for agent_context in connect_context.agent_map.iter() {
                //移除agent记录
                self.agent_map.remove(agent_context.agent_id.as_str());
                //把agent绑定记录从messenger中移除
                for messenger_map in agent_context.messenger_user_group_channel_map.iter() {
                    //要求messenger必须存在
                    if let Some(messenger_context) = self.messenger_map.get(messenger_map.key()) {
                        for user_map in messenger_map.iter() {
                            messenger_context.bound_map.remove(user_map.key());
                        }
                    }
                    else {
                        error!("Messenger not found: connect_id {}, messenger_id {}", connect_id, messenger_map.key());
                    }
                }
            }
        }
    }
    
    pub fn register_messenger(&self, messenger_id: &str, messenger: Arc<dyn Messenger>) -> Result<()> {
        match self.messenger_map.entry(messenger_id.to_string()) {
            Entry::Vacant(entry) => {
                entry.insert(MessengerContext {
                    messenger_id: Arc::new(messenger_id.to_string()),
                    messenger: messenger,
                    bound_map: DashMap::new(),
                });
                Ok(())
            }
            Entry::Occupied(entry) => {
                Err(Error::MessengerAlreadyRegistered(entry.key().to_string()))
            }
        }
    }

    pub async fn bind_agent_user(&self, connect_context: &ConnectContext, bind_request: BindRequestDTO) -> Result<Vec<Arc<ChannelInfo>>> {
        let agent_id = Arc::new(bind_request.agent_id);

        //检查agent是否已绑定
        let agent_context = match self.agent_map.entry(agent_id.as_str().to_string()) {
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

        let messenger_context = self.messenger_map.get(bind_request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(bind_request.messenger_id.to_string()))?;
        
        let messenger_info = messenger_context.messenger.get_info().await?;

        let user_info = messenger_info.user_map.get(bind_request.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(bind_request.user_id.to_string()))?;

        let mut channel_infos = Vec::new();
        
        //绑定用户
        let curr_agent_id = messenger_context.bound_map.entry(bind_request.user_id).or_insert_with(|| agent_context.agent_id.clone());

        if curr_agent_id.as_str() == agent_context.agent_id.as_str() {
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
                    let channel = create_channel(messenger_context.messenger.as_ref(), user_info.user_id.as_str(), group_info.group_id.as_str(), group_map.value()).await?;
                    let channel_info = channel.get_info().await?;
                    channel_infos.push(channel_info);
                }
            }
        }

        Ok(channel_infos)
    }

    pub async fn unbind_agent_user(&self, connect_context: &ConnectContext, bind_request: BindRequestDTO) {
        //移除channel
        if let Some(agent_context) = connect_context.agent_map.get(bind_request.agent_id.as_str()) {
            if let Some(user_map) = agent_context.messenger_user_group_channel_map.get(bind_request.messenger_id.as_str()) {
                user_map.remove(bind_request.user_id.as_str());
            }
        }

        //解除绑定
        if let Some(messenger_context) = self.messenger_map.get(bind_request.messenger_id.as_str()) {
            messenger_context.bound_map.remove(bind_request.user_id.as_str());
        }
    }

    pub async fn handle_group_change(&self, event: &GroupChangeEvent) -> Result<()> {
        if let Some(messenger_context) = self.messenger_map.get(&event.messenger_id) {
            if let Some(agent_id) = messenger_context.bound_map.get(event.user_id.as_str()) {
                if let Some(agent_context) = self.agent_map.get(agent_id.as_str()){
                    if let Some(connect_context) = self.connect_map.get(&agent_context.connect_id){
                        if let Some(user_map) = agent_context.messenger_user_group_channel_map.get(event.messenger_id.as_str()){
                            if let Some(group_map) = user_map.get(event.user_id.as_str()){
                                match event.change_type {
                                    GroupChangeType::Joined => {
                                        //新建channel
                                        let channel = create_channel(messenger_context.messenger.as_ref(), event.user_id.as_str(), event.group_id.as_str(), group_map.value()).await?;
                                        let channel_info = channel.get_info().await?;
                                        //给agent发消息
                                        connect_context.ws_context.send_json(WsMessage {
                                            sn: connect_context.ws_context.next_request_sn(),
                                            payload_type: TYPE_JOINT_GROUP,
                                            payload: serde_json::to_value(channel_info)?,
                                        });
                                        //传递消息到channel
                                        channel.forward_group_message(event).await?;
                                    }
                                    GroupChangeType::Left => {
                                        //退出group
                                        if let Some((_,channel)) = group_map.remove(&event.group_id) {
                                            //传递消息到channel
                                            channel.channel.forward_group_message(event).await?;
                                            //给agent发消息
                                            connect_context.ws_context.send_json(WsMessage {
                                                sn: connect_context.ws_context.next_request_sn(),
                                                payload_type: TYPE_LEFT_GROUP,
                                                payload: serde_json::to_value(channel_info)?,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    
    pub async fn handle_agent_message(&self, agent_id: &str, wss_msg: WssMessage) -> Result<()> {
        match wss_msg.r#type.as_str() {
            "outgoing_message" => {
                let outgoing_data: OutgoingMessageData = serde_json::from_value(wss_msg.data)?;
                
                if let Some(channel) = self.channel_registry.get(&outgoing_data.channel_id) {
                    let outgoing_msg = OutgoingMessage {
                        channel_id: outgoing_data.channel_id.clone(),
                        target_user_id: None,
                        msg_type: outgoing_data.msg_type.clone(),
                        content: outgoing_data.content.clone(),
                        attachments: outgoing_data.attachments.clone(),
                    };
                    
                    channel.send_message(outgoing_msg).await?;
                    
                    // Also create a MessageRecord for memory store
                    let record = MessageRecord {
                        channel_id: outgoing_data.channel_id.clone(),
                        user_id: agent_id.to_string(),
                        is_self: 1,
                        msg_type: outgoing_data.msg_type,
                        content: outgoing_data.content,
                        attachments: outgoing_data.attachments,
                        timestamp: Utc::now(),
                    };
                    
                    // Push to memory store
                    if let Some(store) = &self.memory_store_client {
                        store.push_message_records(agent_id, "default", &[record]).await?;
                    }
                } else {
                    return Err(crate::error::ChannelError::ChannelNotFound(outgoing_data.channel_id));
                }
            }
            "get_channels" => {
                let channels = self.get_all_channels(agent_id, None);
                let channels_data = ChannelsData { channels };
                let wss_response = WssMessage {
                    r#type: "channels".to_string(),
                    data: serde_json::to_value(channels_data).unwrap(),
                };
                self.wss_server.send_to_agent(agent_id, wss_response)?;
            }
            "attachment_download" => {
                let download_data: AttachmentDownloadData = serde_json::from_value(wss_msg.data)?;
                for messenger in self.messenger_map.list() {
                    if let Ok(attachment) = messenger.get_attachment_data(&download_data.key).await {
                        let attachment_data = AttachmentData {
                            key: download_data.key.clone(),
                            filename: attachment.name,
                            mime_type: attachment.mime_type,
                            data: base64::engine::general_purpose::STANDARD.encode(&attachment.data),
                        };
                        
                        let wss_response = WssMessage {
                            r#type: "attachment_data".to_string(),
                            data: serde_json::to_value(attachment_data).unwrap(),
                        };
                        
                        self.wss_server.send_to_agent(agent_id, wss_response)?;
                        break;
                    }
                }
            }
            _ => {}
        }
        
        Ok(())
    }
    
    pub fn enqueue_message(&self, channel_id: &str, record: MessageRecord) {
        self.message_queues
            .entry(channel_id.to_string())
            .or_insert_with(Vec::new)
            .push(record);
    }
    
    pub async fn process_message_queue(&self, channel_id: Option<&str>, agent_id: &str) -> Result<()> {
        let channel_ids = if let Some(cid) = channel_id {
            vec![cid.to_string()]
        } else {
            self.message_queues.iter().map(|r| r.key().clone()).collect()
        };
        
        for cid in channel_ids {
            if let Some(mut queue) = self.message_queues.get_mut(&cid) {
                let records: Vec<MessageRecord> = queue.drain(..).collect();
                
                if !records.is_empty() {
                    // Push to memory store
                    if let Some(store) = &self.memory_store_client {
                        store.push_message_records(agent_id, "default", &records).await?;
                    }
                    
                    // Send to agent via WSS
                    for record in records {
                        let incoming_data = IncomingMessageData {
                            channel_id: record.channel_id,
                            user_id: record.user_id,
                            is_self: record.is_self,
                            msg_type: record.msg_type,
                            content: record.content,
                            timestamp: record.timestamp,
                        };
                        
                        let wss_msg = WssMessage {
                            r#type: "incoming_message".to_string(),
                            data: serde_json::to_value(incoming_data).unwrap(),
                        };
                        
                        self.wss_server.send_to_agent(agent_id, wss_msg)?;
                    }
                }
            }
        }
        
        Ok(())
    }
}
