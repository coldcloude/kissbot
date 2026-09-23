//! 对外 HTTP 接口：定位、事件查询、状态与重建（统一 `ApiResponse`，认证走 X-Api-Key）。

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use kissbot_api::*;

use crate::service::AbstractService;

pub fn create_router(service: Arc<AbstractService>) -> Router {
    Router::new()
        .route("/abstract/locate", post(locate))
        .route("/abstract/event/list", post(event_list))
        .route("/abstract/event/summary", post(event_summary))
        .route("/abstract/event/records", post(event_records))
        .route("/abstract/event/records-after", post(event_records_after))
        .route("/abstract/event/records-tail", post(event_records_tail))
        .route("/abstract/tree/get", post(tree_get))
        .route("/abstract/rebuild", post(rebuild))
        .with_state(service)
}

type ApiResult<T> = (StatusCode, Json<ApiResponse<T>>);

fn ok<T>(data: T) -> ApiResult<T> {
    (StatusCode::OK, Json(ApiResponse::success(data)))
}

fn err<T>(message: String) -> ApiResult<T> {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(message)))
}

async fn locate(State(service): State<Arc<AbstractService>>, Json(req): Json<LocateRequest>) -> impl IntoResponse {
    match service.locate(&req).await {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn event_list(State(service): State<Arc<AbstractService>>, Json(req): Json<EventListRequest>) -> impl IntoResponse {
    match service
        .event_list(
            req.agent_id.as_str(),
            req.role.as_str(),
            req.parent_id.as_ref().map(|id| id.as_str()),
        )
        .await
    {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn event_summary(State(service): State<Arc<AbstractService>>, Json(req): Json<EventSummaryRequest>) -> impl IntoResponse {
    match service.event_summary(req.agent_id.as_str(), req.role.as_str(), req.event_id.as_str()).await {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn event_records(State(service): State<Arc<AbstractService>>, Json(req): Json<EventRecordsRequest>) -> impl IntoResponse {
    match service.event_records(req.agent_id.as_str(), req.role.as_str(), req.event_id.as_str()).await {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn event_records_after(State(service): State<Arc<AbstractService>>, Json(req): Json<EventRecordsAfterRequest>) -> impl IntoResponse {
    match service
        .event_records_after(req.agent_id.as_str(), req.role.as_str(), req.event_id.as_str(), &req.after)
        .await
    {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn event_records_tail(State(service): State<Arc<AbstractService>>, Json(req): Json<EventRecordsTailRequest>) -> impl IntoResponse {
    match service
        .event_records_tail(req.agent_id.as_str(), req.role.as_str(), req.event_id.as_str(), req.count)
        .await
    {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn tree_get(State(service): State<Arc<AbstractService>>, Json(req): Json<TreeGetRequest>) -> impl IntoResponse {
    match service.tree_get(req.agent_id.as_str(), req.role.as_str()).await {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn rebuild(State(service): State<Arc<AbstractService>>, Json(req): Json<RebuildRequest>) -> impl IntoResponse {
    let id = crate::scope::ScopeId::new(req.agent_id.as_str(), req.role.as_str());
    if req.rebuild {
        match service.rebuild(&id).await {
            Ok(response) => ok(response),
            Err(e) => err(e.to_string()),
        }
    } else {
        match service.manual_trigger(&id).await {
            Ok(outcome) => {
                let event_count = service.event_count(id.agent_id.as_str(), id.role.as_str()).await.unwrap_or(0);
                ok(RebuildResponse { archived: outcome.archived, event_count })
            }
            Err(e) => err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::ScopeManager;
    use crate::service::AbstractService;
    use crate::stub::StubModel;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_service() -> (tempfile::TempDir, Arc<AbstractService>) {
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(ScopeManager::new(temp.path()));
        let model = Arc::new(StubModel::new());
        let service = Arc::new(AbstractService::new(manager, model, 300, 4));
        (temp, service)
    }

    async fn post_json(router: Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn routes_return_api_response() {
        let (_temp, service) = test_service();
        let router = create_router(service);

        // 空作用域定位：没有消息涉及的事件
        let (status, body) = post_json(router.clone(), "/abstract/locate", serde_json::json!({
            "agent_id": "0", "role": "dev",
            "messages": [{"time": "2026-09-22 13:48:37", "user_name": "alice", "is_self": 0,
                          "messenger_id": "web", "self_user_id": "self1", "group_id": "g1",
                          "content": {"msg_type": "Text", "data": "你好"}}]
        })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert!(body["data"]["events"].is_null(), "没有已存在事件时不返回 events");

        // 空树查询
        let (status, body) = post_json(router.clone(), "/abstract/tree/get", serde_json::json!({"agent_id": "0", "role": "dev"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["nodes"], serde_json::json!([]));

        // 事件树查询（空作用域）
        let (status, body) = post_json(router.clone(), "/abstract/tree/get", serde_json::json!({"agent_id": "0", "role": "dev"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["nodes"], serde_json::json!([]));

        // 事件不存在 → 失败响应（摘要与原文一致）
        for path in ["/abstract/event/summary", "/abstract/event/records"] {
            let (status, body) = post_json(router.clone(), path, serde_json::json!({"agent_id": "0", "role": "dev", "event_id": "e1"})).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body["success"], false);
        }

        // 手动触发（无记录 → 0）
        let (status, body) = post_json(router, "/abstract/rebuild", serde_json::json!({"agent_id": "0", "role": "dev"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["archived"], 0);
    }

    #[tokio::test]
    async fn rebuild_route_archives_records() {
        let (temp, service) = test_service();
        // 直接写共享记忆文件
        let dir = temp.path().join("0").join("memory-store").join("2026-dev");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let record = serde_json::json!({
            "user_id": "self", "self_user_id": "self", "messenger_id": "web", "group_id": "g1",
            "is_self": 1, "messenger_name": "", "user_name": "alice", "group_name": "",
            "content": {"msg_type": "Text", "data": "你好"}, "time": "2026-09-22 10:00:00", "sn": 1
        });
        tokio::fs::write(dir.join("channel-records-2026-09-22.jsonl"), format!("{}\n", record)).await.unwrap();

        let router = create_router(service);
        let (status, body) = post_json(router.clone(), "/abstract/rebuild", serde_json::json!({"agent_id": "0", "role": "dev", "rebuild": true})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["archived"], 1);
        assert_eq!(body["data"]["event_count"], 1);

        let (_, body) = post_json(router, "/abstract/event/list", serde_json::json!({"agent_id": "0", "role": "dev"})).await;
        assert_eq!(body["data"]["events"][0]["event_id"], "e1");
        assert_eq!(body["data"]["events"][0]["record_count"], 1);
    }
}
