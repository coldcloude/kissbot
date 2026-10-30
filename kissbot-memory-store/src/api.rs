use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json,
    Router,
};

use arcstr::ArcStr;
use kissbot_api::*;
use crate::record::RecordManager;
use kissbot_memory::index::MemoryIndexer;

pub fn create_router() -> Router {
    Router::new()
        .route("/store/channel", post(append_channel_record))
        .route("/store/query/channel", post(query_channel_records))
        .route("/store/query/channel/recent", post(query_channel_recent_records))
}

async fn append_channel_record(Json(req): Json<memory::ChannelRequests>) -> impl IntoResponse {
    let result = {
        let record_manager = RecordManager::get();
        record_manager.append_channel_record(req.requests, req.force > 0).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(ArcStr::from(e.to_string())))),
    }
}

async fn query_channel_records(Json(req): Json<memory::QueryRequest>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_channel_records(req).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(ArcStr::from(e.to_string())))),
    }
}

async fn query_channel_recent_records(Json(req): Json<memory::RecentQuery>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_channel_recent(req.agent_id, req.role_event, req.count).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(ArcStr::from(e.to_string())))),
    }
}
