use crate::{Error, error::Result};
use crate::messenger::Messenger;
use crate::channel::Channel;
use crate::data::*;
use crate::memory_store_client::MemoryStoreClient;
use crate::wss_server::WssServer;
use async_trait::async_trait;
use dashmap::{DashMap, DashSet, Entry};
use flume::{Receiver, Sender, bounded};
use kai_ws::{TYPE_HEARTBEAT, WsCloseProcessor, WsContext, WsHeartbeatHandler, WsProcessorInitializer, ws_handle_connection};
use kissbot_api::channel::BindRequestDTO;
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
    channel_info: Arc<ChannelInfo>,
    channel: Arc<dyn Channel>,
    memory_store_queue: (Sender<Vec<IncomingMessage>>, Receiver<Vec<IncomingMessage>>),
    agent_queue: (Sender<Vec<IncomingMessage>>, Receiver<Vec<IncomingMessage>>),
}

struct AgentContext {
    agent_id: Arc<String>,
    messenger_info_map: DashMap<String, Arc<MessengerInfo>>,
    channel_map: DashMap<String, ChannelContext>,
}

struct ConnectContext {
    connect_id: u32,
    ws_context: Arc<WsContext>,
    agent_map: DashMap<String, AgentContext>,
}

pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, ConnectContext>,
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
        manager.connect_map.insert(connect_id, ConnectContext {
            connect_id,
            ws_context: ws_context.clone(),
            agent_map: DashMap::new(),
        });
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
                            messenger_context.bound_map.remove(user_id.as_str());
                        }
                    }
                    else {
                        error!("Messenger not found: connect_id {}, messenger_id {}", connect_id, messenger_id);
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

    pub async fn bind_agent_user(&self, connect_id: u32, bind_request: BindRequestDTO) -> Result<Vec<Arc<ChannelInfo>>> {
        let connect_context = self.connect_map.get(&connect_id)
            .ok_or_else(|| Error::ConnectNotFound(connect_id.to_string()))?;

        let agent_context = connect_context.agent_map.entry(bind_request.agent_id.as_str().to_string()).or_insert_with(|| AgentContext {
            agent_id: Arc::new(bind_request.agent_id),
            channel_map: DashMap::new(),
            messenger_info_map: DashMap::new(),
        });

        let messenger_context = self.messenger_map.get(bind_request.messenger_id.as_str())
            .ok_or_else(|| Error::MessengerNotFound(bind_request.messenger_id.to_string()))?;
        
        let messenger_info = messenger_context.messenger.get_info().await?;

        let user_info = messenger_info.user_map.get(bind_request.user_id.as_str())
            .ok_or_else(|| Error::UserNotFound(bind_request.user_id.to_string()))?;

        let mut channel_infos = Vec::new();
        
        let curr_agent_id = messenger_context.bound_map.entry(bind_request.user_id).or_insert_with(|| agent_context.agent_id.clone());
        if curr_agent_id.as_str() == agent_context.agent_id.as_str() {
            //成功绑定到自身
            let target_messenger_info = agent_context.messenger_info_map.entry(bind_request.messenger_id.as_str().to_string()).or_insert_with(|| Arc::new(MessengerInfo {
                messenger_id: messenger_info.messenger_id.clone(),
                messenger_name: messenger_info.messenger_name.clone(),
                user_map: Arc::new(DashMap::new()),
            }));
            let mut inserted = false;
            target_messenger_info.user_map.entry(user_info.user_id.as_str().to_string()).or_insert_with(|| {
                inserted = true;
                user_info.clone()
            });
            if inserted {
                //是新绑定的user，创建channels
                for group_info in user_info.group_map.iter() {
                    //新建channel
                    let channel_id = Arc::new(format_channel_id(&messenger_info.messenger_id, &user_info.user_id, &group_info.group_id));
                    let channel = messenger_context.messenger.create_channel(&user_info.user_id, &group_info.group_id).await?;
                    let channel_info = Arc::new(ChannelInfo {
                        channel_id: channel_id.clone(),
                        messenger_id: messenger_info.messenger_id.clone(),
                        user_id: user_info.user_id.clone(),
                        group_id: group_info.group_id.clone(),
                    });
                    //放入返回值
                    channel_infos.push(channel_info.clone());
                    //记录context
                    agent_context.channel_map.insert(channel_id.as_str().to_string(), ChannelContext {
                        channel,
                        channel_info,
                        agent_queue: bounded(MSG_QUEUE_SIZE),
                        memory_store_queue: bounded(MSG_QUEUE_SIZE),
                    });
                }
            }
        }

        Ok(channel_infos)
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
