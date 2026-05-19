use crate::error::Result;
use crate::types::*;
use kissbot_api::store::*;
use reqwest::Client;
use std::sync::Arc;

#[derive(Clone)]
pub struct MemoryStoreClient {
    client: Arc<Client>,
    base_url: String,
}

impl MemoryStoreClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Arc::new(Client::new()),
            base_url,
        }
    }
    
    pub async fn push_message_records(&self, agent_id: &str, role_name: &str, records: &[MessageRecord]) -> Result<()> {
        let channel_requests: Vec<ChannelRequest> = records
            .iter()
            .map(|record| ChannelRequest {
                agent_id: agent_id.to_string(),
                role_name: role_name.to_string(),
                channel_id: record.channel_id.clone(),
                user_id: record.user_id.clone(),
                is_self: record.is_self,
                msg_type: record.msg_type.clone(),
                content: record.content.clone(),
                time: record.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
            })
            .collect();
        
        let req = ChannelRequests {
            requests: channel_requests,
            force: 0,
        };
        
        let url = format!("{}/store/channel", self.base_url);
        let response = self.client.post(&url).json(&req).send().await?;
        
        if !response.status().is_success() {
            return Err(crate::error::ChannelError::Other(
                format!("Failed to push message records: {}", response.status())
            ));
        }
        
        Ok(())
    }
}
