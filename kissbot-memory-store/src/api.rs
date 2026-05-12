use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json,
    Router,
};

use kissbot_api::ApiResponse;
use crate::record::{ChannelRecord, RecordManager, ThinkingRecord, ToolRecord};

pub fn create_router() -> Router {
    Router::new()
        .route("/store/channel-record", post(append_channel_record))
        .route("/store/think-record", post(append_thinking_record))
        .route("/store/tool-record", post(append_tool_record))
}

async fn append_channel_record(Json(req): Json<ChannelRecord>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_channel_record(req).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn append_thinking_record(Json(req): Json<ThinkingRecord>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_thinking_record(req).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn append_tool_record(Json(req): Json<ToolRecord>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_tool_record(req).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
