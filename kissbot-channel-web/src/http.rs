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
use kissbot_api::ApiResponse;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use crate::messenger::{ADMIN_USER_ID, GroupConfig, UserConfig, WebMessenger};
use kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfo, Content, MessageItem, MSG_TYPE_ATTACHMENT, MSG_TYPE_MULTI, MSG_TYPE_TEXT};
use serde_json::Value;

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
    pub file_name: Arc<String>,
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
pub struct InitAttachmentRequest {
    pub group_id: Arc<String>,
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
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
        .route("/api/attachment/init", post(handle_init_attachment))
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
    let (content, msg_type) = build_message_content(&req);
    let outgoing = OutgoingMessage {
        messenger_id: messenger.messenger_id.clone(),
        user_id: ADMIN_USER_ID.clone(),
        group_id: req.group_id.clone(),
        msg_type: Arc::new(msg_type),
        content,
    };

    match messenger.send(Arc::new(outgoing)).await {
        Ok(resp) => Json(ApiResponse::success(serde_json::json!({
            "msg_id": resp.msg_id.as_str(),
            "time": resp.time.as_str(),
        }))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}

fn build_message_content(req: &SendMessageRequest) -> (Content, String) {
    let atts = req.attachments.as_deref().unwrap_or_default();
    if atts.is_empty() {
        return (Content::Text(Arc::new(req.content.to_string())), MSG_TYPE_TEXT.to_string());
    }
    // 构建 multi 类型消息
    let mut items: Vec<serde_json::Value> = Vec::new();
    // 文本部分
    if !req.content.is_empty() {
        items.push(serde_json::json!({
            "msg_type": MSG_TYPE_TEXT,
            "content": req.content,
        }));
    }
    // 附件部分
    for a in atts {
        let info = AttachmentInfo {
            file_name: a.file_name.clone(),
            mime_type: Arc::new(mime_guess::from_path(a.file_name.as_str())
                .first_or_octet_stream().to_string()),
            size_bytes: 0,
        };
        items.push(serde_json::json!({
            "msg_type": MSG_TYPE_ATTACHMENT,
            "content": serde_json::to_value(&info).unwrap_or_default(),
        }));
    }
    #[allow(unused_mut)]
    let mut content_value = Content::Multi(
        items.into_iter().map(|item| {
            let item_val = item.as_object().cloned().unwrap_or_default();
            let msg_type_val = item_val.get("msg_type").and_then(|v| v.as_str()).unwrap_or(MSG_TYPE_TEXT);
            let content_val = item_val.get("content").cloned().unwrap_or(serde_json::Value::Null);
            let content = match msg_type_val {
                MSG_TYPE_ATTACHMENT => {
                    match serde_json::from_value::<AttachmentInfo>(content_val.clone()) {
                        Ok(info) => Content::AttachmentInfo(Arc::new(info)),
                        Err(_) => Content::Text(Arc::new(content_val.to_string())),
                    }
                }
                _ => Content::Text(Arc::new(content_val.to_string())),
            };
            Arc::new(MessageItem {
                msg_type: Arc::new(msg_type_val.to_string()),
                content,
            })
        }).collect()
    );
    (content_value, MSG_TYPE_MULTI.to_string())
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

/// POST /api/attachment/init — 初始化附件上传，发送消息并返回 OutgoingMessageResponse
async fn handle_init_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<InitAttachmentRequest>,
) -> impl IntoResponse {
    let info = AttachmentInfo {
        file_name: req.file_name.clone(),
        mime_type: req.mime_type.clone(),
        size_bytes: req.size_bytes,
    };
    let outgoing = OutgoingMessage {
        messenger_id: messenger.messenger_id.clone(),
        user_id: ADMIN_USER_ID.clone(),
        group_id: req.group_id.clone(),
        msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
        content: Content::AttachmentInfo(Arc::new(info)),
    };

    match messenger.send(Arc::new(outgoing)).await {
        Ok(resp) => Json(ApiResponse::success(resp)),
        Err(e) => Json(ApiResponse::<Arc<OutgoingMessageResponse>>::error(e.to_string())),
    }
}

/// POST /api/attachment/upload — 上传文件实体
async fn handle_upload_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut attachment_key: Option<String> = None;
    let mut attachment_transfer_id: Option<u32> = None;
    let mut file_data: Option<Bytes> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().map(|s| s.to_string());
        if name.as_deref() == Some("key") {
            if let Ok(data) = field.text().await {
                attachment_key = Some(data.trim().to_string());
            }
        } else if name.as_deref() == Some("transfer_id") {
            if let Ok(data) = field.text().await {
                attachment_transfer_id = data.trim().parse::<u32>().ok();
            }
        } else if name.as_deref() == Some("file") {
            if let Ok(data) = field.bytes().await {
                file_data = Some(data);
            }
        }
    }

    let attachment_key = match attachment_key {
        Some(k) => k,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing key".to_string())),
    };

    let attachment_transfer_id = match attachment_transfer_id {
        Some(id) => id,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing transfer_id".to_string())),
    };

    let file_data = match file_data {
        Some(d) => d,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing file data".to_string())),
    };

    // 通过 AttachmentStore 写入
    match messenger.attachment_store.write_chunk(&attachment_key, attachment_transfer_id, 0, file_data.len() as u32, file_data).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
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
    let file_len = match messenger.attachment_store.open_file(key) {
        Ok((_, len)) => len,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let meta = match messenger.attachment_store.get_meta(key) {
        Ok(m) => m,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let mime = mime_guess::from_path(meta.file_name.as_str()).first_or_octet_stream().to_string();

    // 解析 Range header
    let range_header = headers.get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok());

    if let Some(range_str) = range_header {
        // 解析 "bytes=start-end" 格式
        if let Some((start, end)) = parse_range(range_str, file_len) {
            let length = end - start + 1;
            let (file, _) = match messenger.attachment_store.open_file(key) {
                Ok(f) => f,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            };
            match read_attachment_range(file, start, length) {
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
    let (file, _) = match messenger.attachment_store.open_file(key) {
        Ok(f) => f,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    match read_attachment_range(file, 0, file_len) {
        Ok(data) => {
            ([(axum::http::header::CONTENT_TYPE, mime)], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// 读取指定范围的文件数据
fn read_attachment_range(mut file: std::fs::File, offset: u64, length: u64) -> Result<Bytes, String> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; length as usize];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(Bytes::from(buf))
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
