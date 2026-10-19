use std::sync::Arc;
use std::time::Duration;

use arcstr::ArcStr;
use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileAppendWriterContext, FileObjectAppender, ErrorHandler};
use kissbot_api::memory::{
    ChannelRecord, ChannelRequest, ChannelRequests, QueryRequest, RecentQuery, RecordKey,
    ThinkRequest, ThinkRequests, ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests,
};
use kissbot_api::{ApiConfig, ApiResponse, RoleEvent};
use kissbot_security::{HEADER_API_KEY, SecurityConfig};
use reqwest::Client;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::error;

use crate::types::{Error, Result};

// 批处理参数
const RECORD_QUEUE_SIZE: usize = 10;
const RECORD_MAX_DELAY: Duration = Duration::from_millis(100);
const CHANNEL_KEY: ArcStr = arcstr::literal!("1");
const THINK_KEY: ArcStr = arcstr::literal!("2");
const TOOL_CALL_KEY: ArcStr = arcstr::literal!("3");
const TOOL_RESULT_KEY: ArcStr = arcstr::literal!("4");

// ========== 共享发送段（各记录类型复用，只写一遍） ==========

/// 共享的 HTTP 配置（client / base_url / api_key，各类型 context 经 Arc 同源引用）
pub struct MemoryStoreHttpClient {
    client: Client,
    base_url: ArcStr,
    api_key: ArcStr,
}

impl MemoryStoreHttpClient {
    /// POST {base_url}{path}，带 X-Api-Key 鉴权头；base_url 空则跳过（Ok）；
    /// 非 2xx 返回 Err（错误含状态码与返回体）
    async fn send_store_request(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> std::result::Result<(), kai_file::Error> {
        if self.base_url.is_empty() {
            return Ok(());
        }
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let response = self.client.post(&url)
            .header(&*HEADER_API_KEY, self.api_key.as_str())
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

    /// 查询请求：POST {base_url}{path}，带 X-Api-Key 鉴权头，返回反序列化后的响应体；
    /// 非 2xx 返回 Err（错误含状态码与返回体）；base_url 为空时 URL 为相对路径 → reqwest 报错
    /// （读路径无 store 配置不可静默跳过，与写路径 Ok 跳过语义不同）
    async fn send_store_query<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> std::result::Result<T, kai_file::Error> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let response = self.client.post(&url)
            .header(&*HEADER_API_KEY, self.api_key.as_str())
            .json(body)
            .send()
            .await
            .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?;
        if !response.status().is_success() {
            let status = response.status();
            let msg = response.text().await.unwrap_or_default();
            return Err(kai_file::Error::WriteError(format!("[{}] {}", status, msg)));
        }
        response.json::<T>().await
            .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))
    }
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
    /// 共享 HTTP 配置（与各 context 同一 Arc；读路径 read_recent_for_context 也经它发请求）
    http_client: Arc<MemoryStoreHttpClient>,
    channel_appender: FileObjectAppender<ArcStr, ChannelRequest, StoreSender<ChannelStoreContext>, ChannelStoreContext, LoggingErrorHandler>,
    think_appender: FileObjectAppender<ArcStr, ThinkRequest, StoreSender<ThinkStoreContext>, ThinkStoreContext, LoggingErrorHandler>,
    tool_call_appender: FileObjectAppender<ArcStr, ToolCallRequest, StoreSender<ToolCallStoreContext>, ToolCallStoreContext, LoggingErrorHandler>,
    tool_result_appender: FileObjectAppender<ArcStr, ToolResultRequest, StoreSender<ToolResultStoreContext>, ToolResultStoreContext, LoggingErrorHandler>,
}

/// query/channel 响应 data 类型：Vec<(RecordKey, Vec<(sn, ChannelRecord)>)>（组 → 记录列表）
pub type QueryChannelData = Vec<(RecordKey, Vec<(u32, ChannelRecord)>)>;

impl MemoryStoreClient {
    /// 指定共享 HTTP 配置构造（new() 内部走此路径；测试注入 mock store 地址用）
    fn with_config(base_url: ArcStr, api_key: ArcStr) -> Self {
        // 共享 HTTP 配置：self 与各类型 context 经 Arc 同源引用（client / base_url / api_key 一份）
        let http_client = Arc::new(MemoryStoreHttpClient {
            client: Client::new(),
            base_url,
            api_key,
        });
        Self {
            http_client: http_client.clone(),
            channel_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ChannelStoreContext { http_client: http_client.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            think_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ThinkStoreContext { http_client: http_client.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_call_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolCallStoreContext { http_client: http_client.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_result_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolResultStoreContext { http_client: http_client.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
        }
    }

    pub fn new() -> Self {
        let api_config = ApiConfig::get();
        let security = SecurityConfig::get();
        Self::with_config(api_config.memory_store_url.clone(), security.api_key.clone())
    }

    pub async fn push_channel_record(&self, record: ChannelRequest) {
        self.channel_appender.append(CHANNEL_KEY.clone(), vec![record]).await;
    }

    /// 推送 think 记录（reasoning_content + thinking 双字段，key 关联 ChannelRecord(Think)）
    /// 调用方：AgentPipeline::run_once（思考记忆推送）
    pub async fn push_think(&self, record: ThinkRequest) {
        self.think_appender.append(THINK_KEY.clone(), vec![record]).await;
    }

    /// 推送 tool-call 记录（station 工具调用前写入记忆）
    pub async fn push_tool_call(&self, record: ToolCallRequest) {
        self.tool_call_appender.append(TOOL_CALL_KEY.clone(), vec![record]).await;
    }

    /// 推送 tool-result 记录（station 工具调用后写入记忆）
    pub async fn push_tool_result(&self, record: ToolResultRequest) {
        self.tool_result_appender.append(TOOL_RESULT_KEY.clone(), vec![record]).await;
    }

    pub async fn query_last(&self, agent_id: ArcStr, role_event: Arc<RoleEvent>, count: u32) -> Result<QueryChannelData> {
        let body = RecentQuery {
            agent_id,
            role_event,
            count,
        };
        let resp: ApiResponse<QueryChannelData> = self.http_client
        .send_store_query("/store/query/channel/recent", &body).await
        .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e).into()))?;
        resp.data.ok_or_else(|| Error::InternalError(resp.error.unwrap_or_default()))
    }

    pub async fn query_recent(&self, agent_id: ArcStr, role_event: Arc<RoleEvent>, start_time: ArcStr, end_time: ArcStr) -> Result<QueryChannelData> {
        let body = QueryRequest {
            agent_id,
            role_event,
            start_time,
            end_time,
        };
        let resp: ApiResponse<QueryChannelData> = self.http_client
        .send_store_query("/store/query/channel", &body).await
        .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e).into()))?;
        resp.data.ok_or_else(|| Error::InternalError(resp.error.unwrap_or_default()))
    }
}

struct ChannelStoreContext {
    http_client: Arc<MemoryStoreHttpClient>,
}

fn channel_requests(records: Vec<ChannelRequest>) -> ChannelRequests {
    ChannelRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<ArcStr, ChannelRequest> for ChannelStoreContext {
    async fn write(&mut self, _key: &ArcStr, records: Vec<ChannelRequest>) -> std::result::Result<(), kai_file::Error> {
        self.http_client.send_store_request("/store/channel", &channel_requests(records)).await
    }
}

// ========== think / tool-call / tool-result 的 context 与 requests 构造 ==========

pub struct ThinkStoreContext {
    http_client: Arc<MemoryStoreHttpClient>,
}

fn think_requests(records: Vec<ThinkRequest>) -> ThinkRequests {
    ThinkRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<ArcStr, ThinkRequest> for ThinkStoreContext {
    async fn write(&mut self, _key: &ArcStr, records: Vec<ThinkRequest>) -> std::result::Result<(), kai_file::Error> {
        self.http_client.send_store_request("/store/think", &think_requests(records)).await
    }
}

pub struct ToolCallStoreContext {
    http_client: Arc<MemoryStoreHttpClient>,
}

fn tool_call_requests(records: Vec<ToolCallRequest>) -> ToolCallRequests {
    ToolCallRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<ArcStr, ToolCallRequest> for ToolCallStoreContext {
    async fn write(&mut self, _key: &ArcStr, records: Vec<ToolCallRequest>) -> std::result::Result<(), kai_file::Error> {
        self.http_client.send_store_request("/store/tool-call", &tool_call_requests(records)).await
    }
}

pub struct ToolResultStoreContext {
    http_client: Arc<MemoryStoreHttpClient>,
}

fn tool_result_requests(records: Vec<ToolResultRequest>) -> ToolResultRequests {
    ToolResultRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<ArcStr, ToolResultRequest> for ToolResultStoreContext {
    async fn write(&mut self, _key: &ArcStr, records: Vec<ToolResultRequest>) -> std::result::Result<(), kai_file::Error> {
        self.http_client.send_store_request("/store/tool-result", &tool_result_requests(records)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use kissbot_api::ContentBlock;

    #[test]
    fn think_requests_force_is_one() {
        let reqs = think_requests(vec![ThinkRequest {
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            reasoning_content: Some(ArcStr::from("推理")),
            thinking: None,
            key: ArcStr::from("k1"),
            time: ArcStr::from("2026-08-04 10:00:00"),
        }]);
        assert_eq!(reqs.force, 1, "force 统一为 1");
        assert_eq!(reqs.requests[0].reasoning_content.as_deref().unwrap(), "推理");
    }

    #[test]
    fn channel_requests_force_is_one() {
        let reqs = channel_requests(vec![ChannelRequest {
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            messenger_id: ArcStr::from("web"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("u1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::from(""),
            user_name: ArcStr::from(""),
            group_name: ArcStr::from(""),
            content: kissbot_api::Content::Text(ArcStr::from("你好")),
            time: ArcStr::from("2026-08-03 10:00:00"),
        }]);
        assert_eq!(reqs.force, 1, "channel force 统一为 1");
        assert_eq!(reqs.requests[0].user_id.as_str(), "u1");
    }

    #[test]
    fn tool_call_and_result_requests_force_is_one() {
        let call = tool_call_requests(vec![ToolCallRequest {
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            tool_name: ArcStr::from("get_date"),
            tool_params: Arc::new(serde_json::json!({})),
            key: ArcStr::new(),
            time: ArcStr::from("2026-08-03 10:00:00"),
        }]);
        assert_eq!(call.force, 1);
        assert_eq!(call.requests[0].tool_name.as_str(), "get_date");

        let result = tool_result_requests(vec![ToolResultRequest {
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            tool_result: Some(Arc::new(vec![ContentBlock::text(ArcStr::from(r#"{"ok":true}"#))])),
            key: ArcStr::new(),
            time: ArcStr::from("2026-08-03 10:00:00"),
        }]);
        assert_eq!(result.force, 1);
        assert!(matches!(&result.requests[0].tool_result.as_ref().unwrap()[0], ContentBlock::Text { text, .. } if text.as_str() == r#"{"ok":true}"#));
    }

    #[tokio::test]
    async fn send_store_request_skips_empty_base_url() {
        let config = MemoryStoreHttpClient {
            client: Client::new(),
            base_url: ArcStr::new(),
            api_key: ArcStr::from("k"),
        };
        // base_url 空 → 直接 Ok（不联网）
        let rst = config.send_store_request("/store/think", &serde_json::json!({})).await;
        assert!(rst.is_ok(), "base_url 空应跳过发送");
    }

    #[tokio::test]
    async fn send_store_query_deserializes_response_and_errors_on_non_2xx() {
        use axum::routing::post;
        use axum::{Json, Router};
        use serde_json::json;
        use tokio::net::TcpListener;

        // 本地 mock：/ok 返回 JSON 体；/bad 返回 400
        let app = Router::new()
            .route("/ok", post(|| async { Json(json!({ "success": true, "data": { "k": 1 } })) }))
            .route("/bad", post(|| async { (axum::http::StatusCode::BAD_REQUEST, "boom") }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = MemoryStoreHttpClient {
            client: Client::new(),
            base_url: ArcStr::from(format!("http://{}", addr)),
            api_key: ArcStr::from("k"),
        };
        // 成功：响应体反序列化（含鉴权头由 mock 忽略）
        let v: serde_json::Value = config.send_store_query("/ok", &json!({})).await.unwrap();
        assert_eq!(v["data"]["k"], 1, "响应体应反序列化返回");
        // 非 2xx → Err（含状态码）
        let rst = config.send_store_query::<serde_json::Value>("/bad", &json!({})).await;
        assert!(rst.is_err(), "非 2xx 应报错");
    }

    #[tokio::test]
    async fn send_store_query_empty_base_url_errors() {
        // base_url 空 → 相对路径请求 → reqwest 报错（读路径无 store 配置不可静默跳过，与写路径 Ok 跳过语义不同）
        let config = MemoryStoreHttpClient {
            client: Client::new(),
            base_url: ArcStr::new(),
            api_key: ArcStr::from("k"),
        };
        let rst = config.send_store_query::<serde_json::Value>("/x", &serde_json::json!({})).await;
        assert!(rst.is_err(), "base_url 空应报错");
    }
}
