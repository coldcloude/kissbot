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
use kissbot_api::ApiResponse;
use kissbot_channel::GroupChangeType;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use crate::messenger::{admin_user_group_id, ADMIN_USER_ID, GroupConfig, UserConfig, WebMessenger};

// ========== DTOs ==========

#[derive(Debug, Serialize)]
pub struct MessengerAdminInfo {
    pub messenger_id: Arc<String>,
    pub admin_name: Arc<String>,
    pub users: Arc<DashMap<String, Arc<UserConfig>>>,
    pub groups: Arc<DashMap<String, Arc<GroupConfig>>>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub group_id: Arc<String>,
    pub content: Arc<String>,
    #[serde(default)]
    pub attachments: Option<Vec<AttachmentRef>>,
}

#[derive(Debug, Deserialize)]
pub struct AttachmentRef {
    pub filename: Arc<String>,
    pub key: Arc<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub group_name: Arc<String>,
    #[serde(default)]
    pub member_ids: Vec<Arc<String>>,
}

#[derive(Debug, Deserialize)]
pub struct RenameGroupRequest {
    pub group_id: Arc<String>,
    pub group_name: Arc<String>,
}

#[derive(Debug, Deserialize)]
pub struct ManageMembersRequest {
    pub group_id: Arc<String>,
    #[serde(default)]
    pub add_ids: Vec<Arc<String>>,
    #[serde(default)]
    pub remove_ids: Vec<Arc<String>>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteGroupRequest {
    pub group_id: Arc<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub user_name: Arc<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteUserRequest {
    pub user_id: Arc<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameUserRequest {
    pub user_id: Arc<String>,
    pub user_name: Arc<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameAdminRequest {
    pub admin_name: Arc<String>,
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

// ========== Router ==========

pub fn create_router(messenger: Arc<WebMessenger>) -> Router {
    Router::new()
        .route("/api/info", get(handle_info))
        .route("/api/message/send", post(handle_send_message))
        .route("/api/messages", get(handle_get_messages))
        .route("/api/groups/create", post(handle_create_group))
        .route("/api/groups/rename", post(handle_rename_group))
        .route("/api/groups/manage-members", post(handle_manage_members))
        .route("/api/groups/delete", post(handle_delete_group))
        .route("/api/admin/rename", post(handle_rename_admin))
        .route("/api/users/create", post(handle_create_user))
        .route("/api/users/rename", post(handle_rename_user))
        .route("/api/users/delete", post(handle_delete_user))
        .route("/api/attachment/upload", post(handle_upload_attachment))
        .route("/api/attachment/download", get(handle_download_attachment))
        .route("/api/attachment/thumbnail", get(handle_thumbnail))
        .route("/api/events", get(handle_sse_events))
        .layer(CorsLayer::permissive())
        .with_state(messenger)
}

// ========== Handlers ==========

/// GET /api/info
async fn handle_info(
    State(messenger): State<Arc<WebMessenger>>,
) -> impl IntoResponse {
    Json(ApiResponse::success(MessengerAdminInfo {
        messenger_id: messenger.messenger_id.clone(),
        admin_name: messenger.admin_name().await,
        users: messenger.config_users().await,
        groups: messenger.config_groups().await,
    }))
}

/// POST /api/message/send
async fn handle_send_message(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let (content, msg_type) = build_message_content(&req);

    match messenger.send_from_admin(req.group_id.as_str(), &content, &msg_type, &time).await {
        Ok(msg_id) => Json(ApiResponse::success(serde_json::json!({
            "msg_id": msg_id.as_str(),
            "time": time
        }))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

fn build_message_content(req: &SendMessageRequest) -> (String, String) {
    let atts = req.attachments.as_deref().unwrap_or_default();
    if atts.is_empty() {
        return (req.content.to_string(), "text".to_string());
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
    })).unwrap_or_else(|_| req.content.to_string());
    (content, "mixed".to_string())
}

/// GET /api/messages — 暂返回空
async fn handle_get_messages(
    _messenger: State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let _group_id = match params.get("group_id") {
        Some(id) => id,
        None => return Json(ApiResponse::<Vec<MessageResponse>>::error("Missing group_id".to_string())),
    };
    Json(ApiResponse::success(Vec::<MessageResponse>::new()))
}

/// POST /api/groups/create
async fn handle_create_group(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<CreateGroupRequest>,
) -> impl IntoResponse {
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let mut member_ids: Vec<String> = req.member_ids.iter().map(|m| m.to_string()).collect();
    if !member_ids.iter().any(|m| m.as_str() == ADMIN_USER_ID.as_str()) {
        member_ids.push(ADMIN_USER_ID.to_string());
    }

    match messenger.add_group(req.group_name.as_str(), member_ids.clone()).await {
        Ok(group_id) => {
            for m in &member_ids {
                if m.as_str() != ADMIN_USER_ID.as_str() {
                    messenger.notify_group_change(m, &group_id, GroupChangeType::Joined, &time).await;
                }
            }
            Json(ApiResponse::success(serde_json::json!({
                "group_id": group_id,
            })))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/groups/rename
async fn handle_rename_group(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<RenameGroupRequest>,
) -> impl IntoResponse {
    if messenger.parse_admin_user_group(req.group_id.as_str()).await.is_some() {
        return Json(ApiResponse::<serde_json::Value>::error("Admin-user group cannot be renamed".to_string()));
    }
    match messenger.rename_group(req.group_id.as_str(), req.group_name.as_str()).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/groups/manage-members
async fn handle_manage_members(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<ManageMembersRequest>,
) -> impl IntoResponse {
    if messenger.parse_admin_user_group(req.group_id.as_str()).await.is_some() {
        return Json(ApiResponse::<serde_json::Value>::error("Admin-user group cannot be modified".to_string()));
    }
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let add_ids: Vec<String> = req.add_ids.iter().map(|a| a.to_string()).collect();
    let remove_ids: Vec<String> = req.remove_ids.iter().map(|r| r.to_string()).collect();
    match messenger.manage_members(req.group_id.as_str(), &add_ids, &remove_ids).await {
        Ok(_) => {
            for add_id in &add_ids {
                messenger.notify_group_change(add_id, req.group_id.as_str(), GroupChangeType::Joined, &time).await;
            }
            for remove_id in &remove_ids {
                messenger.notify_group_change(remove_id, req.group_id.as_str(), GroupChangeType::Left, &time).await;
            }
            Json(ApiResponse::success(serde_json::json!({"success": true})))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/groups/delete
async fn handle_delete_group(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<DeleteGroupRequest>,
) -> impl IntoResponse {
    if messenger.parse_admin_user_group(req.group_id.as_str()).await.is_some() {
        return Json(ApiResponse::<serde_json::Value>::error("Admin-user group cannot be deleted".to_string()));
    }
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let group = messenger.get_group(req.group_id.as_str()).await;

    match messenger.delete_group(req.group_id.as_str()).await {
        Ok(_) => {
            if let Some(g) = group {
                for m in g.members.iter() {
                    if m.as_str() != ADMIN_USER_ID.as_str() {
                        messenger.notify_group_change(m.as_str(), req.group_id.as_str(), GroupChangeType::Left, &time).await;
                    }
                }
            }
            Json(ApiResponse::success(serde_json::json!({"success": true})))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/users/create
async fn handle_create_user(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<CreateUserRequest>,
) -> impl IntoResponse {
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    match messenger.add_user(req.user_name.as_str()).await {
        Ok(ref user_id) => {
            let group_id = admin_user_group_id(user_id);
            messenger.notify_group_change(user_id, &group_id, GroupChangeType::Joined, &time).await;
            Json(ApiResponse::success(serde_json::json!({
                "user_id": user_id,
            })))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/users/rename
async fn handle_rename_user(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<RenameUserRequest>,
) -> impl IntoResponse {
    match messenger.rename_user(req.user_id.as_str(), req.user_name.as_str()).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/admin/rename
async fn handle_rename_admin(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<RenameAdminRequest>,
) -> impl IntoResponse {
    match messenger.update_admin_name(req.admin_name.as_str()).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/users/delete
async fn handle_delete_user(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<DeleteUserRequest>,
) -> impl IntoResponse {
    let time = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    match messenger.remove_user(req.user_id.as_str()).await {
        Ok(_) => {
            let group_id = admin_user_group_id(req.user_id.as_str());
            messenger.notify_group_change(req.user_id.as_str(), &group_id, GroupChangeType::Left, &time).await;
            Json(ApiResponse::success(serde_json::json!({"success": true})))
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/attachment/upload
async fn handle_upload_attachment(
    State(messenger): State<Arc<WebMessenger>>,
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

        match messenger.attachment_store.save_attachment(group_id, &msg_id, &filename, &data, &mime_type) {
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
                return Json(ApiResponse::<serde_json::Value>::error(e.to_string()));
            }
        }
    }

    Json(ApiResponse::success(serde_json::Value::Array(result)))
}

/// GET /api/attachment/download
async fn handle_download_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let key = match params.get("key") {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, "Missing key").into_response(),
    };

    match messenger.attachment_store.get_attachment_by_key(key) {
        Ok(data) => {
            let mime = mime_guess::from_path(key).first_or_octet_stream();
            ([(axum::http::header::CONTENT_TYPE, mime.to_string())], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// GET /api/attachment/thumbnail
async fn handle_thumbnail(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let key = match params.get("key") {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, "Missing key").into_response(),
    };

    match messenger.attachment_store.get_thumbnail_by_key(key) {
        Ok(data) => {
            ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// GET /api/events — SSE 长连接
async fn handle_sse_events(
    State(messenger): State<Arc<WebMessenger>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let groups = messenger.config_groups().await;
    let sse = messenger.sse.clone();

    let mut receivers = Vec::new();
    for group in groups.iter() {
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
