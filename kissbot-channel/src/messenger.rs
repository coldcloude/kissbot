use crate::error::Result;
use crate::data::*;
use crate::channel::Channel;
use std::sync::{Arc, Weak};

// Messenger trait
#[async_trait::async_trait]
pub trait Messenger: Send + Sync {
    fn messenger_id(&self) -> &str;

    async fn get_info(&self) -> Result<Arc<MessengerInfo>>;
    
    async fn create_channel(&self, user_id: &str, group_id: &str) -> Result<Arc<dyn Channel>>;
    
    fn register_on_group_change(&self, callback: Weak<dyn GroupChangeHandler>);
}
