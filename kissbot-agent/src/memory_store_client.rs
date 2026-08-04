use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileAppendWriterContext, FileObjectAppender, ErrorHandler};
use kissbot_api::memory::{
    ChannelRequest, ChannelRequests, ThinkRequest, ThinkRequests,
    ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests,
};
use kissbot_security::HEADER_API_KEY;
use reqwest::Client;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::error;

// 批处理参数
const RECORD_QUEUE_SIZE: usize = 100;
const RECORD_MAX_DELAY: Duration = Duration::from_secs(1);
const CHANNEL_KEY: &str = "0";
const THINK_KEY: &str = "think";
const TOOL_CALL_KEY: &str = "tool-call";
const TOOL_RESULT_KEY: &str = "tool-result";

// ========== 共享发送段（各记录类型复用，只写一遍） ==========

/// 构造共享的 HTTP 配置（client / base_url / api_key，各类型 context 同源）
fn store_http_config() -> (Client, String, Arc<String>) {
    let api_config = kissbot_api::ApiConfig::get();
    let security = kissbot_security::SecurityConfig::get();
    (Client::new(), api_config.memory_store_url.clone(), security.api_key.clone())
}

/// POST {base_url}{path}，带 X-Api-Key 鉴权头；base_url 空则跳过（Ok）；
/// 非 2xx 返回 Err（错误含状态码与返回体）
async fn send_store_request(
    client: &Client,
    base_url: &str,
    api_key: &str,
    path: &str,
    body: &impl Serialize,
) -> std::result::Result<(), kai_file::Error> {
    if base_url.is_empty() {
        return Ok(());
    }
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = client.post(&url)
        .header(HEADER_API_KEY, api_key)
        .json(body)
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

/// 写失败统一日志 handler（替换 NoopErrorHandler，修复写失败静默丢弃）
struct LoggingErrorHandler;
#[async_trait]
impl<K, R> ErrorHandler<K, R> for LoggingErrorHandler
where
    K: std::fmt::Debug + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    async fn on_write_error(&self, key: &K, _batch: Vec<R>, error: &kai_file::Error) {
        error!("记忆写入失败 key={:?}: {}", key, error);
    }
}

// ========== 泛型 sender（各记录类型复用 FileAppendWriter，只写一遍） ==========

pub struct StoreSender<C> {
    context: Arc<Mutex<C>>,
}

impl<C> StoreSender<C> {
    pub fn new(context: C) -> Self {
        Self { context: Arc::new(Mutex::new(context)) }
    }
}

#[async_trait]
impl<K, R, C> FileAppendWriter<K, R, C> for StoreSender<C>
where
    C: FileAppendWriterContext<K, R>,
{
    async fn get_lock(&self, _key: &K) -> Arc<Mutex<C>> {
        self.context.clone()
    }
    async fn remove_lock(&self, _key: &K) {}
}

// ========== MemoryStoreClient（push 直接用 kissbot-api 的 *Request） ==========

pub struct MemoryStoreClient {
    channel_appender: FileObjectAppender<String, ChannelRequest, StoreSender<MemoryStoreContext>, MemoryStoreContext, LoggingErrorHandler>,
    think_appender: FileObjectAppender<String, ThinkRequest, StoreSender<ThinkStoreContext>, ThinkStoreContext, LoggingErrorHandler>,
    tool_call_appender: FileObjectAppender<String, ToolCallRequest, StoreSender<ToolCallStoreContext>, ToolCallStoreContext, LoggingErrorHandler>,
    tool_result_appender: FileObjectAppender<String, ToolResultRequest, StoreSender<ToolResultStoreContext>, ToolResultStoreContext, LoggingErrorHandler>,
}

impl MemoryStoreClient {
    pub fn new() -> Self {
        let (client, base_url, api_key) = store_http_config();
        Self {
            channel_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(MemoryStoreContext { client: client.clone(), base_url: base_url.clone(), api_key: api_key.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            think_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ThinkStoreContext { client: client.clone(), base_url: base_url.clone(), api_key: api_key.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_call_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolCallStoreContext { client: client.clone(), base_url: base_url.clone(), api_key: api_key.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_result_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolResultStoreContext { client, base_url, api_key })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
        }
    }

    pub async fn push_channel_record(&self, record: ChannelRequest) {
        self.channel_appender.append(CHANNEL_KEY.to_string(), vec![record]).await;
    }

    /// 推送 think 记录（reasoning_content + thinking 双字段，key 关联 ChannelRecord(Think)）
    /// 调用方：coordinator 步骤 4（思考记忆推送）
    pub async fn push_think(&self, record: ThinkRequest) {
        self.think_appender.append(THINK_KEY.to_string(), vec![record]).await;
    }

    /// 推送 tool-call 记录（station 工具调用功能落地时消费）
    #[allow(dead_code)]
    pub async fn push_tool_call(&self, record: ToolCallRequest) {
        self.tool_call_appender.append(TOOL_CALL_KEY.to_string(), vec![record]).await;
    }

    /// 推送 tool-result 记录（station 工具调用功能落地时消费）
    #[allow(dead_code)]
    pub async fn push_tool_result(&self, record: ToolResultRequest) {
        self.tool_result_appender.append(TOOL_RESULT_KEY.to_string(), vec![record]).await;
    }
}

struct MemoryStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn channel_requests(records: Vec<ChannelRequest>) -> ChannelRequests {
    ChannelRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ChannelRequest> for MemoryStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ChannelRequest>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/channel", &channel_requests(records)).await
    }
}

// ========== think / tool-call / tool-result 的 context 与 requests 构造 ==========

pub struct ThinkStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn think_requests(records: Vec<ThinkRequest>) -> ThinkRequests {
    ThinkRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ThinkRequest> for ThinkStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ThinkRequest>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/think", &think_requests(records)).await
    }
}

pub struct ToolCallStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn tool_call_requests(records: Vec<ToolCallRequest>) -> ToolCallRequests {
    ToolCallRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ToolCallRequest> for ToolCallStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ToolCallRequest>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/tool-call", &tool_call_requests(records)).await
    }
}

pub struct ToolResultStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn tool_result_requests(records: Vec<ToolResultRequest>) -> ToolResultRequests {
    ToolResultRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ToolResultRequest> for ToolResultStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ToolResultRequest>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/tool-result", &tool_result_requests(records)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_requests_force_is_one() {
        let reqs = think_requests(vec![ThinkRequest {
            agent_id: Arc::new("a1".into()),
            role_name: Arc::new("r1".into()),
            reasoning_content: Arc::new("推理".into()),
            thinking: Arc::new(String::new()),
            key: Arc::new("k1".into()),
            time: Arc::new("2026-08-04 10:00:00".into()),
        }]);
        assert_eq!(reqs.force, 1, "force 统一为 1");
        assert_eq!(reqs.requests[0].reasoning_content.as_str(), "推理");
    }

    #[test]
    fn channel_requests_force_is_one() {
        let reqs = channel_requests(vec![ChannelRequest {
            agent_id: Arc::new("a1".into()),
            role_name: Arc::new("r1".into()),
            messenger_id: Arc::new("web".into()),
            user_id: Arc::new("u1".into()),
            self_user_id: Arc::new("u1".into()),
            group_id: Arc::new("g1".into()),
            is_self: 0,
            messenger_name: Arc::new("".into()),
            user_name: Arc::new("".into()),
            group_name: Arc::new("".into()),
            content: kissbot_api::Content::Text(Arc::new("你好".into())),
            time: Arc::new("2026-08-03 10:00:00".into()),
        }]);
        assert_eq!(reqs.force, 1, "channel force 统一为 1");
        assert_eq!(reqs.requests[0].user_id.as_str(), "u1");
    }

    #[test]
    fn tool_call_and_result_requests_force_is_one() {
        let call = tool_call_requests(vec![ToolCallRequest {
            agent_id: Arc::new("a1".into()),
            role_name: Arc::new("r1".into()),
            tool_name: Arc::new("get_date".into()),
            tool_params: Arc::new(serde_json::json!({})),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        }]);
        assert_eq!(call.force, 1);
        assert_eq!(call.requests[0].tool_name.as_str(), "get_date");

        let result = tool_result_requests(vec![ToolResultRequest {
            agent_id: Arc::new("a1".into()),
            role_name: Arc::new("r1".into()),
            tool_result: Arc::new(serde_json::json!({ "ok": true })),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        }]);
        assert_eq!(result.force, 1);
        assert_eq!(result.requests[0].tool_result["ok"], true);
    }

    #[tokio::test]
    async fn send_store_request_skips_empty_base_url() {
        let client = Client::new();
        // base_url 空 → 直接 Ok（不联网）
        let rst = send_store_request(&client, "", "k", "/store/think", &serde_json::json!({})).await;
        assert!(rst.is_ok(), "base_url 空应跳过发送");
    }
}
