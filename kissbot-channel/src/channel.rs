use crate::error::Result;
use crate::types::*;
use std::sync::Arc;

// Channel trait
#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    fn channel_id(&self) -> &str;
    fn messenger_id(&self) -> &str;
    fn agent_id(&self) -> &str;
    fn group_id(&self) -> &str;
    fn user_id(&self) -> &str;
    
    async fn send_message(&self, message: OutgoingMessage) -> Result<()>;
    
    async fn get_status(&self) -> Result<ChannelStatus>;
}

pub struct ChannelRegistry {
    channels: dashmap::DashMap<String, Arc<dyn Channel + Send + Sync>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            channels: dashmap::DashMap::new(),
        }
    }
    
    pub fn register(&self, channel: Arc<dyn Channel + Send + Sync>) {
        self.channels.insert(channel.channel_id().to_string(), channel);
    }
    
    pub fn get(&self, channel_id: &str) -> Option<Arc<dyn Channel + Send + Sync>> {
        self.channels.get(channel_id).map(|r| r.value().clone())
    }
    
    pub fn list_by_agent(&self, agent_id: &str) -> Vec<Arc<dyn Channel + Send + Sync>> {
        self.channels
            .iter()
            .filter(|r| r.value().agent_id() == agent_id)
            .map(|r| r.value().clone())
            .collect()
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}
