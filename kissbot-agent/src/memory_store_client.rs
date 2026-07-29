use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileObjectAppender, NoopErrorHandler, appender::FileAppendWriterContext};
use kissbot_api::memory::{ChannelRequest, ChannelRequests};
use kissbot_security::HEADER_API_KEY;
use kissbot_api::Content;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

// 批处理参数
const RECORD_QUEUE_SIZE: usize = 100;
const RECORD_MAX_DELAY: Duration = Duration::from_secs(1);
const FILE_KEY: &str = "0";

/// 单个 channel 记录（与 ChannelRequest 同构，用于 push_channel_record 接口）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub content: Content,
    pub time: Arc<String>,
}

pub struct MemoryStoreClient {
    appender: FileObjectAppender<String, ChannelRecord, MemoryStoreSender, MemoryStoreContext>,
}

impl MemoryStoreClient {
    pub fn new() -> Self {
        let sender = Arc::new(MemoryStoreSender::new());
        Self {
            appender: FileObjectAppender::new(sender, Arc::new(NoopErrorHandler {}), RECORD_MAX_DELAY, RECORD_QUEUE_SIZE),
        }
    }

    pub async fn push_channel_record(&self, record: ChannelRecord) {
        self.appender.append(FILE_KEY.to_string(), vec![record]).await;
    }
}

struct MemoryStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

pub struct MemoryStoreSender {
    context: Arc<Mutex<MemoryStoreContext>>,
}

impl MemoryStoreSender {
    pub fn new() -> Self {
        let api_config = kissbot_api::ApiConfig::get();
        let security = kissbot_security::SecurityConfig::get();
        let ctx = MemoryStoreContext {
            client: Client::new(),
            base_url: api_config.memory_store_url.clone(),
            api_key: security.api_key.clone(),
        };
        Self {
            context: Arc::new(Mutex::new(ctx)),
        }
    }
}

#[async_trait]
impl FileAppendWriter<String, ChannelRecord, MemoryStoreContext> for MemoryStoreSender {
    async fn get_lock(&self, _key: &String) -> Arc<Mutex<MemoryStoreContext>> {
        self.context.clone()
    }
    async fn remove_lock(&self, _key: &String) {}
}

#[async_trait]
impl FileAppendWriterContext<String, ChannelRecord> for MemoryStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ChannelRecord>) -> std::result::Result<(), kai_file::Error> {
        if self.base_url.is_empty() {
            return Ok(());
        }

        let requests: Vec<ChannelRequest> = records.into_iter().map(|r| ChannelRequest {
            agent_id: r.agent_id,
            role_name: r.role_name,
            messenger_id: r.messenger_id,
            user_id: r.user_id,
            group_id: r.group_id,
            is_self: r.is_self,
            content: r.content,
            time: r.time,
        }).collect();

        let req = ChannelRequests { requests, force: 1 };
        let url = format!("{}/store/channel", self.base_url.trim_end_matches('/'));
        let response = self.client.post(&url)
            .header(HEADER_API_KEY, self.api_key.as_str())
            .json(&req)
            .send()
            .await
            .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let msg = response.text().await.unwrap_or_default();
            return Err(kai_file::Error::WriteError(format!("[{}] {}", status, msg)));
        }
        Ok(())
    }
}
