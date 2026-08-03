use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileAppendWriterContext, FileObjectAppender, ErrorHandler};
use kissbot_api::memory::{
    ChannelRequest, ChannelRequests, ThinkRequest, ThinkRequests,
    ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests,
};
use kissbot_security::HEADER_API_KEY;
use kissbot_api::Content;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::error;

// 批处理参数
const RECORD_QUEUE_SIZE: usize = 100;
const RECORD_MAX_DELAY: Duration = Duration::from_secs(1);
const CHANNEL_KEY: &str = "0";
const THINK_KEY: &str = "think";
const TOOL_CALL_KEY: &str = "tool-call";
const TOOL_RESULT_KEY: &str = "tool-result";

/// 单个 channel 记录（与 ChannelRequest 同构，用于 push_channel_record 接口）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    /// 接收方身份（= channel 绑定的 user_id，即 agent 视角的 self）
    pub self_user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
    pub group_name: Arc<String>,
    pub content: Content,
    pub time: Arc<String>,
}

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

// ========== 记录类型（与 WriteTask 变体字段同构） ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecord {
    pub agent_id: Arc<String>,
    pub role_name: Option<Arc<String>>,   // None → 空串（与 memory-store 无 role 语义一致）
    pub content: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub agent_id: Arc<String>,
    pub role_name: Option<Arc<String>>,
    pub tool_name: Arc<String>,
    pub tool_params: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub agent_id: Arc<String>,
    pub role_name: Option<Arc<String>>,
    pub tool_result: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

pub struct MemoryStoreClient {
    channel_appender: FileObjectAppender<String, ChannelRecord, StoreSender<MemoryStoreContext>, MemoryStoreContext, LoggingErrorHandler>,
    think_appender: FileObjectAppender<String, ThinkRecord, StoreSender<ThinkStoreContext>, ThinkStoreContext, LoggingErrorHandler>,
    tool_call_appender: FileObjectAppender<String, ToolCallRecord, StoreSender<ToolCallStoreContext>, ToolCallStoreContext, LoggingErrorHandler>,
    tool_result_appender: FileObjectAppender<String, ToolResultRecord, StoreSender<ToolResultStoreContext>, ToolResultStoreContext, LoggingErrorHandler>,
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

    pub async fn push_channel_record(&self, record: ChannelRecord) {
        self.channel_appender.append(CHANNEL_KEY.to_string(), vec![record]).await;
    }

    /// 推送 think 记录（思考内容，方案 A 只存思考内容）
    /// 当前无调用方（coordinator 接入属 Task 2），先标 allow(dead_code) 与 push_tool_call/push_tool_result 保持一致
    #[allow(dead_code)]
    pub async fn push_think(&self, agent_id: String, role_name: Option<String>, content: String, time: String) {
        self.think_appender.append(THINK_KEY.to_string(), vec![ThinkRecord {
            agent_id: Arc::new(agent_id),
            role_name: role_name.map(Arc::new),
            content: Arc::new(content),
            key: Arc::new(String::new()),
            time: Arc::new(time),
        }]).await;
    }

    /// 推送 tool-call 记录（station 工具调用功能落地时消费）
    #[allow(dead_code)]
    pub async fn push_tool_call(&self, agent_id: String, role_name: Option<String>, tool_name: String, tool_params: serde_json::Value, time: String) {
        self.tool_call_appender.append(TOOL_CALL_KEY.to_string(), vec![ToolCallRecord {
            agent_id: Arc::new(agent_id),
            role_name: role_name.map(Arc::new),
            tool_name: Arc::new(tool_name),
            tool_params: Arc::new(tool_params),
            key: Arc::new(String::new()),
            time: Arc::new(time),
        }]).await;
    }

    /// 推送 tool-result 记录（station 工具调用功能落地时消费）
    #[allow(dead_code)]
    pub async fn push_tool_result(&self, agent_id: String, role_name: Option<String>, tool_result: serde_json::Value, time: String) {
        self.tool_result_appender.append(TOOL_RESULT_KEY.to_string(), vec![ToolResultRecord {
            agent_id: Arc::new(agent_id),
            role_name: role_name.map(Arc::new),
            tool_result: Arc::new(tool_result),
            key: Arc::new(String::new()),
            time: Arc::new(time),
        }]).await;
    }
}

struct MemoryStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn channel_request(r: ChannelRecord) -> ChannelRequest {
    ChannelRequest {
        agent_id: r.agent_id,
        role_name: r.role_name,
        messenger_id: r.messenger_id,
        user_id: r.user_id,
        self_user_id: r.self_user_id,
        group_id: r.group_id,
        is_self: r.is_self,
        messenger_name: r.messenger_name,
        user_name: r.user_name,
        group_name: r.group_name,
        content: r.content,
        time: r.time,
    }
}

fn channel_requests(records: Vec<ChannelRecord>) -> ChannelRequests {
    ChannelRequests { requests: records.into_iter().map(channel_request).collect(), force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ChannelRecord> for MemoryStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ChannelRecord>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/channel", &channel_requests(records)).await
    }
}

// ========== think / tool-call / tool-result 的 context 与 requests 构造 ==========

pub struct ThinkStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn think_request(r: ThinkRecord) -> ThinkRequest {
    ThinkRequest {
        agent_id: r.agent_id,
        role_name: r.role_name.unwrap_or_default(),  // None → 空串（与 memory-store 无 role 语义一致）
        content: r.content,
        key: r.key,
        time: r.time,
    }
}

fn think_requests(records: Vec<ThinkRecord>) -> ThinkRequests {
    ThinkRequests { requests: records.into_iter().map(think_request).collect(), force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ThinkRecord> for ThinkStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ThinkRecord>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/think", &think_requests(records)).await
    }
}

pub struct ToolCallStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn tool_call_request(r: ToolCallRecord) -> ToolCallRequest {
    ToolCallRequest {
        agent_id: r.agent_id,
        role_name: r.role_name.unwrap_or_default(),
        tool_name: r.tool_name,
        tool_params: r.tool_params,
        key: r.key,
        time: r.time,
    }
}

fn tool_call_requests(records: Vec<ToolCallRecord>) -> ToolCallRequests {
    ToolCallRequests { requests: records.into_iter().map(tool_call_request).collect(), force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ToolCallRecord> for ToolCallStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ToolCallRecord>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/tool-call", &tool_call_requests(records)).await
    }
}

pub struct ToolResultStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

fn tool_result_request(r: ToolResultRecord) -> ToolResultRequest {
    ToolResultRequest {
        agent_id: r.agent_id,
        role_name: r.role_name.unwrap_or_default(),
        tool_result: r.tool_result,
        key: r.key,
        time: r.time,
    }
}

fn tool_result_requests(records: Vec<ToolResultRecord>) -> ToolResultRequests {
    ToolResultRequests { requests: records.into_iter().map(tool_result_request).collect(), force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ToolResultRecord> for ToolResultStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ToolResultRecord>) -> std::result::Result<(), kai_file::Error> {
        send_store_request(&self.client, &self.base_url, &self.api_key, "/store/tool-result", &tool_result_requests(records)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_request_maps_fields_and_empty_role() {
        let r = ThinkRecord {
            agent_id: Arc::new("a1".into()),
            role_name: Some(Arc::new("r1".into())),
            content: Arc::new("思考".into()),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        };
        let req = think_request(r);
        assert_eq!(req.agent_id.as_str(), "a1");
        assert_eq!(req.role_name.as_str(), "r1");
        assert_eq!(req.content.as_str(), "思考");
        assert_eq!(req.time.as_str(), "2026-08-03 10:00:00");

        let r2 = ThinkRecord {
            agent_id: Arc::new("a1".into()),
            role_name: None,   // 无 role → 请求体空串
            content: Arc::new("思考2".into()),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:01".into()),
        };
        assert_eq!(think_request(r2).role_name.as_str(), "", "None role_name 应映射为空串");
    }

    #[test]
    fn think_requests_force_is_one() {
        let reqs = think_requests(vec![ThinkRecord {
            agent_id: Arc::new("a1".into()),
            role_name: None,
            content: Arc::new("思考".into()),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        }]);
        assert_eq!(reqs.force, 1, "force 统一为 1");
        assert_eq!(reqs.requests.len(), 1);
    }

    #[test]
    fn channel_requests_force_is_one() {
        let rec = ChannelRecord {
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
            content: Content::Text(Arc::new("你好".into())),
            time: Arc::new("2026-08-03 10:00:00".into()),
        };
        let reqs = channel_requests(vec![rec]);
        assert_eq!(reqs.force, 1, "channel force 统一为 1");
        assert_eq!(reqs.requests[0].user_id.as_str(), "u1");
    }

    #[test]
    fn tool_call_and_result_requests_force_is_one() {
        let call = tool_call_requests(vec![ToolCallRecord {
            agent_id: Arc::new("a1".into()),
            role_name: None,
            tool_name: Arc::new("get_date".into()),
            tool_params: Arc::new(serde_json::json!({})),
            key: Arc::new(String::new()),
            time: Arc::new("2026-08-03 10:00:00".into()),
        }]);
        assert_eq!(call.force, 1);
        assert_eq!(call.requests[0].tool_name.as_str(), "get_date");
        assert_eq!(call.requests[0].role_name.as_str(), "", "None role_name 应映射为空串");

        let result = tool_result_requests(vec![ToolResultRecord {
            agent_id: Arc::new("a1".into()),
            role_name: None,
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
