use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use tracing::info;

use crate::config_manager::{ChannelConfig, ChannelUser, ConfigManager, ProviderConfig, ProviderModel};
use crate::types::Result;

/// 管理 REST API 服务器（axum，X-Api-Key 鉴权，security.admin_api_key）
pub struct HttpServer {
    #[allow(dead_code)]
    config: Arc<ConfigManager>,
    admin_api_key: String,
    host: String,
    port: u16,
}

// ========== 请求 DTO ==========

#[derive(Deserialize)]
struct NameRequest {
    name: String,
}

#[derive(Deserialize)]
struct ChannelIdRequest {
    channel_id: String,
}

#[derive(Deserialize)]
struct AddAdminRequest {
    channel_id: String,
    messenger_id: String,
    user_id: String,
}

#[derive(Deserialize)]
struct RemoveAdminRequest {
    channel_id: String,
    messenger_id: String,
    user_id: String,
}

impl HttpServer {
    /// 生产构造：admin_api_key 从 kissbot_security 全局配置读取
    pub fn new(config: Arc<ConfigManager>, host: String, port: u16) -> Self {
        let admin_api_key = kissbot_security::SecurityConfig::get().admin_api_key.to_string();
        Self::with_admin_key(config, admin_api_key, host, port)
    }

    /// 测试/注入构造：显式传入 admin_api_key，避免触发 kissbot-config 全局单例
    pub fn with_admin_key(config: Arc<ConfigManager>, admin_api_key: String, host: String, port: u16) -> Self {
        Self { config, admin_api_key, host, port }
    }

    fn build_router(&self) -> Router {
        let config = self.config.clone();
        let key = self.admin_api_key.clone();
        Router::new()
            .route("/config", get(get_config))
            .route("/config/providers", post(add_provider))
            .route("/config/providers/remove", post(remove_provider))
            .route("/config/default", post(set_default))
            .route("/config/channels", post(add_channel))
            .route("/config/channels/remove", post(remove_channel))
            .route("/config/admins", post(add_admin))
            .route("/config/admins/remove", post(remove_admin))
            .with_state(AppState { config, key })
    }

    /// 启动 HTTP 服务器（阻塞，在协程中运行）
    pub async fn start(&self) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| crate::types::Error::IoError(e.to_string()))?;
        info!("管理 API 服务器启动: {}", addr);
        axum::serve(listener, self.build_router()).await
            .map_err(|e| crate::types::Error::IoError(e.to_string()))?;
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    config: Arc<ConfigManager>,
    key: String,
}

/// 鉴权：X-Api-Key 与 admin_api_key 比对
fn check_api_key(headers: &HeaderMap, expected: &str) -> bool {
    headers.get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|k| k == expected)
        .unwrap_or(false)
}

fn unauthorized() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::UNAUTHORIZED, Json(json!({ "success": false, "error": "unauthorized" })))
}

fn ok<T: serde::Serialize>(data: T) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "success": true, "data": data })))
}

fn fail(e: crate::types::Error) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "success": false, "error": e.to_string() })))
}

// ========== Handlers ==========

async fn get_config(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    let snap = state.config.nexus_snapshot().await;
    ok(snap)
}

async fn add_provider(State(state): State<AppState>, headers: HeaderMap, Json(cfg): Json<ProviderConfig>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.add_provider(cfg).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn remove_provider(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<NameRequest>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.remove_provider(&req.name).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn set_default(State(state): State<AppState>, headers: HeaderMap, Json(pm): Json<ProviderModel>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.set_default_model(pm).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn add_channel(State(state): State<AppState>, headers: HeaderMap, Json(ch): Json<ChannelConfig>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.add_channel(ch).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn remove_channel(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<ChannelIdRequest>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.remove_channel(&req.channel_id).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn add_admin(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<AddAdminRequest>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    let admin = ChannelUser {
        messenger_id: Arc::new(req.messenger_id),
        user_id: Arc::new(req.user_id),
    };
    match state.config.add_admin(&req.channel_id, &admin).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn remove_admin(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<RemoveAdminRequest>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.remove_admin(&req.channel_id, &req.messenger_id, &req.user_id).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use kissbot_api::ArcSwapHashMap;
    use tempfile::tempdir;
    use tower::util::ServiceExt;

    fn test_provider(name: &str) -> crate::config_manager::ProviderConfig {
        crate::config_manager::ProviderConfig {
            name: Arc::new(name.into()),
            provider_type: "openai".into(),
            base_url: "https://api.example.com".into(),
            api_key: "sk-test".into(),
            default_context_length: 65536,
            default_max_tokens: 4096,
            default_temperature: 0.7,
            default_timeout_secs: 60,
            default_retry_count: 3,
            models: Arc::new(ArcSwapHashMap::new()),
        }
    }

    // ConfigManager 字段私有，无法在模块外直接构造；
    // 通过 ConfigManager::new() 加载临时 KISSBOT_CONFIG 构造（data_dir 指向 tempdir）。
    // 注意：kissbot-config 是进程级 OnceLock 单例，本测试是 kissbot-agent 唯一触发它的测试，
    // 因此设置环境变量后首次调用即完成初始化，不会与其他测试冲突。
    async fn test_manager(dir: &tempfile::TempDir) -> Arc<ConfigManager> {
        let data_dir = dir.path().join("data");
        let cfg_path = dir.path().join("config.json");
        let cfg_json = format!(
            r#"{{"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9090,"ws_reconnect_interval_secs":5,"init_agent_id":"","init_role":"","init_model":{{"provider":"deepseek","model":"gpt-4o"}}}}}}"#,
            data_dir.to_str().unwrap()
        );
        std::fs::write(&cfg_path, cfg_json).unwrap();
        // 2024 edition：设置环境变量需要 unsafe
        unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };
        Arc::new(ConfigManager::new().await.unwrap())
    }

    async fn send(app: axum::Router, method: &str, uri: &str, key: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if !key.is_empty() {
            builder = builder.header("x-api-key", key);
        }
        let req = if let Some(b) = body {
            builder.header("content-type", "application/json")
                .body(Body::from(b.to_string())).unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({})))
    }

    #[tokio::test]
    async fn config_endpoints_auth_and_crud() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir).await;
        let server = HttpServer::with_admin_key(manager.clone(), "admin-key-123".into(), "127.0.0.1".into(), 0);
        let app = server.build_router();

        // 无 key → 401
        let (status, _) = send(app.clone(), "GET", "/config", "", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // 错误 key → 401
        let (status, _) = send(app.clone(), "GET", "/config", "wrong", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // GET /config 初始快照
        let (status, body) = send(app.clone(), "GET", "/config", "admin-key-123", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"]["providers"].is_object());
        assert!(body["data"]["default_model"].is_object());

        // POST /config/providers 添加
        let (status, body) = send(app.clone(), "POST", "/config/providers", "admin-key-123",
            Some(serde_json::to_value(test_provider("deepseek")).unwrap())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        // 重名 → 失败
        let (status, body) = send(app.clone(), "POST", "/config/providers", "admin-key-123",
            Some(serde_json::to_value(test_provider("deepseek")).unwrap())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false);

        // POST /config/default
        let (status, body) = send(app.clone(), "POST", "/config/default", "admin-key-123",
            Some(serde_json::json!({ "provider": "deepseek", "model": "deepseek-4-flash" }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(manager.default_model().await.model, "deepseek-4-flash");

        // POST /config/providers/remove
        let (status, body) = send(app.clone(), "POST", "/config/providers/remove", "admin-key-123",
            Some(serde_json::json!({ "name": "deepseek" }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);

        // POST /config/channels 添加 + admins
        let (status, body) = send(app.clone(), "POST", "/config/channels", "admin-key-123",
            Some(serde_json::json!({
                "channel_id": "web-main", "ws_url": "ws://127.0.0.1:8201",
                "admins": [], "default_bind_user": null, "enabled_by_default": true
            }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        let (status, body) = send(app.clone(), "POST", "/config/admins", "admin-key-123",
            Some(serde_json::json!({ "channel_id": "web-main", "messenger_id": "web", "user_id": "u2" }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        // 落盘验证（ConfigManager data_dir = <tempdir>/data）
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("data/nexus.json")).unwrap()).unwrap();
        assert!(saved["providers"].is_object());
        assert_eq!(saved["default_model"]["model"], "deepseek-4-flash");
    }
}
