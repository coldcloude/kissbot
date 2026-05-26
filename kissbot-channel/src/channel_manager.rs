use crate::error::Result;
use crate::messenger::Messenger;
use crate::channel::Channel;
use crate::data::*;
use crate::memory_store_client::MemoryStoreClient;
use crate::wss_server::WssServer;
use async_trait::async_trait;
use dashmap::{DashMap, Entry};
use flume::{Receiver, Sender};
use kai_ws::{TYPE_HEARTBEAT, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsProcessorInitializer, ws_handle_connection};
use tracing::{Level, error, info, span};
use std::sync::{Arc, Weak, atomic::{AtomicU32, Ordering}};
use base64::Engine;
use tokio::{net::TcpListener, time::{Duration}};


const MSG_QUEUE_SIZE: usize = 100;

static INTERVAL: Duration = Duration::from_secs(10);

struct MessengerContext {
    pub info: Arc<MessengerInfo>,
    pub messenger: Arc<dyn Messenger>,
}

struct ChannelContext {
    pub info: Arc<ChannelInfo>,
    pub channel: Arc<dyn Channel>,
    pub memory_store_queue: (Sender<Vec<IncomingMessage>>, Receiver<Vec<IncomingMessage>>),
    pub agent_queue: (Sender<Vec<IncomingMessage>>, Receiver<Vec<IncomingMessage>>),
}

struct AgentContext {
    agent_id: Arc<String>,
    messenger_info_map: DashMap<String, Arc<MessengerInfo>>,
    channel_map: DashMap<String, Arc<ChannelContext>>,
}

pub struct ConnectContext {
    pub connect_id: u32,
    pub ws_context: Arc<WsContext>,
    agent_map: DashMap<String, Arc<AgentContext>>,
}

pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
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
        manager.connect_map.insert(connect_id, Arc::new(ConnectContext {
            connect_id,
            ws_context: ws_context.clone(),
            agent_map: DashMap::new(),
        }));
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

impl ChannelManager {
    pub fn new(memory_store_base_url: &str) -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
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
        if let Some((_,connect_context)) = self.connect_map.remove(&connect_id) {
            for agent_context in connect_context.agent_map.iter() {
                //把agent绑定的用户移回到Messenger控制
                for messenger_info in agent_context.messenger_info_map.iter() {
                    let messenger_id = messenger_info.messenger_id.as_str().to_string();
                    //要求messenger必须存在
                    if let Some(messenger_context) = self.messenger_map.get(&messenger_id) {
                        for user_info in messenger_info.user_map.iter() {
                            let user_id = user_info.user_id.as_str().to_string();
                            messenger_context.info.user_map.insert(user_id, user_info.clone());
                        }
                    }
                    else {
                        error!("Messenger not found: connect_id {}, messenger_id {}", connect_id, messenger_id);
                    }
                }
            }
        }
    }
    
    pub fn setup_wss_callback(self: &Arc<Self>) {
        let manager_clone = self.clone();
        let _callback: WssOnMessageReceived = Arc::new(move |agent_id: String, msg: WssMessage| {
            let manager = manager_clone.clone();
            tokio::spawn(async move {
                let _ = manager.handle_agent_message(&agent_id, msg).await;
            });
        });
        
        // We need to use unsafe or interior mutability here
        // For simplicity, let's create a new WssServer with the callback
        // But this is just a demonstration
    }
    
    pub fn register_messenger(&self, messenger: Arc<dyn Messenger>) {
        // Register on_group_change callback
        let _wss_server = self.wss_server.clone();
        let callback: GroupChangeHandler = Arc::new(move |_event: GroupChangeEvent| {
            // In a real implementation, we'd track which agents are interested
        });
        
        messenger.register_on_group_change(callback);
        self.messenger_map.register(messenger);
    }
    
    pub fn get_messenger(&self, messenger_id: &str) -> Option<Arc<dyn Messenger>> {
        self.messenger_map.get(messenger_id)
    }
    
    pub async fn handle_agent_bind(
        &self,
        agent_id: String,
        messenger_id: String,
        user_ids: Vec<String>,
    ) -> Result<Vec<ChannelInfo>> {
        let messenger = self.get_messenger(&messenger_id)
            .ok_or_else(|| crate::error::ChannelError::MessengerNotFound(messenger_id.clone()))?;
        
        let mut channel_infos = Vec::new();
        let mut channel_ids = Vec::new();
        
        for user_id in user_ids {
            let groups = messenger.get_user_groups(&user_id).await?;
            
            for group in groups {
                let channel_id = format_channel_id(&messenger_id, &group.group_id, &user_id);
                
                // Create on_message_received callback
                let wss_server = self.wss_server.clone();
                let memory_store_client = self.memory_store_client.clone();
                let agent_id_clone = agent_id.clone();
                
                let on_msg_received: OnMessageReceived = Arc::new(move |record: MessageRecord| {
                    let wss_server_clone = wss_server.clone();
                    let memory_store_clone = memory_store_client.clone();
                    let agent_id_clone2 = agent_id_clone.clone();
                    let record_clone = record.clone();
                    
                    tokio::spawn(async move {
                        // Send to agent via WSS
                        let incoming_data = IncomingMessageData {
                            channel_id: record_clone.channel_id.clone(),
                            user_id: record_clone.user_id.clone(),
                            is_self: record_clone.is_self,
                            msg_type: record_clone.msg_type.clone(),
                            content: record_clone.content.clone(),
                            timestamp: record_clone.timestamp,
                        };
                        
                        let wss_msg = WssMessage {
                            r#type: "incoming_message".to_string(),
                            data: serde_json::to_value(incoming_data).unwrap(),
                        };
                        
                        let _ = wss_server_clone.send_to_agent(&agent_id_clone2, wss_msg);
                        
                        // Push to memory store
                        if let Some(store) = memory_store_clone {
                            let _ = store.push_message_records(&agent_id_clone2, "default", &[record_clone]).await;
                        }
                    });
                });
                
                let channel = messenger.create_channel(
                    agent_id.clone(),
                    user_id.clone(),
                    group.group_id.clone(),
                    on_msg_received,
                ).await?;
                
                let channel_info = ChannelInfo {
                    channel_id: channel_id.clone(),
                    messenger_id: messenger_id.clone(),
                    group_id: group.group_id.clone(),
                    group_name: group.group_name.clone(),
                    user_id: user_id.clone(),
                };
                
                self.channel_registry.register(channel);
                channel_infos.push(channel_info);
                channel_ids.push(channel_id);
            }
        }
        
        self.agent_channel_map.insert(agent_id.clone(), channel_ids);
        
        // Send bind ack to agent
        let bind_ack_data = BindAckData {
            agent_id: agent_id.clone(),
            channels: channel_infos.clone(),
        };
        
        let wss_msg = WssMessage {
            r#type: "bind_ack".to_string(),
            data: serde_json::to_value(bind_ack_data).unwrap(),
        };
        
        self.wss_server.send_to_agent(&agent_id, wss_msg)?;
        
        Ok(channel_infos)
    }
    
    pub fn get_all_channels(&self, agent_id: &str, _messenger_id: Option<&str>) -> Vec<ChannelInfo> {
        let mut channel_infos = Vec::new();
        
        for channel in self.channel_registry.list_by_agent(agent_id) {
            channel_infos.push(ChannelInfo {
                channel_id: channel.channel_id().to_string(),
                messenger_id: channel.messenger_id().to_string(),
                group_id: channel.group_id().to_string(),
                group_name: "".to_string(),
                user_id: channel.user_id().to_string(),
            });
        }
        
        channel_infos
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
