use crate::error::Result;
use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileObjectAppender, NoopErrorHandler, appender::FileAppendWriterContext};
use kissbot_api::{ChannelRequest, ChannelRequests, IncomingMessage};
use kissbot_security::HEADER_API_KEY;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use std::{sync::Arc, time::Duration};

const RECORD_QUEUE_SIZE: usize = 100;
const RECORD_MAX_DELAY: Duration = Duration::from_secs(1);

const FILE_KEY: &str = "0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    agent_id: Arc<String>,
    role_name: Arc<String>,
    message: Arc<IncomingMessage>,
}

pub struct MemorySender {
    context: Arc<Mutex<MemorySenderContext>>,
}

pub struct MemorySenderContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

pub struct MemoryStoreClient {
    message_appender: FileObjectAppender<String, MessageRecord, MemorySender, MemorySenderContext>,
}

impl MemorySender {
    pub fn new() -> Self {
        let api_config = kissbot_api::ApiConfig::get();
        let security = kissbot_security::SecurityConfig::get();
        let ctx = MemorySenderContext {
            client: Client::new(),
            base_url: api_config.memory_store_url.clone(),
            api_key: security.api_key.clone(),
        };
        Self {
            context: Arc::new(Mutex::new(ctx)),
        }
    }
}

impl MemoryStoreClient {
    pub fn new() -> Self {
        let sender = Arc::new(MemorySender::new());
        Self {
            message_appender: FileObjectAppender::new(sender, Arc::new(NoopErrorHandler {}), RECORD_MAX_DELAY, RECORD_QUEUE_SIZE),
        }
    }

    pub async fn push_messages(&self, agent_id: Arc<String>, role_name: Arc<String>, message: Arc<IncomingMessage>) -> Result<()> {
        self.message_appender.append(FILE_KEY.to_string(), vec![MessageRecord {
            agent_id,
            role_name,
            message,
        }]).await;
        Ok(())
    }
}

#[async_trait]
impl FileAppendWriter<String, MessageRecord, MemorySenderContext> for MemorySender {
    async fn get_lock(&self, _key: &String) -> Arc<Mutex<MemorySenderContext>> {
        self.context.clone()
    }
    async fn remove_lock(&self, _key: &String) {
        // no op
    }
}

#[async_trait]
impl FileAppendWriterContext<String,MessageRecord> for MemorySenderContext {
    async fn write(&mut self, _key: &String, records: Vec<MessageRecord>) -> std::result::Result<(), kai_file::Error> {
        let mut requests = Vec::with_capacity(records.len());
        for record in records {
            requests.push(ChannelRequest {
                agent_id: record.agent_id.clone(),
                role_name: record.role_name.clone(),
                messenger_id: record.message.messenger_id.clone(),
                user_id: record.message.user_id.clone(),
                group_id: record.message.group_id.clone(),
                is_self: record.message.is_self,
                msg_type: record.message.msg_type.clone(),
                content: record.message.content.clone(),
                time: record.message.time.clone(),
            });
        }
    
        let req = ChannelRequests {
            requests,
            force: 1,
        };
        
        let url = format!("{}/store/channel", self.base_url);
        let response = self.client.post(&url)
            .header(HEADER_API_KEY, self.api_key.as_str())
            .json(&req).send().await.map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?;
        
        if !response.status().is_success() {
            let res_status = response.status();
            let res_msg = match response.text().await { Ok(msg) => msg, Err(e) => format!("Response error: {}", e.to_string()) };
            let err_msg = format!("Failed to push message records: [{}] {}", res_status, res_msg);
            return Err(kai_file::Error::WriteError(err_msg));
        }
        Ok(())
    }
}
