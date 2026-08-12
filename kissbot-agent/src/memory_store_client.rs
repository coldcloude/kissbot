use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileAppendWriterContext, FileObjectAppender, ErrorHandler};
use kissbot_api::memory::{
    ChannelRecord, ChannelRequest, ChannelRequests, QueryRequest, RecentQuery, RecordKey,
    ThinkRequest, ThinkRequests, ToolCallRequest, ToolCallRequests, ToolResultRequest, ToolResultRequests,
};
use kissbot_api::ApiResponse;
use kissbot_security::HEADER_API_KEY;
use reqwest::Client;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::error;

use crate::config_manager::EffectiveContextConfig;
use crate::message::{MessageContent, extract_content};
use crate::types::{Error, Result};

// 批处理参数
const RECORD_QUEUE_SIZE: usize = 10;
const RECORD_MAX_DELAY: Duration = Duration::from_millis(100);
const CHANNEL_KEY: &str = "1";
const THINK_KEY: &str = "2";
const TOOL_CALL_KEY: &str = "3";
const TOOL_RESULT_KEY: &str = "4";

// ========== 共享发送段（各记录类型复用，只写一遍） ==========

/// 共享的 HTTP 配置（client / base_url / api_key，各类型 context 经 Arc 同源引用）
pub struct StoreHttpConfig {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

impl StoreHttpConfig {
    /// 构造共享 HTTP 配置（client / base_url / api_key 取自全局配置）
    fn new() -> Self {
        let api_config = kissbot_api::ApiConfig::get();
        let security = kissbot_security::SecurityConfig::get();
        Self {
            client: Client::new(),
            base_url: api_config.memory_store_url.clone(),
            api_key: security.api_key.clone(),
        }
    }

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
            .header(HEADER_API_KEY, self.api_key.as_str())
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
            .header(HEADER_API_KEY, self.api_key.as_str())
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
    config: Arc<StoreHttpConfig>,
    channel_appender: FileObjectAppender<String, ChannelRequest, StoreSender<ChannelStoreContext>, ChannelStoreContext, LoggingErrorHandler>,
    think_appender: FileObjectAppender<String, ThinkRequest, StoreSender<ThinkStoreContext>, ThinkStoreContext, LoggingErrorHandler>,
    tool_call_appender: FileObjectAppender<String, ToolCallRequest, StoreSender<ToolCallStoreContext>, ToolCallStoreContext, LoggingErrorHandler>,
    tool_result_appender: FileObjectAppender<String, ToolResultRequest, StoreSender<ToolResultStoreContext>, ToolResultStoreContext, LoggingErrorHandler>,
}

impl MemoryStoreClient {
    /// 指定共享 HTTP 配置构造（new() 内部走此路径；测试注入 mock store 地址用）
    fn with_config(config: Arc<StoreHttpConfig>) -> Self {
        // 共享 HTTP 配置：self 与各类型 context 经 Arc 同源引用（client / base_url / api_key 一份）
        Self {
            config: config.clone(),
            channel_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ChannelStoreContext { config: config.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            think_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ThinkStoreContext { config: config.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_call_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolCallStoreContext { config: config.clone() })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
            tool_result_appender: FileObjectAppender::new(
                Arc::new(StoreSender::new(ToolResultStoreContext { config })),
                Arc::new(LoggingErrorHandler),
                RECORD_MAX_DELAY,
                RECORD_QUEUE_SIZE,
            ),
        }
    }

    pub fn new() -> Self {
        // 共享 HTTP 配置：各类型 context 经 Arc 同源引用（client / base_url / api_key 一份）
        Self::with_config(Arc::new(StoreHttpConfig::new()))
    }

    pub async fn push_channel_record(&self, record: ChannelRequest) {
        self.channel_appender.append(CHANNEL_KEY.to_string(), vec![record]).await;
    }

    /// 推送 think 记录（reasoning_content + thinking 双字段，key 关联 ChannelRecord(Think)）
    /// 调用方：coordinator 步骤 4（思考记忆推送）
    pub async fn push_think(&self, record: ThinkRequest) {
        self.think_appender.append(THINK_KEY.to_string(), vec![record]).await;
    }

    /// 推送 tool-call 记录（station 工具调用后写入记忆）
    pub async fn push_tool_call(&self, record: ToolCallRequest) {
        self.tool_call_appender.append(TOOL_CALL_KEY.to_string(), vec![record]).await;
    }

    /// 推送 tool-result 记录（station 工具调用后写入记忆）
    pub async fn push_tool_result(&self, record: ToolResultRequest) {
        self.tool_result_appender.append(TOOL_RESULT_KEY.to_string(), vec![record]).await;
    }

    /// role 模式记忆打包：两次查询并集——① RecentQuery 最近 N 条（无时间参数）→ ln = 最旧一条 time；
    /// ② QueryRequest 时间范围 [M, ln]（M = min(时间窗起点, ln)，无 limit，M == ln 时退化为单点取 ln 同时间组）；
    /// 结果 = ① ∪ ②（两次解析共用同一 BTreeMap，键 (time, sn) → 去重 + 天然时间正序），升序返回
    /// （原 MemoryReader.read_recent_for_context 迁入；HTTP 经共享 config.send_store_query 发）
    pub async fn read_recent_for_context(
        &self,
        agent_id: Arc<String>,
        role_name: Arc<String>,
        cfg: &EffectiveContextConfig,
    ) -> Result<Vec<MessageContent>> {
        // 时间窗起点计算失败（checked_sub_signed 溢出）→ 直接报错，不静默回退
        let start = chrono::Local::now()
            .checked_sub_signed(chrono::Duration::seconds(cfg.memory_time_secs as i64))
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .ok_or_else(|| Error::MemoryTimeWindow("计算记忆时间窗起点失败".to_string()))?;

        let count = cfg.memory_count;

        // ===== Query1：POST {store}/store/query/channel/recent（RecentQuery，无时间参数） =====
        let body = RecentQuery {
            agent_id: agent_id.clone(),
            role_name: role_name.clone(),
            count: count as u32,
        };
        // 响应反序列化为类型化 ApiResponse<QueryChannelData>（tuple 由 serde 解析，无需手拼索引）
        let resp: ApiResponse<QueryChannelData> = self.config
            .send_store_query("/store/query/channel/recent", &body).await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        let groups = resp.data.unwrap_or_default();

        // ===== 解析：BTreeMap 以 (time, sn) 为键同时做去重与排序（迭代天然升序） =====
        // 两次查询共用同一 map → 并集自动去重（Query2 的 [M, ln] 区间与 Query1 尾部在 ln 处重叠）
        let mut map: BTreeMap<(String, u64), MessageContent> = BTreeMap::new();
        parse_channel_groups(groups, &mut map);

        // ln = 已解析记录最旧一条的 time（BTreeMap 首键）；map 为空说明无记录（count=0 时 Query1 必为空）
        // 一并在此处理；Query1 最旧若为非文本，其同时间组除非文本外无其他记录（否则文本记录会占据更小首键）
        let Some((ln, _)) = map.keys().next() else {
            return Ok(Vec::new());
        };
        let ln = ln.clone();
        // 不足 N 条：直接返回（无需 Query2）。理论上去重后记录不重复，map 大小即取到的数量；
        // 注意：以解析后文本数（map.len()）判定——非文本记录在最后 N 内被跳过时也会提前返回
        if map.len() < count {
            return Ok(map.into_values().collect());
        }
        // M = min(时间窗起点 start, ln)
        let m = if start.as_str() < ln.as_str() { start } else { ln.clone() };  // M = min(cutoff, ln)

        // ===== Query2：POST {store}/store/query/channel（QueryRequest 时间范围 [M, ln]，无 limit） =====
        // M == ln 时退化为单点 [ln, ln]（取 ln 同时间组）；与 Query1 并集（共用 map 去重）
        let body = QueryRequest {
            agent_id,
            role_name,
            start_time: Arc::new(m),
            end_time: Arc::new(ln),
        };
        let resp: ApiResponse<QueryChannelData> = self.config
            .send_store_query("/store/query/channel", &body).await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        parse_channel_groups(resp.data.unwrap_or_default(), &mut map);
        Ok(map.into_values().collect())
    }
}

/// query/channel 响应 data 类型：Vec<(RecordKey, Vec<(sn, ChannelRecord)>)>（组 → 记录列表）
type QueryChannelData = Vec<(RecordKey, Vec<(u32, ChannelRecord)>)>;

/// 解析 query 响应分组 → 写入 BTreeMap：(time, sn) 为唯一键（sn 为同文件内唯一序号），
/// 同键重复记录只保留第一条（两查询共用 map → 并集自动去重，迭代天然按 (time, sn) 升序）
/// 文本提取走 extract_content（递归收集全部 Text 段）
fn parse_channel_groups(groups: QueryChannelData, map: &mut BTreeMap<(String, u64), MessageContent>) {
    for (_, records) in groups {
        for (_, rec) in records {
            let time = rec.time.as_str().to_string();  // 键用（MessageContent 值不再保留 time）
            let sn = rec.sn;
            let key = (time, sn);
            // 同 (time, sn) 已存在（两查询并集在 ln 处重叠）→ 提前跳过，免去 user_name clone 与文本提取
            if map.contains_key(&key) {
                continue;
            }
            map.insert(key, extract_content(rec.user_name, &rec.content, rec.is_self));
        }
    }
}

struct ChannelStoreContext {
    config: Arc<StoreHttpConfig>,
}

fn channel_requests(records: Vec<ChannelRequest>) -> ChannelRequests {
    ChannelRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ChannelRequest> for ChannelStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ChannelRequest>) -> std::result::Result<(), kai_file::Error> {
        self.config.send_store_request("/store/channel", &channel_requests(records)).await
    }
}

// ========== think / tool-call / tool-result 的 context 与 requests 构造 ==========

pub struct ThinkStoreContext {
    config: Arc<StoreHttpConfig>,
}

fn think_requests(records: Vec<ThinkRequest>) -> ThinkRequests {
    ThinkRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ThinkRequest> for ThinkStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ThinkRequest>) -> std::result::Result<(), kai_file::Error> {
        self.config.send_store_request("/store/think", &think_requests(records)).await
    }
}

pub struct ToolCallStoreContext {
    config: Arc<StoreHttpConfig>,
}

fn tool_call_requests(records: Vec<ToolCallRequest>) -> ToolCallRequests {
    ToolCallRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ToolCallRequest> for ToolCallStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ToolCallRequest>) -> std::result::Result<(), kai_file::Error> {
        self.config.send_store_request("/store/tool-call", &tool_call_requests(records)).await
    }
}

pub struct ToolResultStoreContext {
    config: Arc<StoreHttpConfig>,
}

fn tool_result_requests(records: Vec<ToolResultRequest>) -> ToolResultRequests {
    ToolResultRequests { requests: records, force: 1 }
}

#[async_trait]
impl FileAppendWriterContext<String, ToolResultRequest> for ToolResultStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ToolResultRequest>) -> std::result::Result<(), kai_file::Error> {
        self.config.send_store_request("/store/tool-result", &tool_result_requests(records)).await
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
        let config = StoreHttpConfig {
            client: Client::new(),
            base_url: String::new(),
            api_key: Arc::new("k".into()),
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
        let config = StoreHttpConfig {
            client: Client::new(),
            base_url: format!("http://{}", addr),
            api_key: Arc::new("k".into()),
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
        let config = StoreHttpConfig {
            client: Client::new(),
            base_url: String::new(),
            api_key: Arc::new("k".into()),
        };
        let rst = config.send_store_query::<serde_json::Value>("/x", &serde_json::json!({})).await;
        assert!(rst.is_err(), "base_url 空应报错");
    }

    use crate::message::pack_memory_messages;
    use crate::types::Message;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::net::TcpListener;

    // 测试用上下文配置（非记忆字段用默认值占位）
    fn ctx_config(memory_time_secs: u64, memory_count: usize) -> EffectiveContextConfig {
        EffectiveContextConfig {
            channel_batch_interval_secs: 0,
            memory_time_secs,
            memory_count,
            compress_prompt: String::new(),
            stations: Default::default(),
        }
    }

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
        // group key 为 RecordKey（agent_id/role_name/date），与真实 store 返回一致
        json!([[{ "agent_id": "agent", "role_name": "role", "date": "2026-08-05" }, entries]])
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
                Json(json!({ "success": true, "data": [[{ "agent_id": req.agent_id, "role_name": req.role_name, "date": "2026-08-05" }, entries]], "error": null }))
            }))
            .route("/store/query/channel", post(move |Json(req): Json<QueryRequest>| async move {
                let entries: Vec<_> = range_all.iter()
                    .filter(|(_, r)| {
                        let t = r["time"].as_str().unwrap();
                        t >= req.start_time.as_str() && t <= req.end_time.as_str()
                    })
                    .map(|(sn, rec)| json!([sn, rec.clone()]))
                    .collect();
                Json(json!({ "success": true, "data": [[{ "agent_id": req.agent_id, "role_name": req.role_name, "date": "2026-08-05" }, entries]], "error": null }))
            }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{}", addr)
    }

    // 测试构造：指定 memory-store 根地址（覆盖 new() 的 ApiConfig 读取，避免与 http_server 测试的全局配置冲突）
    fn client_at(url: &str) -> MemoryStoreClient {
        MemoryStoreClient::with_config(Arc::new(StoreHttpConfig {
            client: Client::new(),
            base_url: url.to_string(),
            api_key: Arc::new("k".into()),
        }))
    }

    #[tokio::test]
    async fn read_recent_window_covers_ln_returns_all_window_records() {
        // cutoff ≤ ln（窗口覆盖最后 N 边界）：M = cutoff → Query2 = [cutoff, ln] 整段 → 并集 = 窗口内全部记录
        // 30 条全部落在窗口内（now-1800 ~ now-60），count=10 → 结果 = 全部 30 条（m0..m29），时间正序
        let records: Vec<serde_json::Value> = (0..30)
            .map(|i| record_json(&time_ago(1800 - i * 60), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(7200, 10);  // cutoff=now-7200 早于 ln=now-600 → M = cutoff
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &cfg).await.unwrap();
        assert_eq!(out.len(), 30, "窗口覆盖 ln 时结果 = 窗口内全部记录");
        assert_eq!(out[0].content, vec![Arc::new("m0".to_string())]);
        assert_eq!(out[29].content, vec![Arc::new("m29".to_string())]);
    }

    #[tokio::test]
    async fn read_recent_sparse_extends_beyond_window() {
        // cutoff > ln（最后 N 条全在窗口内，ln 早于窗口起点）：M = min(cutoff, ln) = ln →
        // Query2 退化为单点 [ln, ln]（ln 同时间组），并集后仍为最后 N 条（跨更早时间）
        let records: Vec<serde_json::Value> = (0..15)
            .map(|i| record_json(&time_ago(600 - i * 20), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(60, 10);  // 窗口起点 now-60s 晚于全部记录 → M = ln
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &cfg).await.unwrap();
        assert_eq!(out.len(), 10, "稀疏场景结果 = 最后 10 条（跨更早时间）");
        assert_eq!(out[0].content, vec![Arc::new("m5".to_string())], "起点 = 第 6 条（最后 10 条最旧一条）");
    }

    #[tokio::test]
    async fn read_recent_empty_and_less_than_n() {
        // 空数据 → 空结果
        let url = start_mock_store(channel_data(vec![])).await;
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &ctx_config(7200, 10)).await.unwrap();
        assert!(out.is_empty(), "空数据返回空");

        // 不足 N 条取全部
        let records = vec![
            record_json(&time_ago(60), "u", "a"),
            record_json(&time_ago(120), "u", "b"),
        ];
        let url = start_mock_store(channel_data(records)).await;
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &ctx_config(7200, 10)).await.unwrap();
        assert_eq!(out.len(), 2, "不足 N 条取全部");

        // count = 0：不参与并集，直接返回空（防 start_idx = len 空切片 panic）
        let url = start_mock_store(channel_data(vec![
            record_json(&time_ago(60), "u", "a"),
            record_json(&time_ago(120), "u", "b"),
        ])).await;
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &ctx_config(7200, 0)).await.unwrap();
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
        let cfg = ctx_config(250, 10);  // cutoff=now-250 介于 t0 与 t1 之间 → M = cutoff
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &cfg).await.unwrap();
        assert_eq!(out.len(), 11, "ln 同时间组完整保留（含 start_idx 之前同时间记录）");
        // 同时间组内部顺序 = BTreeMap 键 (time, sn) 升序 → 只断言 m4 被取回，不断言其在组内的具体位置
        assert!(out.iter().any(|m| m.content == vec![Arc::new("m4".to_string())]), "start_idx 之前的同时间记录被并集取回");
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
        let cfg = ctx_config(150, 10);  // cutoff=now-150 晚于 ln(t1=now-200) → M = ln → [ln, ln]
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &cfg).await.unwrap();
        assert_eq!(out.len(), 11, "cutoff > ln 时 [ln, ln] 取回边界前同时间记录（m2）");
        // 同时间组内部顺序 = BTreeMap 键 (time, sn) 升序 → 只断言 m2 被取回
        assert!(out.iter().any(|m| m.content == vec![Arc::new("m2".to_string())]), "m2 在最后 N 边界之前、与 ln 同时间 → 被 [ln, ln] 取回");
    }

    #[tokio::test]
    async fn read_recent_keeps_non_text_as_empty_content() {
        // 非文本记录不再在解析阶段跳过：以空 content Vec 保留在结果中（pack 时跳过）；Multi 取其中 Text 子项拼接
        let t = time_ago(60);
        // 非 Text 变体（ToolCall 等）content 为空 Vec；Multi 取其中 Text 子项拼接
        // 注意：旧测试数据用的 "Image" 不是真实 Content 变体，typed 反序列化会失败
        let data = json!([[{ "agent_id": "agent", "role_name": "role", "date": "2026-08-05" }, [
            [0, channel_record_json(&t, "u1", json!({ "msg_type": "Text", "data": "你好" }), false, 0)],
            [1, channel_record_json(&t, "u2", json!({ "msg_type": "Multi", "data": [
                { "msg_type": "Text", "data": "a" }, { "msg_type": "ToolCall", "data": "key1" }, { "msg_type": "Text", "data": "b" }
            ] }), false, 1)],
            [2, channel_record_json(&t, "u3", json!({ "msg_type": "ToolCall", "data": "key2" }), false, 2)],
        ]]]);
        let url = start_mock_store(data).await;
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &ctx_config(7200, 10)).await.unwrap();
        assert_eq!(out.len(), 3, "非文本记录保留在结果中（空 content）");
        assert_eq!(out[0].content, vec![Arc::new("你好".to_string())]);
        assert_eq!(out[1].content, vec![Arc::new("a".to_string()), Arc::new("b".to_string())], "Multi 内容取 Text 子项拼接");
        assert!(out[2].content.is_empty(), "ToolCall 记录 content 为空 Vec");
        // pack 时跳过空 content 记录 → 2 条 User 合并为 1 条 + 末尾空 Assistant
        assert_eq!(pack_memory_messages(&out).len(), 2, "pack 跳过空 content 记录");
    }

    #[tokio::test]
    async fn read_recent_dedups_duplicate_records_by_time_sn_at_parse() {
        // store 响应含重复记录（同 time+sn）：解析阶段先去重（而非并集末尾），
        // 不足 N 条直接返回的路径也不泄漏重复；不同 sn 的同内容记录不被误合并
        let t = time_ago(60);
        let data = json!([[{ "agent_id": "agent", "role_name": "role", "date": "2026-08-05" }, [
            [0, channel_record_json(&time_ago(120), "u", json!({ "msg_type": "Text", "data": "a" }), false, 0)],
            [1, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 1)],
            [2, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 1)],  // 同 time+sn 重复 → 去重
            [3, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 2)],  // 不同 sn → 保留
        ]]]);
        let url = start_mock_store(data).await;
        // 不足 N 条（4 → count=10）：直接返回 msgs，重复记录也不得泄漏
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &ctx_config(7200, 10)).await.unwrap();
        assert_eq!(out.len(), 3, "同 (time, sn) 重复只保留一条，不同 sn 的同内容记录保留");
        assert_eq!(out.iter().filter(|m| m.content == vec![Arc::new("hello".to_string())]).count(), 2, "同秒同内容但 sn 不同 → 两条都保留");
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
        let cfg = ctx_config(1800, 10);
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &cfg).await.unwrap();
        assert_eq!(out.len(), 12, "并集去重后 = 全部 12 条（m2 只保留一条）");
        assert_eq!(out[0].content, vec![Arc::new("m0".to_string())]);
        assert_eq!(out[11].content, vec![Arc::new("m11".to_string())]);
    }

    #[tokio::test]
    async fn read_recent_map_len_counts_non_text_records() {
        // 语义锁定：map.len() 含非文本记录（解析不再跳过）→ 即取到的数量。
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
        let cfg = ctx_config(1800, 10);  // cutoff=now-1800 ≤ ln → M = cutoff → Query2 补 m0,m1
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &cfg).await.unwrap();
        assert_eq!(out.len(), 12, "map.len()=10=count 不早退，Query2 补 m0,m1 → 12 条（含空 content 的 m5）");
        assert!(out.iter().any(|m| m.content.is_empty()), "非文本 m5 以空 content 保留在结果中");
        assert_eq!(out[0].content, vec![Arc::new("m0".to_string())]);
        assert_eq!(out[11].content, vec![Arc::new("m11".to_string())]);
    }

    #[tokio::test]
    async fn read_recent_recursively_collects_nested_multi_text() {
        // 嵌套 Multi：递归收集全部 Text 段（任意深度），非文本子项跳过
        let t = time_ago(60);
        let data = json!([[{ "agent_id": "agent", "role_name": "role", "date": "2026-08-05" }, [
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
        let out = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &ctx_config(7200, 10)).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, vec![Arc::new("a".to_string()), Arc::new("b".to_string()), Arc::new("c".to_string()), Arc::new("d".to_string())], "嵌套 Multi 递归收集全部 Text 段，非文本子项跳过");
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
        let msgs = client_at(&url).read_recent_for_context(Arc::new("agent".to_string()), Arc::new("role".to_string()), &ctx_config(7200, 100)).await.unwrap();
        let out = pack_memory_messages(&msgs);
        // 期望：[User("u1: m0\nu2: m1"), Assistant("a2"), User("u3: m2"), Assistant("")]
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u1: m0\nu2: m1"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.as_str() == "a2"));
        assert!(matches!(&out[2], Message::User { content } if content.as_str() == "u3: m2"));
        assert!(matches!(&out[3], Message::Assistant { content, .. } if content.is_empty()));
    }
}
