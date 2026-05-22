use crate::error::Result;
use crate::data::*;
use crate::channel::Channel;
use std::sync::Arc;

// Callbacks
pub type OnMessageReceived = Arc<dyn Fn(MessageRecord) + Send + Sync>;
pub type OnGroupChange = Arc<dyn Fn(GroupChangeEvent) + Send + Sync>;

// Messenger trait
#[async_trait::async_trait]
pub trait Messenger: Send + Sync {
    fn messenger_id(&self) -> &str;
    
    async fn get_available_users(&self) -> Result<Vec<UserInfo>>;
    
    async fn get_user_groups(&self, user_id: &str) -> Result<Vec<GroupInfo>>;
    
    async fn create_channel(
        &self,
        agent_id: String,
        user_id: String,
        group_id: String,
        on_message_received: OnMessageReceived,
    ) -> Result<Arc<dyn Channel + Send + Sync>>;
    
    fn register_on_group_change(&self, callback: OnGroupChange);
    
    async fn get_attachment_metadata(&self, key: &str) -> Result<AttachmentRef>;
    
    async fn get_attachment_data(&self, key: &str) -> Result<Attachment>;
}

pub struct MessengerRegistry {
    messengers: dashmap::DashMap<String, Arc<dyn Messenger>>,
}

impl MessengerRegistry {
    pub fn new() -> Self {
        Self {
            messengers: dashmap::DashMap::new(),
        }
    }
    
    pub fn register(&self, messenger: Arc<dyn Messenger>) {
        self.messengers.insert(messenger.messenger_id().to_string(), messenger);
    }
    
    pub fn get(&self, messenger_id: &str) -> Option<Arc<dyn Messenger>> {
        self.messengers.get(messenger_id).map(|r| r.clone())
    }
    
    pub fn list(&self) -> Vec<Arc<dyn Messenger>> {
        self.messengers.iter().map(|r| r.clone()).collect()
    }
}

impl Default for MessengerRegistry {
    fn default() -> Self {
        Self::new()
    }
}
