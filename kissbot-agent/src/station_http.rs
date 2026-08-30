use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use kissbot_api::ApiResponse;
use kissbot_security::{AuthLayer, SimpleApiKeyValidator};
use serde_json::Value;
use tokio::net::TcpListener;
use tracing::info;

use crate::config_manager::ConfigManager;
use crate::station::Station;
use crate::types::{
    Error, Result, StationCallToolRequest, StationListMcpsRequest, StationListToolsRequest,
};

/// station 对外 HTTP 服务：供其他 station 作为 sub 调用
pub struct StationHttpServer {
    api_key: String,
}

#[derive(Clone)]
struct AppState {
    station: &'static Station,
}

impl StationHttpServer {
    pub fn new() -> Self {
        let api_key = kissbot_security::SecurityConfig::get().api_key.to_string();
        Self { api_key }
    }

    /// 构建 Router（生产：使用全局 Station 单例）
    pub fn create_router(&self) -> Router {
        Self::router_with_station(Station::get(), &self.api_key)
    }

    /// 测试/注入用：使用指定 Station 与 api_key 构建 Router
    pub fn router_with_station(station: &'static Station, api_key: &str) -> Router {
        Router::new()
            .route("/station/tools", post(list_tools))
            .route("/station/mcps", post(list_mcps))
            .route("/station/call-tool", post(call_tool))
            .with_state(AppState { station })
            .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(
                Arc::new(api_key.to_string()),
            ))))
    }

    /// 启动 station HTTP 服务（阻塞，在协程中运行）
    pub async fn start(&self) -> Result<()> {
        let host = ConfigManager::get().station_host().to_string();
        let port = ConfigManager::get().station_port();
        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| Error::IoError(e.to_string()))?;
        info!("station HTTP 服务器启动: {}", addr);
        axum::serve(listener, self.create_router())
            .await
            .map_err(|e| Error::IoError(e.to_string()))?;
        Ok(())
    }
}

fn filter_set(filter: &Option<Vec<String>>) -> Option<HashSet<String>> {
    filter.as_ref().map(|v| v.iter().cloned().collect())
}

fn cycle_response<T>() -> (StatusCode, Json<ApiResponse<T>>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::error("station cycle detected".to_string())),
    )
}

async fn list_tools(
    State(state): State<AppState>,
    Json(req): Json<StationListToolsRequest>,
) -> impl IntoResponse {
    let filter = filter_set(&req.filter);
    match state.station.tools(filter.as_ref(), &req.ancestors).await {
        Ok(tools) => (StatusCode::OK, Json(ApiResponse::success(tools))),
        Err(Error::StationCycle(_)) => cycle_response::<Vec<crate::config_manager::ToolConfig>>(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<crate::config_manager::ToolConfig>>::error(e.to_string())),
        ),
    }
}

async fn list_mcps(
    State(state): State<AppState>,
    Json(req): Json<StationListMcpsRequest>,
) -> impl IntoResponse {
    let filter = filter_set(&req.filter);
    match state.station.mcps(filter.as_ref(), &req.ancestors).await {
        Ok(mcps) => (StatusCode::OK, Json(ApiResponse::success(mcps))),
        Err(Error::StationCycle(_)) => cycle_response::<Vec<crate::config_manager::McpConfig>>(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<crate::config_manager::McpConfig>>::error(
                e.to_string(),
            )),
        ),
    }
}

async fn call_tool(
    State(state): State<AppState>,
    Json(req): Json<StationCallToolRequest>,
) -> impl IntoResponse {
    match state
        .station
        .call_tool(&req.tool_name, req.parameters, &req.ancestors)
        .await
    {
        Ok(result) => (StatusCode::OK, Json(ApiResponse::success(result))),
        // 自环不是工具调用失败，按协议返回非 200
        Err(Error::StationCycle(_)) => cycle_response::<Value>(),
        // 远端连接/协议错误不是工具调用失败，按非 200 返回
        Err(Error::StationConnectionError(e)) => (
            StatusCode::BAD_GATEWAY,
            Json(ApiResponse::<Value>::error(e)),
        ),
        // 工具调用失败统一 HTTP 200 + error
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<Value>::error(e.to_string())),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::config_manager::{StationRepo, ToolkitConfig};
    use arc_swap::ArcSwap;

    fn test_station_repo(station_id: &str) -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = Arc::new(station_id.into());
        let map = Arc::make_mut(&mut repo.toolkits);
        map.insert(
            "filesystem".to_string(),
            ArcSwap::new(Arc::new(ToolkitConfig::default())),
        );
        repo
    }

    fn leak_station(repo: &StationRepo) -> &'static Station {
        Box::leak(Box::new(Station::from_repo(repo, "test-key").unwrap()))
    }

    async fn send(
        app: Router,
        method: &str,
        uri: &str,
        key: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if !key.is_empty() {
            builder = builder.header("X-Api-Key", key);
        }
        let req = if let Some(b) = body {
            builder
                .header("content-type", "application/json")
                .body(Body::from(b.to_string()))
                .unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn station_api_auth_required() {
        let repo = test_station_repo("station-a");
        let station = leak_station(&repo);
        let app = StationHttpServer::router_with_station(station, "secret");

        let (status, body) = send(
            app.clone(),
            "POST",
            "/station/tools",
            "",
            Some(serde_json::json!({ "ancestors": [] })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["success"], false);

        let (status, _) = send(
            app,
            "POST",
            "/station/tools",
            "wrong",
            Some(serde_json::json!({ "ancestors": [] })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn station_list_tools_success_and_cycle() {
        let repo = test_station_repo("station-a");
        let station = leak_station(&repo);
        let app = StationHttpServer::router_with_station(station, "secret");

        let (status, body) = send(
            app.clone(),
            "POST",
            "/station/tools",
            "secret",
            Some(serde_json::json!({
                "filter": ["filesystem"], "ancestors": []
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(body["data"][0]["name"], "read");

        let (status, body) = send(
            app,
            "POST",
            "/station/tools",
            "secret",
            Some(serde_json::json!({
                "ancestors": ["parent", "station-a"]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["success"], false);
        assert!(body["error"].as_str().unwrap().contains("cycle"));
    }

    #[tokio::test]
    async fn station_call_tool_failure_is_200_cycle_non_200() {
        let repo = test_station_repo("station-a");
        let station = leak_station(&repo);
        let app = StationHttpServer::router_with_station(station, "secret");

        // 本地只注册了 filesystem/read；调用不存在的工具属于工具调用失败 → 200 + success=false
        let (status, body) = send(
            app.clone(),
            "POST",
            "/station/call-tool",
            "secret",
            Some(serde_json::json!({
                "tool_name": "missing", "parameters": {}, "ancestors": []
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false);
        assert!(body["error"].as_str().unwrap().contains("工具不存在"));

        // 自环 → 非 200
        let (status, body) = send(
            app,
            "POST",
            "/station/call-tool",
            "secret",
            Some(serde_json::json!({
                "tool_name": "read", "parameters": {}, "ancestors": ["station-a"]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["success"], false);
    }
}
