use crate::error::Result;
use crate::data::*;
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::protocol::Message};

// Agent connection
#[derive(Clone)]
pub struct AgentConnection {
    pub agent_id: String,
    pub sender: mpsc::UnboundedSender<WssMessage>,
}

pub type WssOnMessageReceived = Arc<dyn Fn(String, WssMessage) + Send + Sync>;

pub struct WssServer {
    agent_connections: DashMap<String, AgentConnection>,
    on_message_received: Option<WssOnMessageReceived>,
}

impl WssServer {
    pub fn new() -> Self {
        Self {
            agent_connections: DashMap::new(),
            on_message_received: None,
        }
    }
    
    pub fn register_on_message_received(&mut self, callback: WssOnMessageReceived) {
        self.on_message_received = Some(callback);
    }
    
    pub async fn start(&self, addr: &str) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        println!("WSS Server listening on: {}", addr);
        
        while let Ok((stream, _)) = listener.accept().await {
            let ws_stream = accept_async(stream).await?;
            let (ws_sender, mut ws_receiver) = ws_stream.split();
            let (tx, mut rx) = mpsc::unbounded_channel();
            
            // Spawn task to receive messages from agent
            let agent_connections = self.agent_connections.clone();
            let on_message_received = self.on_message_received.clone();
            
            tokio::spawn(async move {
                let mut agent_id: Option<String> = None;
                
                // Handle outgoing messages to agent
                let mut ws_sender = ws_sender;
                tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = ws_sender.send(Message::Text(json.into())).await;
                        }
                    }
                });
                
                // Handle incoming messages from agent
                while let Some(Ok(msg)) = ws_receiver.next().await {
                    if let Message::Text(text) = msg {
                        if let Ok(wss_msg) = serde_json::from_str::<WssMessage>(&text) {
                            match wss_msg.r#type.as_str() {
                                "bind" => {
                                    if let Ok(bind_data) = serde_json::from_value::<BindData>(wss_msg.data.clone()) {
                                        agent_id = Some(bind_data.agent_id.clone());
                                        agent_connections.insert(
                                            bind_data.agent_id.clone(),
                                            AgentConnection {
                                                agent_id: bind_data.agent_id.clone(),
                                                sender: tx.clone(),
                                            }
                                        );
                                    }
                                }
                                "ping" => {
                                    let pong = WssMessage {
                                        r#type: "pong".to_string(),
                                        data: serde_json::Value::Null,
                                    };
                                    let _ = tx.send(pong);
                                }
                                _ => {
                                    if let (Some(aid), Some(cb)) = (&agent_id, &on_message_received) {
                                        cb(aid.clone(), wss_msg);
                                    }
                                }
                            }
                        }
                    }
                }
                
                // Clean up when connection closes
                if let Some(aid) = agent_id {
                    agent_connections.remove(&aid);
                }
            });
        }
        
        Ok(())
    }
    
    pub fn send_to_agent(&self, agent_id: &str, message: WssMessage) -> Result<()> {
        if let Some(conn) = self.agent_connections.get(agent_id) {
            conn.sender.send(message).map_err(|e| crate::error::ChannelError::SendError(e.to_string()))?;
            Ok(())
        } else {
            Err(crate::error::ChannelError::AgentNotConnected(agent_id.to_string()))
        }
    }
    
    pub fn is_agent_connected(&self, agent_id: &str) -> bool {
        self.agent_connections.contains_key(agent_id)
    }
}

impl Default for WssServer {
    fn default() -> Self {
        Self::new()
    }
}
