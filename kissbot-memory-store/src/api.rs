use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json,
    Router,
};

use kissbot_api::*;
use crate::record::RecordManager;
use kissbot_memory::index::MemoryIndexer;

pub fn create_router() -> Router {
    Router::new()
        .route("/store/channel", post(append_channel_record))
        .route("/store/think", post(append_think_record))
        .route("/store/tool-call", post(append_tool_call_record))
        .route("/store/tool-result", post(append_tool_result_record))
        .route("/store/query/channel", post(query_channel_records))
        .route("/store/query/think", post(query_think_records))
        .route("/store/query/tool-call", post(query_tool_call_records))
        .route("/store/query/tool-result", post(query_tool_result_records))
        .route("/store/query/combos", post(query_combos))
}

async fn append_channel_record(Json(req): Json<memory::ChannelRequests>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_channel_record(req.requests, req.force > 0).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn append_think_record(Json(req): Json<memory::ThinkRequests>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_think_record(req.requests, req.force > 0).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn append_tool_call_record(Json(req): Json<memory::ToolCallRequests>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_tool_call_record(req.requests, req.force > 0).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn append_tool_result_record(Json(req): Json<memory::ToolResultRequests>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_tool_result_record(req.requests, req.force > 0).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn query_channel_records(Json(req): Json<memory::QueryChannelRequest>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_channel_records(req).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn query_think_records(Json(req): Json<memory::QueryRequest>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_think_records(req).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn query_tool_call_records(Json(req): Json<memory::QueryRequest>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_tool_call_records(req).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn query_tool_result_records(Json(req): Json<memory::QueryRequest>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_tool_result_records(req).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

/// 查询 (agent, role, 时间范围) 对应的 channel 记录组合（messenger + user + group），
/// agent 先取组合、再对每个组合用 /store/query/channel 精确查询（记忆打包流程）
async fn query_combos(Json(req): Json<memory::QueryRequest>) -> impl IntoResponse {
    let combos = MemoryIndexer::get().query_combos(req).await;
    match combos {
        Ok(combos) => (StatusCode::OK, Json(ApiResponse::success(combos))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
