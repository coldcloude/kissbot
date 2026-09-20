use std::sync::Arc;

use axum::Router;
use kissbot_mcp::{Implementation, McpHttpConfig, McpServer};
use kissbot_security::{AuthLayer, SimpleApiKeyValidator};
use tokio::net::TcpListener;
use tracing::info;

use crate::config_manager::ConfigManager;
use crate::station::Station;
use crate::types::{Error, Result};

/// station 对外 MCP 服务：单端点 `POST /mcp`，供其他 station 作为 sub 调用
pub struct StationHttpServer {
    api_key: Arc<String>,
}

impl StationHttpServer {
    pub fn new() -> Self {
        let api_key = kissbot_security::SecurityConfig::get().api_key.to_string();
        Self { api_key: Arc::new(api_key) }
    }

    /// 构建 Router（生产：使用全局 Station 单例）
    pub fn create_router(&self) -> Router {
        Self::router_with_station(Station::shared(), self.api_key.as_str())
    }

    /// 测试/注入用：使用指定 Station 与 api_key 构建 Router
    pub fn router_with_station(station: Arc<Station>, api_key: &str) -> Router {
        let server = McpServer::new(station, server_info());
        kissbot_mcp::router(server, McpHttpConfig::default())
            .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(Arc::new(api_key.to_string())))))
    }

    /// 启动 station MCP 服务（阻塞，在协程中运行）
    pub async fn start(&self) -> Result<()> {
        let host = ConfigManager::get().station_host().to_string();
        let port = ConfigManager::get().station_port();
        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| Error::IoError(e.to_string()))?;
        info!("station MCP 服务器启动: {} {}（端点 /mcp）", addr, kissbot_mcp::PROTOCOL_VERSION);
        axum::serve(listener, self.create_router())
            .await
            .map_err(|e| Error::IoError(e.to_string()))?;
        Ok(())
    }
}

fn server_info() -> Implementation {
    Implementation::new("kissbot-station", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use kissbot_mcp::PROTOCOL_VERSION;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::configs::{StationRepo, GroupConfig};
    use crate::station::TERR_TOOL_NOT_FOUND;
    use arc_swap::ArcSwap;

    fn test_station_repo(station_id: &str) -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = Arc::new(station_id.into());
        let map = Arc::make_mut(&mut repo.groups);
        map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(GroupConfig::default())));
        repo
    }

    fn test_station(repo: &StationRepo) -> Arc<Station> {
        Arc::new(Station::from_repo(repo, "test-key").unwrap())
    }

    fn meta() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}
        })
    }

    async fn send(
        app: Router,
        uri: &str,
        key: &str,
        method: &str,
        name: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .header("Mcp-Method", method);
        if let Some(name) = name {
            builder = builder.header("Mcp-Name", name);
        }
        if !key.is_empty() {
            builder = builder.header("X-Api-Key", key);
        }
        let req = builder.body(Body::from(body.to_string())).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn station_mcp_auth_required() {
        let repo = test_station_repo("station-a");
        let station = test_station(&repo);
        let app = StationHttpServer::router_with_station(station, "secret");
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } });

        let (status, _) = send(app.clone(), "/mcp", "", "tools/list", None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = send(app, "/mcp", "wrong", "tools/list", None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn station_tools_list_success_and_missing_headers() {
        let repo = test_station_repo("station-a");
        let station = test_station(&repo);
        let app = StationHttpServer::router_with_station(station, "secret");

        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "group": ["filesystem"], "_meta": meta() }
        });
        let (status, value) = send(app.clone(), "/mcp", "secret", "tools/list", None, body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["result"]["tools"][0]["name"], "read");
        assert_eq!(value["result"]["_meta"]["kissbot/group"][0], "filesystem");

        // 缺少 MCP-Protocol-Version 头 → 400（认证通过后由 MCP 传输层拒绝）
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("Mcp-Method", "tools/list")
            .header("X-Api-Key", "secret")
            .body(Body::from(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() }
            }).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// /mcp 请求体（tools/call 全量参数 + 可选祖先链）
    fn call_tool_body(name: &str, ancestors: &[&str]) -> Value {
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {
                "name": name,
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                    "io.modelcontextprotocol/clientCapabilities": {},
                    "kissbot/ancestors": ancestors,
                }
            }
        })
    }

    #[tokio::test]
    async fn station_call_tool_failure_is_result_error_cycle_is_rpc_error() {
        let repo = test_station_repo("station-a");
        let station = test_station(&repo);
        let app = StationHttpServer::router_with_station(station, "secret");

        // 调用不存在的工具 → 200，失败编码在 CallToolResult 里
        let (status, body) = send(
            app.clone(), "/mcp", "secret", "tools/call", Some("missing"), call_tool_body("missing", &[]),
        ).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["isError"], true, "工具调用失败不是 HTTP 失败：错误随结果返回");
        assert_eq!(body["result"]["structuredContent"]["code"], TERR_TOOL_NOT_FOUND);
        assert!(body["result"]["content"][0]["text"].as_str().unwrap().contains("工具不存在"));

        // 自环（ancestors 含自身）→ JSON-RPC 错误 STATION_CYCLE
        let (status, body) = send(
            app, "/mcp", "secret", "tools/call", Some("read"), call_tool_body("read", &["station-a"]),
        ).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["error"]["code"], kissbot_mcp::STATION_CYCLE);
        assert!(body.get("result").is_none());
    }

    #[tokio::test]
    async fn station_discover_reports_tools_capability() {
        let repo = test_station_repo("station-a");
        let station = test_station(&repo);
        let app = StationHttpServer::router_with_station(station, "secret");
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": { "_meta": meta() } });
        let (status, value) = send(app, "/mcp", "secret", "server/discover", None, body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["result"]["supportedVersions"][0], PROTOCOL_VERSION);
        assert_eq!(value["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "kissbot-station");
    }
}
