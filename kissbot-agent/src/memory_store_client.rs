use std::collections::BTreeMap;
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

use crate::message::{MessageContent, extract_content};
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

    use crate::message::pack_memory_messages;
    use kissbot_api::ContentBlock;
use kissbot_llm::Message;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::net::TcpListener;

    // 相对 now 的时间字符串（与 read_recent_for_context 内部同格式）
    fn time_ago(secs: i64) -> String {
        chrono::Local::now()
            .checked_sub_signed(chrono::Duration::seconds(secs))
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    // 构造完整 ChannelRecord 的 JSON（与真实 store 序列化一致，typed 反序列化需要全部字段；sn 需调用方指定，channel_data 会按序号覆盖）
    fn channel_record_json(time: &str, user_name: &str, content: serde_json::Value, is_self: bool, sn: u64) -> serde_json::Value {
        json!({
            "user_id": "",
            "self_user_id": "",
            "messenger_id": "",
            "group_id": "",
            "is_self": if is_self { 1 } else { 0 },
            "messenger_name": "",
            "user_name": user_name,
            "group_name": "",
            "content": content,
            "time": time,
            "sn": sn,
        })
    }

    // 单条 Text ChannelRecord 的 JSON（is_self=0）
    fn record_json(time: &str, user_name: &str, content: &str) -> serde_json::Value {
        channel_record_json(time, user_name, json!({ "msg_type": "Text", "data": content }), false, 0)
    }

    // 带 is_self 的 Text ChannelRecord JSON（is_self=1 表示 agent 自身消息）
    fn record_json_self(time: &str, user_name: &str, content: &str, is_self: bool) -> serde_json::Value {
        channel_record_json(time, user_name, json!({ "msg_type": "Text", "data": content }), is_self, 0)
    }

    // query/channel 响应 data：Vec<(RecordKey, Vec<(sn, ChannelRecord)>)>；按序号注入 sn 到每条记录（同文件内唯一）
    fn channel_data(records: Vec<serde_json::Value>) -> serde_json::Value {
        let entries: Vec<serde_json::Value> = records.iter().enumerate()
            .map(|(sn, r)| {
                let mut rec = r.clone();
                rec["sn"] = json!(sn as u64);
                json!([sn, rec])
            })
            .collect();
        // group key 为 RecordKey（agent_id/role_event/date），与真实 store 返回一致
        json!([[{ "agent_id": "agent", "role_event": { "role": "role" }, "date": "2026-08-05" }, entries]])
    }

    // 启动本地 mock memory-store：
    // /store/query/channel/recent —— 按 (time, sn) 排序取后 count 条（无时间过滤，与真实 store 一致）
    // /store/query/channel —— 按 [start_time, end_time] 过滤（无 limit，与真实 store 一致）
    async fn start_mock_store(data: serde_json::Value) -> String {
        // 展平所有组为 [(sn, record)]，模拟 store 内部记录流
        let all: Vec<(u64, serde_json::Value)> = data.as_array().unwrap().iter()
            .flat_map(|group| group[1].as_array().unwrap().iter()
                .map(|entry| (entry[0].as_u64().unwrap(), entry[1].clone())))
            .collect();
        let recent_all = all.clone();
        let range_all = all.clone();
        let app = Router::new()
            .route("/store/query/channel/recent", post(move |Json(req): Json<RecentQuery>| async move {
                let mut v = recent_all.clone();
                v.sort_by(|a, b| a.1["time"].as_str().cmp(&b.1["time"].as_str()).then(a.0.cmp(&b.0)));
                let tail: Vec<_> = v.into_iter().rev().take(req.count as usize).collect();
                let entries: Vec<_> = tail.into_iter().rev().map(|(sn, rec)| json!([sn, rec])).collect();
                Json(json!({ "success": true, "data": [[{ "agent_id": req.agent_id, "role_event": req.role_event, "date": "2026-08-05" }, entries]], "error": null }))
            }))
            .route("/store/query/channel", post(move |Json(req): Json<QueryRequest>| async move {
                let entries: Vec<_> = range_all.iter()
                    .filter(|(_, r)| {
                        let t = r["time"].as_str().unwrap();
                        t >= req.start_time.as_str() && t <= req.end_time.as_str()
                    })
                    .map(|(sn, rec)| json!([sn, rec.clone()]))
                    .collect();
                Json(json!({ "success": true, "data": [[{ "agent_id": req.agent_id, "role_event": req.role_event, "date": "2026-08-05" }, entries]], "error": null }))
            }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{}", addr)
    }

    // 测试构造：指定 memory-store 根地址（覆盖 new() 的 ApiConfig 读取，避免与 http_server 测试的全局配置冲突）
    fn client_at(url: &str) -> MemoryStoreClient {
        MemoryStoreClient::with_config(ArcStr::from(url),ArcStr::from("k"))
    }

    #[tokio::test]
    async fn read_recent_window_covers_ln_returns_all_window_records() {
        // cutoff ≤ ln（窗口覆盖最后 N 边界）：M = cutoff → Query2 = [cutoff, ln] 整段 → 并集 = 窗口内全部记录
        // 30 条全部落在窗口内（now-1800 ~ now-60），count=10 → 结果 = 全部 30 条（m0..m29），时间正序
        let records: Vec<serde_json::Value> = (0..30)
            .map(|i| record_json(&time_ago(1800 - i * 60), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        // cutoff=now-7200 早于 ln=now-600 → M = cutoff
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 7200, 10).await.unwrap();
        assert_eq!(out.len(), 30, "窗口覆盖 ln 时结果 = 窗口内全部记录");
        assert_eq!(out[0].content, vec![ArcStr::from("m0")]);
        assert_eq!(out[29].content, vec![ArcStr::from("m29")]);
    }

    #[tokio::test]
    async fn read_recent_sparse_extends_beyond_window() {
        // cutoff > ln（最后 N 条全在窗口内，ln 早于窗口起点）：M = min(cutoff, ln) = ln →
        // Query2 退化为单点 [ln, ln]（ln 同时间组），并集后仍为最后 N 条（跨更早时间）
        let records: Vec<serde_json::Value> = (0..15)
            .map(|i| record_json(&time_ago(600 - i * 20), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        // 窗口起点 now-60s 晚于全部记录 → M = ln
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 60, 10).await.unwrap();
        assert_eq!(out.len(), 10, "稀疏场景结果 = 最后 10 条（跨更早时间）");
        assert_eq!(out[0].content, vec![ArcStr::from("m5")], "起点 = 第 6 条（最后 10 条最旧一条）");
    }

    #[tokio::test]
    async fn read_recent_empty_and_less_than_n() {
        // 空数据 → 空结果
        let url = start_mock_store(channel_data(vec![])).await;
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 7200, 10).await.unwrap();
        assert!(out.is_empty(), "空数据返回空");

        // 不足 N 条取全部
        let records = vec![
            record_json(&time_ago(60), "u", "a"),
            record_json(&time_ago(120), "u", "b"),
        ];
        let url = start_mock_store(channel_data(records)).await;
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 7200, 10).await.unwrap();
        assert_eq!(out.len(), 2, "不足 N 条取全部");

        // count = 0：不参与并集，直接返回空（防 start_idx = len 空切片 panic）
        let url = start_mock_store(channel_data(vec![
            record_json(&time_ago(60), "u", "a"),
            record_json(&time_ago(120), "u", "b"),
        ])).await;
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 7200, 0).await.unwrap();
        assert!(out.is_empty(), "count=0 返回空");
    }

    #[tokio::test]
    async fn read_recent_includes_same_time_group_beyond_n() {
        // 同时间组横跨 N 边界：索引 4,5,6 同在 t1=now-200s，count=10 → Query1 = 索引 5..14，ln = t1；
        // cutoff=now-250 介于 t0 与 t1 之间 → M = cutoff → Query2 = [now-250, t1] 整段（含完整 t1 组）
        // → 并集 = 索引 4..14 共 11 条（m4 在 start_idx 之前但同 ln 时间，被取回）
        let t0 = time_ago(300);
        let t1 = time_ago(200);
        let t2 = time_ago(100);
        let mut records = Vec::new();
        for i in 0..4 { records.push(record_json(&t0, "u", &format!("m{}", i))); }
        for i in 4..7 { records.push(record_json(&t1, "u", &format!("m{}", i))); }
        for i in 7..15 { records.push(record_json(&t2, "u", &format!("m{}", i))); }
        let url = start_mock_store(channel_data(records)).await;
        // cutoff=now-250 介于 t0 与 t1 之间 → M = cutoff
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 250, 10).await.unwrap();
        assert_eq!(out.len(), 11, "ln 同时间组完整保留（含 start_idx 之前同时间记录）");
        // 同时间组内部顺序 = BTreeMap 键 (time, sn) 升序 → 只断言 m4 被取回，不断言其在组内的具体位置
        assert!(out.iter().any(|m| m.content == vec![ArcStr::from("m4")]), "start_idx 之前的同时间记录被并集取回");
    }

    #[tokio::test]
    async fn read_recent_ln_group_beyond_n_when_cutoff_after_ln() {
        // 锁定分支：cutoff > ln → M = ln → Query2 退化为单点 [ln, ln]，取回最后 N 边界之前同 ln 时间的记录（不拆散）
        // 13 条：m0,m1@t0；m2,m3,m4@t1；m5..m12@t2（t0 < t1 < t2），count=10 →
        // Query1 = m3..m12（最后 10 条），ln = t1；cutoff=now-150 > ln(t1=now-200) → M = ln →
        // Query2 = [t1, t1] → m2,m3,m4（m3,m4 已去重）→ 并集 = m2..m12 共 11 条
        let t0 = time_ago(300);
        let t1 = time_ago(200);
        let t2 = time_ago(100);
        let mut records = Vec::new();
        for i in 0..2 { records.push(record_json(&t0, "u", &format!("m{}", i))); }
        for i in 2..5 { records.push(record_json(&t1, "u", &format!("m{}", i))); }
        for i in 5..13 { records.push(record_json(&t2, "u", &format!("m{}", i))); }
        let url = start_mock_store(channel_data(records)).await;
        // cutoff=now-150 晚于 ln(t1=now-200) → M = ln → [ln, ln]
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 150, 10).await.unwrap();
        assert_eq!(out.len(), 11, "cutoff > ln 时 [ln, ln] 取回边界前同时间记录（m2）");
        // 同时间组内部顺序 = BTreeMap 键 (time, sn) 升序 → 只断言 m2 被取回
        assert!(out.iter().any(|m| m.content == vec![ArcStr::from("m2")]), "m2 在最后 N 边界之前、与 ln 同时间 → 被 [ln, ln] 取回");
    }

    #[tokio::test]
    async fn read_recent_keeps_non_text_as_empty_content() {
        // 非文本记录在解析阶段不跳过：以空 content Vec 保留在结果中（pack 时跳过）；Multi 取其中 Text 子项拼接
        let t = time_ago(60);
        // 非 Text 变体（ToolCall 等）content 为空 Vec；Multi 取其中 Text 子项拼接
        // 注意：旧测试数据用的 "Image" 不是真实 Content 变体，typed 反序列化会失败
        let data = json!([[{ "agent_id": "agent", "role_event": { "role": "role" }, "date": "2026-08-05" }, [
            [0, channel_record_json(&t, "u1", json!({ "msg_type": "Text", "data": "你好" }), false, 0)],
            [1, channel_record_json(&t, "u2", json!({ "msg_type": "Multi", "data": [
                { "msg_type": "Text", "data": "a" }, { "msg_type": "ToolCall", "data": "key1" }, { "msg_type": "Text", "data": "b" }
            ] }), false, 1)],
            [2, channel_record_json(&t, "u3", json!({ "msg_type": "ToolCall", "data": "key2" }), false, 2)],
        ]]]);
        let url = start_mock_store(data).await;
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 7200, 10).await.unwrap();
        assert_eq!(out.len(), 3, "非文本记录保留在结果中（空 content）");
        assert_eq!(out[0].content, vec![ArcStr::from("你好")]);
        assert_eq!(out[1].content, vec![ArcStr::from("a"), ArcStr::from("b")], "Multi 内容取 Text 子项拼接");
        assert!(out[2].content.is_empty(), "ToolCall 记录 content 为空 Vec");
        // pack 时跳过空 content 记录 → 2 条 User 合并为 1 条 + 末尾空 Assistant
        assert_eq!(pack_memory_messages(&out).len(), 2, "pack 跳过空 content 记录");
    }

    #[tokio::test]
    async fn read_recent_dedups_duplicate_records_by_time_sn_at_parse() {
        // store 响应含重复记录（同 time+sn）：解析阶段先去重（而非并集末尾），
        // 不足 N 条直接返回的路径也不泄漏重复；不同 sn 的同内容记录不被误合并
        let t = time_ago(60);
        let data = json!([[{ "agent_id": "agent", "role_event": { "role": "role" }, "date": "2026-08-05" }, [
            [0, channel_record_json(&time_ago(120), "u", json!({ "msg_type": "Text", "data": "a" }), false, 0)],
            [1, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 1)],
            [2, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 1)],  // 同 time+sn 重复 → 去重
            [3, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 2)],  // 不同 sn → 保留
        ]]]);
        let url = start_mock_store(data).await;
        // 不足 N 条（4 → count=10）：直接返回 msgs，重复记录也不得泄漏
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 7200, 10).await.unwrap();
        assert_eq!(out.len(), 3, "同 (time, sn) 重复只保留一条，不同 sn 的同内容记录保留");
        assert_eq!(out.iter().filter(|m| m.content == vec![ArcStr::from("hello")]).count(), 2, "同秒同内容但 sn 不同 → 两条都保留");
    }

    #[tokio::test]
    async fn read_recent_unions_both_queries_and_dedups() {
        // 两次查询并集去重：Query2 区间 [M, ln] 与 Query1 尾部在 ln 处重叠（m2 两查询都有）
        // 12 条 m0..m11 时间 now-1200+i*60，count=10 → Query1 = m2..m11，ln = m2 的时间 now-1080；
        // cutoff=now-1800 ≤ ln → M = cutoff → Query2 = [now-1800, now-1080] → m0,m1,m2 → 共用 seen 去重 → 12 条
        let records: Vec<serde_json::Value> = (0..12)
            .map(|i| record_json(&time_ago(1200 - i * 60), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        // cutoff=now-1800 ≤ ln → M = cutoff → Query2 补 m0,m1
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 1800, 10).await.unwrap();
        assert_eq!(out.len(), 12, "并集去重后 = 全部 12 条（m2 只保留一条）");
        assert_eq!(out[0].content, vec![ArcStr::from("m0")]);
        assert_eq!(out[11].content, vec![ArcStr::from("m11")]);
    }

    #[tokio::test]
    async fn read_recent_map_len_counts_non_text_records() {
        // 语义锁定：map.len() 含非文本记录（解析不跳过）→ 即取到的数量。
        // 12 条记录 m0..m11（时间 now-1200+i*60），其中 m5 为非文本（ToolCall，content 为空 Vec）；
        // count=10 → Query1 解析后 map=10（= count，不早退）→ Query2 补 m0,m1 → 12 条（m5 以空 content 保留）
        let records: Vec<serde_json::Value> = (0..12)
            .map(|i| {
                let t = time_ago(1200 - i * 60);
                if i == 5 {
                    channel_record_json(&t, "u", json!({ "msg_type": "ToolCall", "data": "k" }), false, i as u64)
                } else {
                    record_json(&t, "u", &format!("m{}", i))
                }
            })
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        // cutoff=now-1800 ≤ ln → M = cutoff → Query2 补 m0,m1
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 1800, 10).await.unwrap();
        assert_eq!(out.len(), 12, "map.len()=10=count 不早退，Query2 补 m0,m1 → 12 条（含空 content 的 m5）");
        assert!(out.iter().any(|m| m.content.is_empty()), "非文本 m5 以空 content 保留在结果中");
        assert_eq!(out[0].content, vec![ArcStr::from("m0")]);
        assert_eq!(out[11].content, vec![ArcStr::from("m11")]);
    }

    #[tokio::test]
    async fn read_recent_recursively_collects_nested_multi_text() {
        // 嵌套 Multi：递归收集全部 Text 段（任意深度），非文本子项跳过
        let t = time_ago(60);
        let data = json!([[{ "agent_id": "agent", "role_event": { "role": "role" }, "date": "2026-08-05" }, [
            [0, channel_record_json(&t, "u1", json!({ "msg_type": "Multi", "data": [
                { "msg_type": "Text", "data": "a" },
                { "msg_type": "Multi", "data": [
                    { "msg_type": "Text", "data": "b" },
                    { "msg_type": "ToolCall", "data": "k" },
                    { "msg_type": "Text", "data": "c" },
                ] },
                { "msg_type": "Text", "data": "d" },
            ] }), false, 0)],
        ]]]);
        let url = start_mock_store(data).await;
        let out = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 7200, 10).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, vec![ArcStr::from("a"), ArcStr::from("b"), ArcStr::from("c"), ArcStr::from("d")], "嵌套 Multi 递归收集全部 Text 段，非文本子项跳过");
    }

    #[tokio::test]
    async fn read_recent_extracts_is_self_and_packs_alternating() {
        // 全链路：mock store 返回 [self, self, user, user, self, user] → 解析提取 is_self → 打包交替 User/Assistant
        let records = vec![
            record_json_self(&time_ago(300), "agent", "a0", true),
            record_json_self(&time_ago(280), "agent", "a1", true),
            record_json_self(&time_ago(260), "u1", "m0", false),
            record_json_self(&time_ago(240), "u2", "m1", false),
            record_json_self(&time_ago(220), "agent", "a2", true),
            record_json_self(&time_ago(200), "u3", "m2", false),
        ];
        let url = start_mock_store(channel_data(records)).await;
        let msgs = client_at(&url).read_recent_for_context(ArcStr::from("agent"), Arc::new(RoleEvent::new(ArcStr::from("role"))), 7200, 100).await.unwrap();
        let out = pack_memory_messages(&msgs);
        // 期望：[User("u1: m0\nu2: m1"), Assistant("a2"), User("u3: m2"), Assistant("")]
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u1: m0\nu2: m1"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.as_str().unwrap() == "a2"));
        assert!(matches!(&out[2], Message::User { content } if content.as_str() == "u3: m2"));
        // 结尾补的空 Assistant：content 为空串（不是 Null——须在 wire 上显式给出 content 字段）
        assert!(matches!(&out[3], Message::Assistant { content, .. } if content.as_str() == Some("")));
    }
}
