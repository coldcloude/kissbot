use std::sync::Arc;
use std::time::Duration;

use arcstr::ArcStr;
use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileAppendWriterContext, FileObjectAppender, ErrorHandler};
use kissbot_api::memory::{
    ChannelRecord, ChannelRequest, ChannelRequests, QueryRequest, RecentQuery, RecordKey,
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

// ========== HTTP 发送段（写入与查询共用） ==========

/// 共享的 HTTP 配置（client / base_url / api_key）
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

// ========== 通道记录 sender（批量投递到 memory-store） ==========

/// 所有通道记录请求共用一个上下文，串行发往 memory-store
struct ChannelStoreWriter {
    context: Arc<Mutex<ChannelStoreContext>>,
}

impl ChannelStoreWriter {
    fn new(context: ChannelStoreContext) -> Self {
        Self { context: Arc::new(Mutex::new(context)) }
    }
}

#[async_trait]
impl FileAppendWriter<ArcStr, ChannelRequest, ChannelStoreContext> for ChannelStoreWriter {
    async fn get_lock(&self, _key: &ArcStr) -> Arc<Mutex<ChannelStoreContext>> {
        self.context.clone()
    }
    async fn remove_lock(&self, _key: &ArcStr) {}
}

// ========== MemoryStoreClient ==========

pub struct MemoryStoreClient {
    /// 共享 HTTP 配置（与各 context 同一 Arc；读路径 build_context_from_memory_store 也经它发请求）
    http_client: Arc<MemoryStoreHttpClient>,
    channel_appender: FileObjectAppender<ArcStr, ChannelRequest, ChannelStoreWriter, ChannelStoreContext, LoggingErrorHandler>,
}

/// query/channel 响应 data 类型：Vec<(RecordKey, Vec<(sn, ChannelRecord)>)>（组 → 记录列表）
pub type QueryChannelData = Vec<(RecordKey, Vec<(u32, ChannelRecord)>)>;

impl MemoryStoreClient {
    /// 指定共享 HTTP 配置构造（new() 内部走此路径；测试注入 mock store 地址用）
    fn with_config(base_url: ArcStr, api_key: ArcStr) -> Self {
        // 共享 HTTP 配置：client / base_url / api_key 一份
        let http_client = Arc::new(MemoryStoreHttpClient {
            client: Client::new(),
            base_url,
            api_key,
        });
        Self {
            http_client: http_client.clone(),
            channel_appender: FileObjectAppender::new(
                Arc::new(ChannelStoreWriter::new(ChannelStoreContext { http_client: http_client.clone() })),
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
        let promise = self.channel_appender.append(CHANNEL_KEY.clone(), vec![record]).await;
        let _ = promise.await;
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

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn channel_requests_force_is_one() {
        let reqs = channel_requests(vec![ChannelRequest {
            meta: None,
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

    #[tokio::test]
    async fn send_store_request_skips_empty_base_url() {
        let config = MemoryStoreHttpClient {
            client: Client::new(),
            base_url: ArcStr::new(),
            api_key: ArcStr::from("k"),
        };
        // base_url 空 → 直接 Ok（不联网）
        let rst = config.send_store_request("/store/channel", &serde_json::json!({})).await;
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
