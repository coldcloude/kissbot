use crate::{Error, IncomingMessages, error::Result};
use flume::{Receiver, Sender, bounded};
use kissbot_api::{SyncString, store::*};
use kissbot_security::HEADER_API_KEY;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{collections::LinkedList, sync::Arc};

const RECORD_QUEUE_SIZE: usize = 10000;

pub type ChannelRequest = ChannelRequestGeneric<SyncString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChannelRequest;

impl ChannelRequestKind<SyncString> for SyncChannelRequest {
    type Type = ChannelRequest;
}

pub type ChannelRequests = ChannelRequestsGeneric<SyncString, SyncChannelRequest>;

pub struct MessagesRecord {
    agent_id: Arc<String>,
    role_name: Arc<String>,
    messages: Arc<IncomingMessages>,
}

pub struct MemoryStoreClient {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
    messages_queue: (Sender<MessagesRecord>, Receiver<MessagesRecord>),

}

impl MemoryStoreClient {
    pub fn new(base_url: &str, api_key: Arc<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.to_string(),
            api_key,
            messages_queue: bounded::<MessagesRecord>(RECORD_QUEUE_SIZE),
        }
    }

    pub async fn push_messages(&self, agent_id: Arc<String>, role_name: Arc<String>, messages: Arc<IncomingMessages>) -> Result<()> {
        self.messages_queue.0.send_async(MessagesRecord {
            agent_id,
            role_name,
            messages,
        }).await?;
        Ok(())
    }
    
    pub async fn start_send_messages(&self) -> Result<()> {
        loop {
            let record = self.messages_queue.1.recv_async().await?;
            let mut record_list = LinkedList::new();
            let mut current_record = Some(record);
            while let Some(record) = current_record {
                record_list.push_back(record);
                if let Ok(record) = self.messages_queue.1.try_recv() {
                    current_record = Some(record);
                } else {
                    current_record = None;
                }
            }
            let mut size = 0;
            for record in record_list.iter() {
                size += record.messages.len();
            }
            let mut requests = Vec::with_capacity(size);
            while let Some(record) = record_list.pop_front() {
                for message in record.messages.iter() {
                    requests.push(ChannelRequest {
                        agent_id: record.agent_id.clone(),
                        role_name: record.role_name.clone(),
                        messenger_id: message.messenger_id.clone(),
                        user_id: message.user_id.clone(),
                        group_id: message.group_id.clone(),
                        is_self: message.is_self,
                        msg_type: message.msg_type.clone(),
                        content: message.content.clone(),
                        time: message.time.clone(),
                    });
                }
            }
        
            let req = ChannelRequests {
                requests,
                force: 0,
            };
            
            let url = format!("{}/store/channel", self.base_url);
            let response = self.client.post(&url)
                .header(HEADER_API_KEY, self.api_key.as_str())
                .json(&req).send().await?;
            
            if !response.status().is_success() {
                let err_msg = format!("Failed to push message records: [{}] {}", response.status(), response.text().await?);
                return Err(Error::RequestError(err_msg));
            }
        }
    }
}
