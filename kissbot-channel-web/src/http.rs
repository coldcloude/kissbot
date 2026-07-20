use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use bytes::Bytes;
use std::time::Duration;

use axum::{
    extract::{Query, State},
    http::{StatusCode, HeaderMap},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use axum::extract::multipart::Multipart;
use dashmap::DashMap;
use futures::stream::{Stream, StreamExt};
use kissbot_api::{ApiResponse, AttachmentPayloadResponse};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use crate::messenger::{GroupConfig, UserConfig, WebMessenger};
use kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse};

// ========== DTOs ==========

#[derive(Debug, Serialize)]
pub struct MessengerAdminInfo {
    pub messenger_id: Arc<String>,
    pub admin_name: Arc<String>,
    pub users: Arc<DashMap<String, Arc<UserConfig>>>,
    pub groups: Arc<DashMap<String, Arc<GroupConfig>>>,
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
    pub file_name: String,
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
    Json(outgoing): Json<Arc<OutgoingMessage>>,
) -> impl IntoResponse {
    match messenger.send(outgoing).await {
        Ok(resp) => Json(ApiResponse::success(resp)),
        Err(e) => Json(ApiResponse::<Arc<OutgoingMessageResponse>>::error(e.to_string())),
    }
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
    let member_ids: Vec<String> = req.member_ids.iter().map(|m| m.to_string()).collect();

    match messenger.add_group(req.group_name.as_str(), member_ids).await {
        Ok(group_id) => Json(ApiResponse::success(serde_json::json!({
            "group_id": group_id,
        }))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/groups/rename
async fn handle_rename_group(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<RenameGroupRequest>,
) -> impl IntoResponse {
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
    let add_ids: Vec<String> = req.add_ids.iter().map(|a| a.to_string()).collect();
    let remove_ids: Vec<String> = req.remove_ids.iter().map(|r| r.to_string()).collect();
    match messenger.manage_members(req.group_id.as_str(), &add_ids, &remove_ids).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/groups/delete
async fn handle_delete_group(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<DeleteGroupRequest>,
) -> impl IntoResponse {
    match messenger.delete_group(req.group_id.as_str()).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/users/create
async fn handle_create_user(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<CreateUserRequest>,
) -> impl IntoResponse {
    match messenger.add_user(req.user_name.as_str()).await {
        Ok(ref user_id) => Json(ApiResponse::success(serde_json::json!({
            "user_id": user_id,
        }))),
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
    match messenger.remove_user(req.user_id.as_str()).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

/// POST /api/attachment/upload — 上传文件实体
async fn handle_upload_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut attachment_transfer_id: Option<u32> = None;
    let mut file_data: Option<Bytes> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().map(|s| s.to_string());
        if name.as_deref() == Some("transfer_id") {
            if let Ok(data) = field.text().await {
                attachment_transfer_id = data.trim().parse::<u32>().ok();
            }
        } else if name.as_deref() == Some("file") {
            if let Ok(data) = field.bytes().await {
                file_data = Some(data);
            }
        }
    }

    let attachment_transfer_id = match attachment_transfer_id {
        Some(id) => id,
        None => return Json(ApiResponse::<AttachmentPayloadResponse>::error("Missing transfer_id".to_string())),
    };

    let file_data = match file_data {
        Some(d) => d,
        None => return Json(ApiResponse::<AttachmentPayloadResponse>::error("Missing file data".to_string())),
    };

    // 通过 AttachmentStore 写入
    match messenger.attachment_store.write_chunk(attachment_transfer_id, 0, file_data.len() as u32, file_data).await {
        Ok(resp) => Json(ApiResponse::success(resp)),
        Err(e) => Json(ApiResponse::<AttachmentPayloadResponse>::error(e.to_string())),
    }
}

/// GET /api/attachment/download — 支持 Range 断点续传
async fn handle_download_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let key = match params.get("key") {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, "Missing key").into_response(),
    };

    // 获取文件元数据
    let meta = match messenger.attachment_store.get_meta(key) {
        Ok(m) => m,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let file_len = meta.info.size_bytes;
    let mime = meta.info.mime_type.as_str().to_string();

    // 解析 Range header
    let range_header = headers.get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok());

    if let Some(range_str) = range_header {
        // 解析 "bytes=start-end" 格式
        if let Some((start, end)) = parse_range(range_str, file_len) {
            let length = end - start + 1;
            match messenger.attachment_store.read_attachment_range(key, start, length) {
                Ok(data) => {
                    let content_range = format!("bytes {}-{}/{}", start, end, file_len);
                    return (
                        StatusCode::PARTIAL_CONTENT,
                        [
                            (axum::http::header::CONTENT_RANGE, content_range),
                            (axum::http::header::CONTENT_TYPE, mime),
                        ],
                        data,
                    ).into_response();
                }
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
    }

    // 无 Range 或 Range 解析失败，返回全量文件
    match messenger.attachment_store.read_attachment_range(key, 0, file_len) {
        Ok(data) => {
            ([(axum::http::header::CONTENT_TYPE, mime)], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// 解析 "bytes=start-end" 格式的 Range header
fn parse_range(range_str: &str, file_len: u64) -> Option<(u64, u64)> {
    let range_str = range_str.strip_prefix("bytes=")?;
    let (start_str, end_str) = range_str.split_once('-')?;
    let start: u64 = start_str.parse().ok()?;
    let end: u64 = end_str.parse().ok()?;
    if start >= file_len || end >= file_len || start > end {
        return None;
    }
    Some((start, end))
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

    match messenger.attachment_store.get_thumbnail(key) {
        Ok(data) => {
            ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// GET /api/events — SSE 长连接（全局广播，不再按 group 注册）
async fn handle_sse_events(
    State(messenger): State<Arc<WebMessenger>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = messenger.sse.register();
    let stream = rx.into_stream().map(|data| Ok(Event::default().data(data)));
    Sse::new(stream).keep_alive(KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("keep-alive"))
}
