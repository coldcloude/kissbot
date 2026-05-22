use crate::{Error, IncomingMessage, error::Result};
use kissbot_api::{SyncString, store::*};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type ChannelRequest = ChannelRequestGeneric<SyncString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChannelRequest;

impl ChannelRequestKind<SyncString> for SyncChannelRequest {
    type Type = ChannelRequest;
}

pub type ChannelRequests = ChannelRequestsGeneric<SyncString, SyncChannelRequest>;

pub struct MemoryStoreClient {
    client: Client,
    base_url: String,
}

impl MemoryStoreClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }
    
    pub async fn push_message_records(&self, agent_id: &str, role_name: &str, records: &[IncomingMessage]) -> Result<()> {
        let channel_requests: Vec<ChannelRequest> = records
            .iter()
            .map(|record| ChannelRequest {
                agent_id: Arc::new(agent_id.to_string()),
                role_name: Arc::new(role_name.to_string()),
                channel_id: record.channel_id.clone(),
                user_id: record.user_id.clone(),
                is_self: record.is_self,
                msg_type: record.msg_type.clone(),
                content: record.content.clone(),
                time: record.time.clone(),
            })
            .collect();
        
        let req = ChannelRequests {
            requests: channel_requests,
            force: 0,
        };
        
        let url = format!("{}/store/channel", self.base_url);
        let response = self.client.post(&url).json(&req).send().await?;
        
        if !response.status().is_success() {
            let err_msg = format!("Failed to push message records: [{}] {}", response.status(), response.text().await?);
            return Err(Error::RequestError(err_msg));
        }
        
        Ok(())
    }
}
