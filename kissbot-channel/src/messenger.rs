use dashmap::DashMap;

use crate::error::Result;
use crate::data::*;
use crate::channel::Channel;
use std::sync::Arc;

// Messenger trait
#[async_trait::async_trait]
pub trait Messenger: Send + Sync {
    fn messenger_id(&self) -> &str;

    async fn get_user_names(&self) -> Result<Arc<DashMap<String, Arc<String>>>>;
    
    async fn get_user_info(&self, user_id: &str) -> Result<Arc<UserInfo>>;
    
    async fn create_channel(&self, agent_id: String, user_id: String, group_id: String) -> Result<Arc<dyn Channel>>;
    
    fn register_on_group_change(&self, callback: Arc<dyn GroupChangeHandler>);
}
