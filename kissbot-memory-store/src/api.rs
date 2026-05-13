use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json,
    Router,
};

use kissbot_api::ApiResponse;
use crate::record::{ChannelRequest, RecordManager, ThinkingRequest, ToolCallRequest, ToolResultRequest};

pub fn create_router() -> Router {
    Router::new()
        .route("/store/channel", post(append_channel_record))
        .route("/store/think", post(append_thinking_record))
        .route("/store/tool-call", post(append_tool_call_record))
        .route("/store/tool-result", post(append_tool_result_record))
}

async fn append_channel_record(Json(req): Json<ChannelRequest>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_channel_record(req).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn append_thinking_record(Json(req): Json<ThinkingRequest>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_thinking_record(req).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn append_tool_call_record(Json(req): Json<ToolCallRequest>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_tool_call_record(req).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn append_tool_result_record(Json(req): Json<ToolResultRequest>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_tool_result_record(req).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
