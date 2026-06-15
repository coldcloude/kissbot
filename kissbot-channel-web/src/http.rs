use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use axum::extract::multipart::Multipart;
use chrono::Utc;
use dashmap::DashMap;
use futures::stream::{Stream, StreamExt};
use kissbot_channel::GroupChangeType;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use crate::attachment::AttachmentStore;
use crate::messenger::{admin_user_group_id, WebMessenger};

// =========== SSE 分发器 ===========
// admin 不走 Channel 体系，独立 SSE 通道由 SseDispatcher 管理。

pub struct SseDispatcher {
    senders: DashMap<String, flume::Sender<String>>,
}

impl SseDispatcher {
    pub fn new() -> Self {
        Self { senders: DashMap::new() }
    }

    pub fn register(&self, group_id: &str) -> flume::Receiver<String> {
        let (tx, rx) = flume::unbounded();
        self.senders.insert(group_id.to_string(), tx);
        rx
    }

    pub fn push(&self, group_id: &str, data: &str) {
        if let Some(tx) = self.senders.get(group_id) {
            let _ = tx.send(data.to_string());
        }
    }
}

// ========== API 响应 ==========

#[derive(Debug, Serialize)]
struct ApiResponse<T: Serialize> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    fn success(data: T) -> Self {
        Self { success: true, data: Some(data), error: None }
    }
    fn error(msg: &str) -> Self {
        Self { success: false, data: None, error: Some(msg.to_string()) }
    }
}

// ========== DTOs ==========

#[derive(Debug, Serialize)]
pub struct ConnectResponse {
    pub user_id: String,
    pub user_name: String,
    pub is_admin: bool,
    pub messenger: MessengerInfoResponse,
}

#[derive(Debug, Serialize)]
pub struct MessengerInfoResponse {
    pub messenger_id: String,
    pub messenger_name: String,
    pub users: Vec<UserResponse>,
    pub groups: Vec<GroupResponse>,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub user_id: String,
    pub user_name: String,
}

/// 返回给前端的群组，仅包含 JSON 配置中真正存储的群组。
/// admin-user 自动生成的单聊群组（a_{user_id}）不在管理列表中展示。
#[derive(Debug, Serialize)]
pub struct GroupResponse {
    pub group_id: String,
    pub group_name: String,
    pub members: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub group_id: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Option<Vec<AttachmentRef>>,
}

#[derive(Debug, Deserialize)]
pub struct AttachmentRef {
    pub filename: String,
    pub key: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub group_name: String,
    #[serde(default)]
    pub member_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameGroupRequest {
    pub group_id: String,
    pub group_name: String,
}

#[derive(Debug, Deserialize)]
pub struct ManageMembersRequest {
    pub group_id: String,
    #[serde(default)]
    pub add_ids: Vec<String>,
    #[serde(default)]
    pub remove_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteGroupRequest {
    pub group_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub user_id: String,
    pub user_name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteUserRequest {
    pub user_id: String,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub msg_id: String,
    pub group_id: String,
    pub user_id: String,
    pub user_name: String,
    pub is_self: usize,
    pub msg_type: String,
    pub content: String,
    pub time: String,
    pub attachments: Vec<AttachmentRefResponse>,
}

#[derive(Debug, Serialize)]
pub struct AttachmentRefResponse {
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub key: String,
    pub has_thumbnail: bool,
}

// ========== AppState ==========

#[derive(Clone)]
pub struct AppState {
    pub messenger: Arc<WebMessenger>,
    pub attachment_store: Arc<AttachmentStore>,
    pub sse: Arc<SseDispatcher>,
}

// ========== Router ==========

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/api/connect", get(handle_connect))
        .route("/api/message/send", post(handle_send_message))
        .route("/api/messages", get(handle_get_messages))
        .route("/api/groups", get(handle_list_groups))
        .route("/api/groups/create", post(handle_create_group))
        .route("/api/groups/rename", post(handle_rename_group))
        .route("/api/groups/manage-members", post(handle_manage_members))
        .route("/api/groups/delete", post(handle_delete_group))
        .route("/api/users", get(handle_list_users))
        .route("/api/users/create", post(handle_create_user))
        .route("/api/users/delete", post(handle_delete_user))
        .route("/api/attachment/upload", post(handle_upload_attachment))
        .route("/api/attachment/download", get(handle_download_attachment))
        .route("/api/attachment/thumbnail", get(handle_thumbnail))
        .route("/api/events", get(handle_sse_events))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// ========== Handlers ==========

/// GET /api/connect
async fn handle_connect(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let admin = state.messenger.admin_info().await;
    let users_list = state.messenger.list_users().await;
    let groups = state.messenger.list_groups_raw().await;

    let mut users = Vec::new();
    users.push(UserResponse {
        user_id: admin.user_id.to_string(),
        user_name: admin.user_name.to_string(),
    });
    for u in &users_list {
        users.push(UserResponse {
            user_id: u.user_id.to_string(),
            user_name: u.user_name.to_string(),
        });
    }

    let groups_resp: Vec<GroupResponse> = groups.iter().map(|g| GroupResponse {
        group_id: g.group_id.to_string(),
        group_name: g.group_name.to_string(),
        members: g.members.iter().map(|m| m.to_string()).collect(),
    }).collect();

    Json(ApiResponse::success(ConnectResponse {
        user_id: admin.user_id.to_string(),
        user_name: admin.user_name.to_string(),
        is_admin: true,
        messenger: MessengerInfoResponse {
            messenger_id: "web".to_string(),
            messenger_name: "Web Chat".to_string(),
            users,
            groups: groups_resp,
        },
    }))
}

/// POST /api/message/send
async fn handle_send_message(
    State(state): State<AppState>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let (content, msg_type) = build_message_content(&req);

    if let Err(e) = state.messenger.admin_send_message(&req.group_id, &content, &msg_type, &time).await {
        return Json(ApiResponse::<serde_json::Value>::error(&e.to_string()));
    }

    Json(ApiResponse::success(serde_json::json!({
        "msg_id": "",
        "time": time
    })))
}

fn build_message_content(req: &SendMessageRequest) -> (String, String) {
    let atts = req.attachments.as_deref().unwrap_or_default();
    if atts.is_empty() {
        return (req.content.clone(), "text".to_string());
    }
    let att_info: Vec<serde_json::Value> = atts.iter().map(|a| {
        let is_image = a.filename.ends_with(".png") || a.filename.ends_with(".jpg") ||
                       a.filename.ends_with(".jpeg") || a.filename.ends_with(".gif") ||
                       a.filename.ends_with(".webp");
        serde_json::json!({
            "filename": a.filename,
            "key": a.key,
            "msg_type": if is_image { "image" } else { "file" }
        })
    }).collect();
    let content = serde_json::to_string(&serde_json::json!({
        "text": req.content,
        "attachments": att_info
    })).unwrap_or_else(|_| req.content.clone());
    (content, "mixed".to_string())
}

/// GET /api/messages — 暂返回空
async fn handle_get_messages(
    _state: State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let _group_id = match params.get("group_id") {
        Some(id) => id,
        None => return Json(ApiResponse::<Vec<MessageResponse>>::error("Missing group_id")),
    };
    Json(ApiResponse::success(Vec::<MessageResponse>::new()))
}

/// GET /api/groups — 仅返回 JSON 配置中真实存储的群组
async fn handle_list_groups(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let groups = state.messenger.list_groups_raw().await;
    let resp: Vec<GroupResponse> = groups.iter().map(|g| GroupResponse {
        group_id: g.group_id.to_string(),
        group_name: g.group_name.to_string(),
        members: g.members.iter().map(|m| m.to_string()).collect(),
    }).collect();
    Json(ApiResponse::success(resp))
}

/// POST /api/groups/create
async fn handle_create_group(
    State(state): State<AppState>,
    Json(req): Json<CreateGroupRequest>,
) -> impl IntoResponse {
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let admin = state.messenger.admin_info().await;

    let mut member_ids = req.member_ids;
    if !member_ids.iter().any(|m| m.as_str() == admin.user_id.as_str()) {
        member_ids.push(admin.user_id.to_string());
    }

    match state.messenger.add_group(&req.group_name, member_ids.clone()).await {
        Ok(group_id) => {
            for m in &member_ids {
                if m.as_str() != admin.user_id.as_str() {
                    state.messenger.notify_group_change(m, &group_id, GroupChangeType::Joined, &time).await;
                }
            }
            Json(ApiResponse::success(serde_json::json!({
                "group_id": group_id,
                "group_name": req.group_name
            })))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(&e.to_string())),
    }
}

/// POST /api/groups/rename
async fn handle_rename_group(
    State(state): State<AppState>,
    Json(req): Json<RenameGroupRequest>,
) -> impl IntoResponse {
    if state.messenger.is_admin_user_group(&req.group_id).await {
        return Json(ApiResponse::<serde_json::Value>::error("Admin-user group cannot be renamed"));
    }
    match state.messenger.rename_group(&req.group_id, &req.group_name).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(&e.to_string())),
    }
}

/// POST /api/groups/manage-members
async fn handle_manage_members(
    State(state): State<AppState>,
    Json(req): Json<ManageMembersRequest>,
) -> impl IntoResponse {
    if state.messenger.is_admin_user_group(&req.group_id).await {
        return Json(ApiResponse::<serde_json::Value>::error("Admin-user group cannot be modified"));
    }
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let gid = req.group_id.clone();

    match state.messenger.manage_members(&gid, &req.add_ids, &req.remove_ids).await {
        Ok(_) => {
            for add_id in &req.add_ids {
                state.messenger.notify_group_change(add_id, &gid, GroupChangeType::Joined, &time).await;
            }
            for remove_id in &req.remove_ids {
                state.messenger.notify_group_change(remove_id, &gid, GroupChangeType::Left, &time).await;
            }
            Json(ApiResponse::success(serde_json::json!({"success": true})))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(&e.to_string())),
    }
}

/// POST /api/groups/delete
async fn handle_delete_group(
    State(state): State<AppState>,
    Json(req): Json<DeleteGroupRequest>,
) -> impl IntoResponse {
    if state.messenger.is_admin_user_group(&req.group_id).await {
        return Json(ApiResponse::<serde_json::Value>::error("Admin-user group cannot be deleted"));
    }
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let group = state.messenger.get_group(&req.group_id).await;
    let admin_id = state.messenger.admin_info().await.user_id;

    match state.messenger.delete_group(&req.group_id).await {
        Ok(_) => {
            if let Some(g) = group {
                for m in g.members.iter() {
                    if m.as_str() != admin_id.as_str() {
                        state.messenger.notify_group_change(m, &req.group_id, GroupChangeType::Left, &time).await;
                    }
                }
            }
            Json(ApiResponse::success(serde_json::json!({"success": true})))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(&e.to_string())),
    }
}

/// GET /api/users
async fn handle_list_users(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let users = state.messenger.list_users().await;
    let resp: Vec<UserResponse> = users.iter().map(|u| UserResponse {
        user_id: u.user_id.to_string(),
        user_name: u.user_name.to_string(),
    }).collect();
    Json(ApiResponse::success(resp))
}

/// POST /api/users/create
async fn handle_create_user(
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> impl IntoResponse {
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    match state.messenger.add_user(&req.user_name).await {
        Ok(user_id) => {
            let group_id = admin_user_group_id(&user_id);
            state.messenger.notify_group_change(&user_id, &group_id, GroupChangeType::Joined, &time).await;
            Json(ApiResponse::success(serde_json::json!({
                "user_id": user_id,
                "user_name": req.user_name
            })))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(&e.to_string())),
    }
}

/// POST /api/users/delete
async fn handle_delete_user(
    State(state): State<AppState>,
    Json(req): Json<DeleteUserRequest>,
) -> impl IntoResponse {
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    match state.messenger.remove_user(&req.user_id).await {
        Ok(_) => {
            let group_id = admin_user_group_id(&req.user_id);
            state.messenger.notify_group_change(&req.user_id, &group_id, GroupChangeType::Left, &time).await;
            Json(ApiResponse::success(serde_json::json!({"success": true})))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(&e.to_string())),
    }
}

/// POST /api/attachment/upload
async fn handle_upload_attachment(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut result = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let filename = field.file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let mime_type = field.content_type()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                mime_guess::from_path(&filename).first_or_octet_stream().to_string()
            });
        let data = match field.bytes().await {
            Ok(d) => d.to_vec(),
            Err(_) => continue,
        };

        let group_id = "temp";
        let msg_id = Utc::now().format("%Y%m%d%H%M%S%6f").to_string();

        match state.attachment_store.save_attachment(group_id, &msg_id, &filename, &data, &mime_type) {
            Ok(meta) => {
                result.push(serde_json::json!({
                    "filename": filename,
                    "mime_type": mime_type,
                    "size_bytes": meta.size_bytes,
                    "key": format!("{}/{}/{}", group_id, msg_id, filename),
                    "has_thumbnail": meta.has_thumbnail,
                }));
            }
            Err(e) => {
                return Json(ApiResponse::<serde_json::Value>::error(&e.to_string()));
            }
        }
    }

    Json(ApiResponse::success(serde_json::Value::Array(result)))
}

/// GET /api/attachment/download
async fn handle_download_attachment(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let key = match params.get("key") {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, "Missing key").into_response(),
    };

    match state.attachment_store.get_attachment_by_key(key) {
        Ok(data) => {
            let mime = mime_guess::from_path(key).first_or_octet_stream();
            ([(axum::http::header::CONTENT_TYPE, mime.to_string())], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// GET /api/attachment/thumbnail
async fn handle_thumbnail(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let key = match params.get("key") {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, "Missing key").into_response(),
    };

    match state.attachment_store.get_thumbnail_by_key(key) {
        Ok(data) => {
            ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// GET /api/events — SSE 长连接
async fn handle_sse_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let groups = state.messenger.list_groups_raw().await;
    let sse = state.sse.clone();

    let mut receivers = Vec::new();
    for group in &groups {
        let rx = sse.register(&group.group_id);
        receivers.push(rx);
    }

    let streams: Vec<_> = receivers.into_iter().map(|rx| {
        rx.into_stream().map(|data| Ok(Event::default().data(data)))
    }).collect();

    let merged = futures::stream::select_all(streams);

    Sse::new(merged).keep_alive(KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("keep-alive"))
}
