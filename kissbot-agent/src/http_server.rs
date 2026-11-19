use std::sync::Arc;

use arcstr::ArcStr;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use kissbot_security::{AuthLayer, SimpleApiKeyValidator};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use tracing::info;

use crate::config_manager::ConfigManager;
use crate::configs::{ChannelConfig, InputChannel, InputChannelsConfig, PipelineConfig};
use crate::nexus::Nexus;
use crate::pipelines::is_supported_pipeline_name;
use crate::station::Station;
use kissbot_api::{ChannelUser, RoleKey};
use kissbot_llm::ProviderModelConfig;
use crate::types::{Error, Result};

/// 管理 REST API 服务器（axum，X-Api-Key 鉴权 layer，security.admin_api_key）
pub struct HttpServer {
    admin_api_key: ArcStr,
}

// ========== 请求 DTO ==========

#[derive(Deserialize)]
struct ProviderRequest {
    name: ArcStr,
    config: ProviderModelConfig,
}

#[derive(Deserialize)]
struct NameRequest {
    name: ArcStr,
}

#[derive(Deserialize)]
struct ChannelIdRequest {
    channel_id: ArcStr,
}

#[derive(Deserialize)]
struct AddAdminRequest {
    channel_id: ArcStr,
    messenger_id: ArcStr,
    user_id: ArcStr,
}

#[derive(Deserialize)]
struct RemoveAdminRequest {
    channel_id: ArcStr,
    messenger_id: ArcStr,
    user_id: ArcStr,
}

#[derive(Deserialize)]
struct PipelineRequest {
    agent_id: ArcStr,
    role_name: ArcStr,
    pipeline_name: ArcStr,
}

#[derive(Deserialize)]
struct PipelineKeyRequest {
    agent_id: ArcStr,
    role_name: ArcStr,
}

#[derive(Deserialize)]
struct InputChannelRequest {
    agent_id: ArcStr,
    role_name: ArcStr,
    channel_id: ArcStr,
    messenger_id: ArcStr,
    user_id: ArcStr,
}

#[derive(Deserialize)]
struct StationGroupRequest {
    group: ArcStr,
}

#[derive(Deserialize)]
struct SubStationRequest {
    station_id: ArcStr,
}

impl PipelineRequest {
    fn role_key(&self) -> Arc<RoleKey> {
        role_key(&self.agent_id, &self.role_name)
    }
}

impl PipelineKeyRequest {
    fn role_key(&self) -> Arc<RoleKey> {
        role_key(&self.agent_id, &self.role_name)
    }
}

impl InputChannelRequest {
    fn role_key(&self) -> Arc<RoleKey> {
        role_key(&self.agent_id, &self.role_name)
    }

    fn input_channel(&self) -> InputChannel {
        InputChannel {
            channel_id: self.channel_id.clone(),
            messenger_id: self.messenger_id.clone(),
            user_id: self.user_id.clone(),
        }
    }
}

fn role_key(agent_id: &ArcStr, role_name: &ArcStr) -> Arc<RoleKey> {
    Arc::new(RoleKey {
        agent_id: agent_id.clone(),
        role_name: role_name.clone(),
    })
}

impl HttpServer {
    /// 从 kissbot_security 全局配置读取 admin_api_key（与 channel-web / memory-ego 一致）；
    /// 监听地址取 ConfigManager 单例的 mgmt_host / mgmt_port
    pub fn new() -> Self {
        let admin_api_key = kissbot_security::SecurityConfig::get().admin_api_key.clone();
        Self { admin_api_key }
    }

    /// 构建 Router：认证统一走 AuthLayer（kissbot-security，与 channel-web / memory-ego 同款）
    fn build_router(&self) -> Router {
        let config = ConfigManager::get();
        let admin_api_key = self.admin_api_key.clone();
        Router::new()
            .route("/config", get(get_config))
            .route("/config/providers", post(add_provider))
            .route("/config/providers/remove", post(remove_provider))
            .route("/config/channels", post(add_channel))
            .route("/config/channels/remove", post(remove_channel))
            .route("/config/channels/enable", post(enable_channel))
            .route("/config/channels/disable", post(disable_channel))
            .route("/config/admins", post(add_admin))
            .route("/config/admins/remove", post(remove_admin))
            .route("/config/pipelines", post(add_pipeline))
            .route("/config/pipelines/remove", post(remove_pipeline))
            .route("/config/pipelines/reload", post(reload_pipeline))
            .route("/config/pipelines/change", post(change_pipeline))
            .route("/config/pipelines/input-channels", post(add_input_channel))
            .route("/config/pipelines/input-channels/remove", post(remove_input_channel))
            .route("/config/station/groups", post(add_station_group))
            .route("/config/station/groups/remove", post(remove_station_group))
            .route("/config/station/sub-stations/enable", post(enable_sub_station))
            .route("/config/station/sub-stations/disable", post(disable_sub_station))
            .with_state(AppState { config })
            .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(admin_api_key))))
    }

    /// 启动 HTTP 服务器（阻塞，在协程中运行）
    pub async fn start(&self) -> Result<()> {
        let addr = format!("{}:{}", ConfigManager::get().mgmt_host(), ConfigManager::get().mgmt_port());
        let listener = TcpListener::bind(&addr).await?;
        info!("管理 API 服务器启动: {}", addr);
        axum::serve(listener, self.build_router()).await?;
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    config: &'static ConfigManager,
}

fn ok<T: serde::Serialize>(data: T) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "success": true, "data": data })))
}

fn fail(e: crate::types::Error) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "success": false, "error": e.to_string() })))
}

// ========== Handlers（认证由 AuthLayer 统一处理，handler 只做业务） ==========

async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    let snap = state.config.nexus_snapshot().await;
    ok(snap)
}

async fn add_provider(State(state): State<AppState>, Json(req): Json<ProviderRequest>) -> impl IntoResponse {
    match state.config.add_provider(&req.name, req.config).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn remove_provider(State(state): State<AppState>, Json(req): Json<NameRequest>) -> impl IntoResponse {
    match state.config.remove_provider(&req.name).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn add_channel(State(state): State<AppState>, Json(ch): Json<ChannelConfig>) -> impl IntoResponse {
    let channel_id = ch.channel_id.clone();
    let enabled = ch.enabled;
    match state.config.add_channel(ch).await {
        Ok(()) => {
            // 配置写入后同步运行态：先移除旧运行态（若有），再按新配置决定是否建立连接
            let channel_manager = &Nexus::get().channel_manager;
            channel_manager.stop_channel(channel_id.as_str()).await;
            if enabled {
                channel_manager.start_channel(channel_id.as_str()).await;
            }
            ok(json!({}))
        }
        Err(e) => fail(e),
    }
}

async fn remove_channel(State(state): State<AppState>, Json(req): Json<ChannelIdRequest>) -> impl IntoResponse {
    match state.config.remove_channel(&req.channel_id).await {
        Ok(()) => {
            // 删除配置后移除运行态并通知绑定该 channel 的 pipeline（input_channels 配置保留）
            Nexus::get().channel_manager.stop_channel(req.channel_id.as_str()).await;
            ok(json!({}))
        }
        Err(e) => fail(e),
    }
}

async fn enable_channel(State(state): State<AppState>, Json(req): Json<ChannelIdRequest>) -> impl IntoResponse {
    let Some(channel) = state.config.channel(req.channel_id.as_str()).await else {
        return fail(Error::ConfigNotFound(format!("channel 不存在: {}", req.channel_id).into()));
    };
    if !channel.enabled {
        if let Err(e) = state.config.set_channel_enabled(req.channel_id.as_str(), true).await {
            return fail(e);
        }
    }
    // 启用运行态：按当前 input 路由创建连接并绑定该 channel 的全部输入身份
    Nexus::get().channel_manager.start_channel(req.channel_id.as_str()).await;
    ok(json!({}))
}

async fn disable_channel(State(state): State<AppState>, Json(req): Json<ChannelIdRequest>) -> impl IntoResponse {
    if state.config.channel(req.channel_id.as_str()).await.is_none() {
        return fail(Error::ConfigNotFound(format!("channel 不存在: {}", req.channel_id).into()));
    }
    if let Err(e) = state.config.set_channel_enabled(req.channel_id.as_str(), false).await {
        return fail(e);
    }
    // 移除运行态：结束连接并通知绑定该 channel 的 pipeline 清理 channel 运行态（不改 input_channels 配置）
    Nexus::get().channel_manager.stop_channel(req.channel_id.as_str()).await;
    ok(json!({}))
}

async fn add_admin(State(state): State<AppState>, Json(req): Json<AddAdminRequest>) -> impl IntoResponse {
    let admin = Arc::new(ChannelUser {
        messenger_id: req.messenger_id,
        user_id: req.user_id,
    });
    match state.config.add_admin(req.channel_id.as_str(), admin).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn remove_admin(State(state): State<AppState>, Json(req): Json<RemoveAdminRequest>) -> impl IntoResponse {
    let admin = ChannelUser {
        messenger_id: req.messenger_id,
        user_id: req.user_id,
    };
    match state.config.remove_admin(req.channel_id.as_str(), &admin).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

// ========== station 配置管理（group 增删 + 子站启停，写配置后同步 Station 运行态） ==========

async fn add_station_group(State(state): State<AppState>, Json(req): Json<StationGroupRequest>) -> impl IntoResponse {
    match state.config.add_station_group(req.group).await {
        Ok(repo) => {
            // 同步运行态：按新 group 集合重建本级工具索引
            Station::get().apply_repo(&repo);
            ok(json!({}))
        }
        Err(e) => fail(e),
    }
}

async fn remove_station_group(State(state): State<AppState>, Json(req): Json<StationGroupRequest>) -> impl IntoResponse {
    match state.config.remove_station_group(req.group.as_str()).await {
        Ok(repo) => {
            // 同步运行态：该 group 的工具随索引重建失效
            Station::get().apply_repo(&repo);
            ok(json!({}))
        }
        Err(e) => fail(e),
    }
}

async fn enable_sub_station(State(state): State<AppState>, Json(req): Json<SubStationRequest>) -> impl IntoResponse {
    match state.config.set_sub_station_enabled(req.station_id.as_str(), true).await {
        Ok(repo) => {
            // 同步运行态：按当前配置建立 MCP 客户端，其工具重新参与列表与路由
            Station::get().apply_repo(&repo);
            ok(json!({}))
        }
        Err(e) => fail(e),
    }
}

async fn disable_sub_station(State(state): State<AppState>, Json(req): Json<SubStationRequest>) -> impl IntoResponse {
    match state.config.set_sub_station_enabled(req.station_id.as_str(), false).await {
        Ok(repo) => {
            // 同步运行态：拆除 MCP 客户端，其工具不再参与列表与路由
            Station::get().apply_repo(&repo);
            ok(json!({}))
        }
        Err(e) => fail(e),
    }
}

// ========== pipeline 管理 ==========

async fn add_pipeline(State(state): State<AppState>, Json(req): Json<PipelineRequest>) -> impl IntoResponse {
    let role_key = req.role_key();
    if !is_supported_pipeline_name(req.pipeline_name.as_str()) {
        return fail(Error::PipelineNotFound(req.pipeline_name, role_key));
    }
    if state.config.pipeline_config(req.agent_id.as_str(), req.role_name.as_str()).await.is_some() {
        return fail(Error::InvalidCommand(
            format!("pipeline 已存在: {}/{}", req.agent_id, req.role_name).into()));
    }
    let config = Arc::new(PipelineConfig { pipeline_name: req.pipeline_name.clone() });
    if let Err(e) = state.config.set_pipeline_config(req.agent_id.clone(), req.role_name.clone(), config).await {
        return fail(e);
    }
    if let Err(e) = Nexus::get().pipeline_manager.ensure_pipeline(role_key.clone(), req.pipeline_name).await {
        return fail(e);
    }
    // 按配置（可能已有 role 级 input_channels）绑定输入身份
    apply_input_channels(&role_key).await;
    ok(json!({}))
}

async fn remove_pipeline(State(state): State<AppState>, Json(req): Json<PipelineKeyRequest>) -> impl IntoResponse {
    if let Err(e) = state.config.remove_pipeline_config(req.agent_id.as_str(), req.role_name.as_str()).await {
        return fail(e);
    }
    // 删除运行态：解绑输入身份并通知 pipeline 清理 channel 运行态（input_channels 配置保留）
    Nexus::get().pipeline_manager.remove_pipeline(&req.role_key()).await;
    ok(json!({}))
}

async fn reload_pipeline(State(state): State<AppState>, Json(req): Json<PipelineKeyRequest>) -> impl IntoResponse {
    let Some(config) = state.config.pipeline_config(req.agent_id.as_str(), req.role_name.as_str()).await else {
        return fail(Error::ConfigNotFound(
            format!("pipeline 不存在: {}/{}", req.agent_id, req.role_name).into()));
    };
    match reload_pipeline_runtime(req.role_key(), config.pipeline_name.clone()).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn change_pipeline(State(state): State<AppState>, Json(req): Json<PipelineRequest>) -> impl IntoResponse {
    let role_key = req.role_key();
    if !is_supported_pipeline_name(req.pipeline_name.as_str()) {
        return fail(Error::PipelineNotFound(req.pipeline_name, role_key));
    }
    if state.config.pipeline_config(req.agent_id.as_str(), req.role_name.as_str()).await.is_none() {
        return fail(Error::ConfigNotFound(
            format!("pipeline 不存在: {}/{}", req.agent_id, req.role_name).into()));
    }
    // 名称变更：先写配置，再按新名称重新加载
    let config = Arc::new(PipelineConfig { pipeline_name: req.pipeline_name.clone() });
    if let Err(e) = state.config.set_pipeline_config(req.agent_id.clone(), req.role_name.clone(), config).await {
        return fail(e);
    }
    match reload_pipeline_runtime(role_key, req.pipeline_name).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn add_input_channel(State(state): State<AppState>, Json(req): Json<InputChannelRequest>) -> impl IntoResponse {
    let role_key = req.role_key();
    let Some(config) = state.config.pipeline_config(req.agent_id.as_str(), req.role_name.as_str()).await else {
        return fail(Error::ConfigNotFound(
            format!("pipeline 不存在: {}/{}", req.agent_id, req.role_name).into()));
    };
    if !Nexus::get().pipeline_manager.contains(role_key.as_ref()) {
        return fail(Error::PipelineNotFound(config.pipeline_name.clone(), role_key));
    }
    let input = Arc::new(req.input_channel());
    let current = merged_input_channels(&state.config, &role_key).await;
    if current.iter().any(|existing| existing.as_ref() == input.as_ref()) {
        return fail(Error::InvalidCommand("input channel 已存在".into()));
    }
    // 一个输入身份只能绑定一条 pipeline：已被其他 pipeline 占用时明确拒绝
    if let Some(owner) = Nexus::get().channel_manager.input_pipeline(
        input.channel_id.clone(), input.messenger_id.clone(), input.user_id.clone()) {
        if owner.as_ref() != role_key.as_ref() {
            return fail(Error::InvalidCommand("input channel 已被其他 pipeline 绑定".into()));
        }
    }
    let mut inputs: Vec<Arc<InputChannel>> = current.iter().cloned().collect();
    inputs.push(input);
    if let Err(e) = set_input_channels_config(&state.config, &role_key, &inputs).await {
        return fail(e);
    }
    Nexus::get().pipeline_manager.set_input_channels(&role_key, &inputs).await;
    ok(json!({}))
}

async fn remove_input_channel(State(state): State<AppState>, Json(req): Json<InputChannelRequest>) -> impl IntoResponse {
    let role_key = req.role_key();
    let Some(config) = state.config.pipeline_config(req.agent_id.as_str(), req.role_name.as_str()).await else {
        return fail(Error::ConfigNotFound(
            format!("pipeline 不存在: {}/{}", req.agent_id, req.role_name).into()));
    };
    if !Nexus::get().pipeline_manager.contains(role_key.as_ref()) {
        return fail(Error::PipelineNotFound(config.pipeline_name.clone(), role_key));
    }
    let target = req.input_channel();
    let current = merged_input_channels(&state.config, &role_key).await;
    let inputs: Vec<Arc<InputChannel>> = current.iter()
        .filter(|input| input.as_ref() != &target)
        .cloned()
        .collect();
    // 未登记时幂等成功
    if inputs.len() == current.len() {
        return ok(json!({}));
    }
    if let Err(e) = set_input_channels_config(&state.config, &role_key, &inputs).await {
        return fail(e);
    }
    Nexus::get().pipeline_manager.set_input_channels(&role_key, &inputs).await;
    ok(json!({}))
}

/// 卸载旧运行态后按名称重新装配，并按配置重新绑定输入身份（reload / change 共用）
async fn reload_pipeline_runtime(role_key: Arc<RoleKey>, pipeline_name: ArcStr) -> Result<()> {
    Nexus::get().pipeline_manager.remove_pipeline(&role_key).await;
    Nexus::get().pipeline_manager.ensure_pipeline(role_key.clone(), pipeline_name).await?;
    apply_input_channels(&role_key).await;
    Ok(())
}

/// 把 (agent, role) 合并后的 input_channels 同步到运行态（解绑已移除身份、绑定新增身份）
async fn apply_input_channels(role_key: &Arc<RoleKey>) {
    let inputs = merged_input_channels(ConfigManager::get(), role_key).await;
    Nexus::get().pipeline_manager.set_input_channels(role_key, inputs.as_slice()).await;
}

/// (agent, role) 合并后的 input_channels（role 级整体替换 agent 级）
async fn merged_input_channels(config: &ConfigManager, role_key: &RoleKey) -> Arc<Vec<Arc<InputChannel>>> {
    config.agent_role_config::<InputChannelsConfig>(
        role_key.agent_id.as_str(), role_key.role_name.as_str()).await
        .map(|config| config.input_channels)
        .unwrap_or_default()
}

/// 写入 (agent, role) 的 input_channels 配置（role_name 为空时写 agent 级）
async fn set_input_channels_config(
    config: &ConfigManager,
    role_key: &RoleKey,
    inputs: &[Arc<InputChannel>],
) -> Result<()> {
    config.set_agent_role_config::<InputChannelsConfig>(
        role_key.agent_id.clone(),
        role_key.role_name.clone(),
        Arc::new(InputChannelsConfig { input_channels: Arc::new(inputs.to_vec()) }),
    ).await
}

#[cfg(test)]
mod tests {
    use kissbot_llm::{ModelConfig, ProviderConfig};

    use super::*;
    use std::collections::HashMap;
    use std::sync::OnceLock;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::util::ServiceExt;

    fn test_provider() -> ProviderModelConfig {
        ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from("https://api.example.com"),
                api_key: ArcStr::from("sk-test"),
            }),
            default_model_config: ModelConfig {
                max_tokens_usage: 128000,
                timeout_secs: Some(60),
                retry_count: Some(3),
            },
            model_configs: Arc::new(HashMap::new()),
        }
    }

    // ConfigManager 字段私有，无法在模块外直接构造；
    // 通过 ConfigManager::new() 加载临时 KISSBOT_CONFIG 构造（data_dir 指向 tempdir，完成即注册单例）。
    // 注意：kissbot-config 是进程级 OnceLock 单例，本测试会触发它；ConfigManager 单例在测试进程内注册一次，
    // 之后 get() 取同一实例。目录用进程级 OnceLock 保活：单例可能先被其他测试注册，目录不能在测试结束时被删除。
    static TEST_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    // 多个用例并行时避免重复写同一 config.json（写与读交错会读到空文件）
    static TEST_INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

    async fn test_manager() {
        TEST_INIT.get_or_init(|| async {
            let dir = TEST_DIR.get_or_init(|| tempfile::tempdir().unwrap());
            let data_dir = dir.path().join("data");
            std::fs::create_dir_all(&data_dir).unwrap();
            // 预置 station.json（含一个停用的子站）：station 子站启停接口只能在已有配置上启停
            let station_json = format!(
                r#"{{"station_id":"station-self","groups":[],"sub_stations":{{"station-a":{{"station_id":"station-a","base_url":"http://127.0.0.1:9001","timeout_secs":5,"enabled":false}}}}}}"#
            );
            std::fs::write(data_dir.join("station.json"), station_json).unwrap();
            let cfg_path = dir.path().join("config.json");
            let cfg_json = format!(
                r#"{{"api":{{"memory_store_url":"","memory_ego_url":""}},"security":{{"api_key":"user-key-456","admin_api_key":"admin-key-123"}},"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9090,"ws_reconnect_interval_secs":5}}}}"#,
                data_dir.to_str().unwrap()
            );
            std::fs::write(&cfg_path, cfg_json).unwrap();
            // 2024 edition：设置环境变量需要 unsafe
            unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };
            ConfigManager::new().await.unwrap();
            // 管理 API 的运行态变更依赖 Nexus / Station 单例；重复注册为幂等
            let _ = Nexus::new().await;
            Station::new().await;
        }).await;
    }

    /// 读回落盘的 nexus.json（ConfigManager 的 data_dir = <tempdir>/data）
    fn saved_nexus_json() -> serde_json::Value {
        let path = TEST_DIR.get().expect("test_manager 应先初始化").path().join("data").join("nexus.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// 读回落盘的 station.json
    fn saved_station_json() -> serde_json::Value {
        let path = TEST_DIR.get().expect("test_manager 应先初始化").path().join("data").join("station.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
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
        test_manager().await;
        let server = HttpServer::new();
        let app = server.build_router();

        // 无 key → 401（AuthLayer 拦截）
        let (status, _) = send(app.clone(), "GET", "/config", "", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // 错误 key → 401（AuthLayer 拦截）
        let (status, _) = send(app.clone(), "GET", "/config", "wrong", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // GET /config 初始快照
        let (status, body) = send(app.clone(), "GET", "/config", "admin-key-123", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"]["providers"].is_object());
        assert!(body["data"]["channels"].is_object());
        // sessions 以普通对象承载（key = session_id）
        assert!(body["data"]["sessions"].is_object(), "sessions 应为对象");

        // POST /config/providers 添加
        let (status, body) = send(app.clone(), "POST", "/config/providers", "admin-key-123",
            Some(serde_json::json!({ "name": "deepseek", "config": test_provider()}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        // 重名 → 失败
        let (status, body) = send(app.clone(), "POST", "/config/providers", "admin-key-123",
            Some(serde_json::json!({ "name": "deepseek", "config": test_provider()}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false);

        // POST /config/providers/remove
        let (status, body) = send(app.clone(), "POST", "/config/providers/remove", "admin-key-123",
            Some(serde_json::json!({ "name": "deepseek" }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);

        // POST /config/channels 添加 + admins
        let (status, body) = send(app.clone(), "POST", "/config/channels", "admin-key-123",
            Some(serde_json::json!({
                "channel_id": "web-main", "ws_url": "ws://127.0.0.1:8201",
                "admins": [],
                "enabled": true
            }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        let (status, body) = send(app.clone(), "POST", "/config/admins", "admin-key-123",
            Some(serde_json::json!({ "channel_id": "web-main", "messenger_id": "web", "user_id": "u2" }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        // 落盘验证（ConfigManager data_dir = <tempdir>/data）
        let saved = saved_nexus_json();
        assert!(saved["providers"].is_object());
        assert_eq!(saved["channels"]["web-main"]["admins"][0]["user_id"], "u2", "add_admin 应落盘");
    }

    #[tokio::test]
    async fn pipeline_endpoints_crud() {
        test_manager().await;
        let app = HttpServer::new().build_router();
        let agent = "pipe-agent";

        // 不支持的 pipeline 名称 → 失败
        let (_, body) = send(app.clone(), "POST", "/config/pipelines", "admin-key-123",
            Some(serde_json::json!({
                "agent_id": agent, "role_name": "", "pipeline_name": "no-such-pipeline"
            }))).await;
        assert_eq!(body["success"], false);

        // 新增 default pipeline → 成功；重复新增 → 失败
        let (_, body) = send(app.clone(), "POST", "/config/pipelines", "admin-key-123",
            Some(serde_json::json!({
                "agent_id": agent, "role_name": "", "pipeline_name": "default"
            }))).await;
        assert_eq!(body["success"], true);
        let (_, body) = send(app.clone(), "POST", "/config/pipelines", "admin-key-123",
            Some(serde_json::json!({
                "agent_id": agent, "role_name": "", "pipeline_name": "default"
            }))).await;
        assert_eq!(body["success"], false, "重复新增应失败");

        // 添加 input channel（channel 未配置：允许登记，绑定运行态仅记警告）
        let input = serde_json::json!({
            "agent_id": agent, "role_name": "",
            "channel_id": "pipe-ch", "messenger_id": "web", "user_id": "u1"
        });
        let (_, body) = send(app.clone(), "POST", "/config/pipelines/input-channels", "admin-key-123",
            Some(input.clone())).await;
        assert_eq!(body["success"], true);
        // 重复添加同一身份 → 失败
        let (_, body) = send(app.clone(), "POST", "/config/pipelines/input-channels", "admin-key-123",
            Some(input.clone())).await;
        assert_eq!(body["success"], false, "重复 input channel 应失败");
        // 落盘验证
        let saved = saved_nexus_json();
        assert_eq!(saved["pipelines"][agent][""]["pipeline_name"], "default");
        assert_eq!(saved["agent_roles"][agent]["config_map"]["input_channels"]["input_channels"][0]["user_id"], "u1");

        // 移除 input channel → 成功（再次移除幂等成功）
        let (_, body) = send(app.clone(), "POST", "/config/pipelines/input-channels/remove", "admin-key-123",
            Some(input.clone())).await;
        assert_eq!(body["success"], true);
        let (_, body) = send(app.clone(), "POST", "/config/pipelines/input-channels/remove", "admin-key-123",
            Some(input)).await;
        assert_eq!(body["success"], true, "移除不存在的 input channel 幂等成功");

        // 更换 pipeline（名称变更）→ 成功，配置与运行态按新名称加载
        let (_, body) = send(app.clone(), "POST", "/config/pipelines/change", "admin-key-123",
            Some(serde_json::json!({
                "agent_id": agent, "role_name": "", "pipeline_name": "default_memory"
            }))).await;
        assert_eq!(body["success"], true);
        // 重新加载 → 成功
        let (_, body) = send(app.clone(), "POST", "/config/pipelines/reload", "admin-key-123",
            Some(serde_json::json!({ "agent_id": agent, "role_name": "" }))).await;
        assert_eq!(body["success"], true);

        // 移除 pipeline → 成功，配置中不再出现
        let (_, body) = send(app.clone(), "POST", "/config/pipelines/remove", "admin-key-123",
            Some(serde_json::json!({ "agent_id": agent, "role_name": "" }))).await;
        assert_eq!(body["success"], true);
        assert!(ConfigManager::get().pipeline_config(agent, "").await.is_none());
    }

    #[tokio::test]
    async fn channel_enable_disable_syncs_runtime() {
        test_manager().await;
        let app = HttpServer::new().build_router();

        // 新增 channel（enabled=false，不建立连接）
        let (_, body) = send(app.clone(), "POST", "/config/channels", "admin-key-123",
            Some(serde_json::json!({
                "channel_id": "toggle-ch", "ws_url": "ws://127.0.0.1:1",
                "admins": [], "enabled": false
            }))).await;
        assert_eq!(body["success"], true);

        // enable → 配置置 true 并创建运行态（连接失败会按间隔重试，不影响接口返回）
        let (_, body) = send(app.clone(), "POST", "/config/channels/enable", "admin-key-123",
            Some(serde_json::json!({ "channel_id": "toggle-ch" }))).await;
        assert_eq!(body["success"], true);
        assert_eq!(ConfigManager::get().channel("toggle-ch").await.unwrap().enabled, true);

        // disable → 配置置 false 并移除运行态
        let (_, body) = send(app.clone(), "POST", "/config/channels/disable", "admin-key-123",
            Some(serde_json::json!({ "channel_id": "toggle-ch" }))).await;
        assert_eq!(body["success"], true);
        assert_eq!(ConfigManager::get().channel("toggle-ch").await.unwrap().enabled, false);

        // 不存在的 channel → 失败
        let (_, body) = send(app.clone(), "POST", "/config/channels/disable", "admin-key-123",
            Some(serde_json::json!({ "channel_id": "no-such-channel" }))).await;
        assert_eq!(body["success"], false);
    }

    #[tokio::test]
    async fn station_endpoints_groups_and_sub_station_lifecycle() {
        test_manager().await;
        let app = HttpServer::new().build_router();

        // 新增 group（幂等）→ 落盘 + 运行态工具索引重建
        for _ in 0..2 {
            let (status, body) = send(app.clone(), "POST", "/config/station/groups", "admin-key-123",
                Some(serde_json::json!({ "group": "filesystem" }))).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["success"], true);
        }
        assert_eq!(saved_station_json()["groups"], serde_json::json!(["filesystem"]));
        assert!(!Station::get().list_tools_local(None).await.unwrap().is_empty(), "运行态应已有 read 工具");

        // 移除 group（幂等）→ 该 group 的工具失效
        for _ in 0..2 {
            let (_, body) = send(app.clone(), "POST", "/config/station/groups/remove", "admin-key-123",
                Some(serde_json::json!({ "group": "filesystem" }))).await;
            assert_eq!(body["success"], true);
        }
        assert_eq!(saved_station_json()["groups"], serde_json::json!([]));
        assert!(Station::get().list_tools_local(None).await.unwrap().is_empty(), "移除 group 后工具应失效");

        // 子站启停：预置的 station-a 初始停用
        let (_, body) = send(app.clone(), "POST", "/config/station/sub-stations/enable", "admin-key-123",
            Some(serde_json::json!({ "station_id": "station-a" }))).await;
        assert_eq!(body["success"], true);
        assert_eq!(saved_station_json()["sub_stations"]["station-a"]["enabled"], true, "启用落盘");

        let (_, body) = send(app.clone(), "POST", "/config/station/sub-stations/disable", "admin-key-123",
            Some(serde_json::json!({ "station_id": "station-a" }))).await;
        assert_eq!(body["success"], true);
        assert_eq!(saved_station_json()["sub_stations"]["station-a"]["enabled"], false, "停用落盘");

        // 不存在的子站 → 失败
        let (_, body) = send(app.clone(), "POST", "/config/station/sub-stations/enable", "admin-key-123",
            Some(serde_json::json!({ "station_id": "no-such-station" }))).await;
        assert_eq!(body["success"], false);
    }
}
