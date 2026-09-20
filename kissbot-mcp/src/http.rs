//! MCP Streamable HTTP 传输（最小实现）：单端点 POST + JSON 响应
//!
//! 只实现规范中请求-响应所需的传输要求：单一 MCP 端点、每请求一个 POST、
//! `MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` 请求头校验、`Origin` 校验。
//! 不支持 SSE 流式响应与 GET 端点。

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bytes::Bytes;
use serde_json::Value;

use crate::protocol::jsonrpc::{JsonRpcRequest, JsonRpcResponse, RequestId, METHOD_NOT_FOUND};
use crate::protocol::meta::{META_CLIENT_CAPABILITIES, META_PROTOCOL_VERSION};
use crate::protocol::{
    McpError, HEADER_MISMATCH, INVALID_PARAMS, METHOD_DISCOVER, METHOD_TOOLS_CALL,
    METHOD_TOOLS_LIST,
};
use crate::server::{McpHandler, McpServer};

const HEADER_PROTOCOL_VERSION: &str = "mcp-protocol-version";
const HEADER_METHOD: &str = "mcp-method";
const HEADER_NAME: &str = "mcp-name";
const HEADER_ORIGIN: &str = "origin";

/// MCP HTTP 端点配置
#[derive(Debug, Clone)]
pub struct McpHttpConfig {
    /// MCP 端点路径（默认 `/mcp`）
    pub endpoint_path: Arc<String>,
    /// 允许的 Origin 白名单；空列表时仅放行本机回环来源
    pub allowed_origins: Vec<String>,
}

impl Default for McpHttpConfig {
    fn default() -> Self {
        Self {
            endpoint_path: Arc::new("/mcp".to_string()),
            allowed_origins: Vec::new(),
        }
    }
}

impl McpHttpConfig {
    pub fn with_endpoint(endpoint_path: impl Into<String>) -> Self {
        Self {
            endpoint_path: Arc::new(endpoint_path.into()),
            ..Default::default()
        }
    }
}

struct McpHttpState<H: McpHandler> {
    server: McpServer<H>,
    config: McpHttpConfig,
}

/// 构建 MCP HTTP Router（不含认证；认证由调用方以 layer 叠加）
pub fn router<H: McpHandler>(server: McpServer<H>, config: McpHttpConfig) -> Router {
    let path = config.endpoint_path.to_string();
    Router::new()
        .route(&path, post(handle::<H>))
        .with_state(Arc::new(McpHttpState { server, config }))
}

async fn handle<H: McpHandler>(
    State(state): State<Arc<McpHttpState<H>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1. Origin 校验（防 DNS rebinding；无 Origin 的 CLI 请求放行）
    if let Some(origin) = header_str(&headers, HEADER_ORIGIN) {
        if !origin_allowed(origin, &state.config.allowed_origins) {
            return error_response(
                StatusCode::FORBIDDEN,
                None,
                McpError::invalid_request(format!("Origin 不被允许: {}", origin)),
            );
        }
    }
    // 2. 解析 JSON 请求体
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                None,
                McpError::parse_error(format!("请求体不是合法 JSON: {}", e)),
            )
        }
    };
    let id: Option<RequestId> = value
        .get("id")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            id,
            McpError::invalid_request("缺少 method 字段"),
        );
    };
    // 3. 协议版本头与请求体 `_meta` 必须同时存在且一致
    let Some(header_version) = header_str(&headers, HEADER_PROTOCOL_VERSION) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            id,
            McpError::new(INVALID_PARAMS, "缺少 MCP-Protocol-Version 请求头"),
        );
    };
    let meta = value.get("params").and_then(|p| p.get("_meta"));
    let Some(body_version) = meta
        .and_then(|m| m.get(META_PROTOCOL_VERSION))
        .and_then(Value::as_str)
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            id,
            McpError::new(
                INVALID_PARAMS,
                format!("params._meta 缺少 {}", META_PROTOCOL_VERSION),
            ),
        );
    };
    if meta
        .and_then(|m| m.get(META_CLIENT_CAPABILITIES))
        .is_none()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            id,
            McpError::new(
                INVALID_PARAMS,
                format!("params._meta 缺少 {}", META_CLIENT_CAPABILITIES),
            ),
        );
    }
    if header_version != body_version {
        return error_response(
            StatusCode::BAD_REQUEST,
            id,
            McpError::new(
                HEADER_MISMATCH,
                format!(
                    "MCP-Protocol-Version 头({}) 与请求体协议版本({}) 不一致",
                    header_version, body_version
                ),
            ),
        );
    }
    if !state
        .server
        .supported_versions()
        .iter()
        .any(|v| v == body_version)
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            id,
            McpError::unsupported_protocol_version(
                body_version,
                state.server.supported_versions(),
            ),
        );
    }
    // 4. 方法头必填且与请求体一致
    let Some(header_method) = header_str(&headers, HEADER_METHOD) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            id,
            McpError::new(INVALID_PARAMS, "缺少 Mcp-Method 请求头"),
        );
    };
    if header_method != method {
        return error_response(
            StatusCode::BAD_REQUEST,
            id,
            McpError::new(
                HEADER_MISMATCH,
                format!("Mcp-Method 头({}) 与请求 method({}) 不一致", header_method, method),
            ),
        );
    }
    // 5. tools/call 的 Mcp-Name 头必填且与 params.name 一致
    if method == METHOD_TOOLS_CALL {
        let Some(name) = value
            .get("params")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
        else {
            return error_response(
                StatusCode::BAD_REQUEST,
                id,
                McpError::new(INVALID_PARAMS, "tools/call 缺少 params.name"),
            );
        };
        match header_str(&headers, HEADER_NAME) {
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    id,
                    McpError::new(INVALID_PARAMS, "tools/call 缺少 Mcp-Name 请求头"),
                )
            }
            Some(header_name) if header_name != name => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    id,
                    McpError::new(
                        HEADER_MISMATCH,
                        format!("Mcp-Name 头({}) 与 params.name({}) 不一致", header_name, name),
                    ),
                )
            }
            _ => {}
        }
    }
    // 6. 解析为 JSON-RPC 请求并分发
    let request: JsonRpcRequest = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                id,
                McpError::invalid_request(format!("JSON-RPC 请求非法: {}", e)),
            )
        }
    };
    let response = state.server.handle(request).await;
    let status = match &response.error {
        Some(e) if e.code == METHOD_NOT_FOUND => StatusCode::NOT_FOUND,
        _ => StatusCode::OK,
    };
    (status, Json(response)).into_response()
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn error_response(status: StatusCode, id: Option<RequestId>, error: McpError) -> Response {
    (status, Json(JsonRpcResponse::error(id, error))).into_response()
}

/// Origin 校验：命中白名单或本机回环来源放行
fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    if allowed.iter().any(|a| a == origin) {
        return true;
    }
    let lower = origin.to_ascii_lowercase();
    ["http://127.0.0.1", "https://127.0.0.1", "http://localhost", "https://localhost", "http://[::1]", "https://[::1]"]
        .iter()
        .any(|prefix| lower == *prefix || lower.starts_with(&format!("{}:", prefix)))
}

/// MCP 端点支持的方法名集合（供文档与测试断言）
pub const SUPPORTED_METHODS: [&str; 3] = [METHOD_DISCOVER, METHOD_TOOLS_LIST, METHOD_TOOLS_CALL];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::meta::{Implementation, ResponseMeta};
    use crate::protocol::{
        CallToolResult, DiscoverResult, ResultType, Tool, ToolsCallParams, ToolsListParams,
        ToolsListResult, PROTOCOL_VERSION,
    };
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    struct TestHandler;

    #[async_trait]
    impl McpHandler for TestHandler {
        async fn discover(&self) -> DiscoverResult {
            DiscoverResult::tools_only(Implementation::new("test", "1"), None)
        }

        async fn list_tools(&self, params: ToolsListParams) -> Result<ToolsListResult, McpError> {
            let mut tool = Tool::new("read", "读", json!({ "type": "object" }));
            if let Some(group) = params.meta.groups.as_ref().and_then(|g| g.first()) {
                tool = tool.with_group(group.clone());
            }
            Ok(ToolsListResult::new(vec![Arc::new(tool)]))
        }

        async fn call_tool(&self, params: ToolsCallParams) -> Result<CallToolResult, McpError> {
            Ok(CallToolResult::text(params.arguments.to_string()))
        }
    }

    fn app() -> Router {
        router(
            McpServer::new(Arc::new(TestHandler), Implementation::new("test", "1")),
            McpHttpConfig::default(),
        )
    }

    fn meta() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}
        })
    }

    async fn send(app: Router, headers: Vec<(&str, &str)>, body: Value) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json");
        for (k, v) in headers {
            builder = builder.header(k, v);
        }
        let req = builder.body(Body::from(body.to_string())).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn std_headers(method: &'static str) -> Vec<(&'static str, &'static str)> {
        vec![
            ("MCP-Protocol-Version", PROTOCOL_VERSION),
            ("Mcp-Method", method),
        ]
    }

    #[tokio::test]
    async fn tools_list_happy_path() {
        let app = app();
        let (status, body) = send(
            app,
            std_headers("tools/list"),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                     "params": { "_meta": {
                         "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                         "io.modelcontextprotocol/clientCapabilities": {},
                         "kissbot/groups": ["fs"]
                     } } }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["tools"][0]["name"], "read");
        assert_eq!(body["result"]["tools"][0]["_meta"]["kissbot/group"], "fs");
    }

    #[tokio::test]
    async fn missing_protocol_version_header_is_400() {
        let app = app();
        let (status, body) = send(
            app,
            vec![("Mcp-Method", "tools/list")],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn protocol_version_mismatch_is_header_mismatch() {
        let app = app();
        let (status, body) = send(
            app,
            vec![("MCP-Protocol-Version", "1999-01-01"), ("Mcp-Method", "tools/list")],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], HEADER_MISMATCH);
    }

    #[tokio::test]
    async fn unknown_method_is_404() {
        let app = app();
        let (status, body) = send(
            app,
            std_headers("tools/nope"),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/nope", "params": { "_meta": meta() } }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn tools_call_requires_matching_mcp_name() {
        let app = app();
        let (status, body) = send(
            app,
            vec![
                ("MCP-Protocol-Version", PROTOCOL_VERSION),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "other"),
            ],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "read", "arguments": {}, "_meta": meta() } }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], HEADER_MISMATCH);
    }

    #[tokio::test]
    async fn remote_origin_is_forbidden() {
        let app = app();
        let (status, body) = send(
            app,
            vec![
                ("Origin", "http://evil.example"),
                ("MCP-Protocol-Version", PROTOCOL_VERSION),
                ("Mcp-Method", "tools/list"),
            ],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body["error"]["message"].as_str().unwrap().contains("Origin"));
    }

    #[tokio::test]
    async fn loopback_origin_is_allowed() {
        let app = app();
        let (status, _) = send(
            app,
            vec![
                ("Origin", "http://127.0.0.1:3080"),
                ("MCP-Protocol-Version", PROTOCOL_VERSION),
                ("Mcp-Method", "tools/list"),
            ],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn malformed_json_is_parse_error_without_id() {
        let app = app();
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(Body::from("{not json"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], crate::protocol::PARSE_ERROR);
        assert!(body.get("id").is_none());
    }

    #[test]
    fn result_type_default_is_complete() {
        assert_eq!(ResultType::default(), ResultType::Complete);
        let _ = ResponseMeta::default();
    }
}
