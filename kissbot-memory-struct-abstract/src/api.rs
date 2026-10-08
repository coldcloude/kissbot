//! 对外 HTTP 接口：定位、case 查询、状态与重建（统一 `ApiResponse`，认证走 X-Api-Key）。

use std::sync::Arc;

use arcstr::ArcStr;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use kissbot_api::*;

use crate::agent_role::AgentRole;
use crate::service::AbstractService;

pub fn create_router(service: Arc<AbstractService>) -> Router {
    Router::new()
        .route("/abstract/locate", post(locate))
        .route("/abstract/case/list", post(case_list))
        .route("/abstract/case/summary", post(case_summary))
        .route("/abstract/case/records", post(case_records))
        .route("/abstract/case/records-after", post(case_records_after))
        .route("/abstract/case/records-tail", post(case_records_tail))
        .route("/abstract/tree/get", post(tree_get))
        .route("/abstract/rebuild", post(rebuild))
        .with_state(service)
}

/// 对外请求仍是「agent_id + 记忆后缀（RoleEvent）」：入口拆成 case 树的角色与独立的 event_id
fn agent_role_of(agent_id: &ArcStr, role_event: &RoleEvent) -> AgentRole {
    AgentRole::new(agent_id.clone(), role_event.role_name.clone())
}

type ApiResult<T> = (StatusCode, Json<ApiResponse<T>>);

fn ok<T>(data: T) -> ApiResult<T> {
    (StatusCode::OK, Json(ApiResponse::success(data)))
}

fn err<T>(message: impl Into<ArcStr>) -> ApiResult<T> {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(message)))
}

async fn locate(State(service): State<Arc<AbstractService>>, Json(req): Json<LocateRequest>) -> impl IntoResponse {
    match service.locate(&req).await {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn case_list(State(service): State<Arc<AbstractService>>, Json(req): Json<CaseListRequest>) -> impl IntoResponse {
    let agent_role = agent_role_of(&req.agent_id, &req.role_event);
    match service
        .case_list(&agent_role, req.parent_id.as_ref().map(|id| id.as_str()))
        .await
    {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn case_summary(State(service): State<Arc<AbstractService>>, Json(req): Json<CaseSummaryRequest>) -> impl IntoResponse {
    let agent_role = agent_role_of(&req.agent_id, &req.role_event);
    match service.case_summary(&agent_role, req.case_name.as_str()).await {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn case_records(State(service): State<Arc<AbstractService>>, Json(req): Json<CaseRecordsRequest>) -> impl IntoResponse {
    let agent_role = agent_role_of(&req.agent_id, &req.role_event);
    match service.case_records(&agent_role, req.case_name.as_str()).await {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn case_records_after(State(service): State<Arc<AbstractService>>, Json(req): Json<CaseRecordsAfterRequest>) -> impl IntoResponse {
    let agent_role = agent_role_of(&req.agent_id, &req.role_event);
    match service
        .case_records_after(&agent_role, req.case_name.as_str(), &req.after)
        .await
    {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn case_records_tail(State(service): State<Arc<AbstractService>>, Json(req): Json<CaseRecordsTailRequest>) -> impl IntoResponse {
    let agent_role = agent_role_of(&req.agent_id, &req.role_event);
    match service
        .case_records_tail(&agent_role, req.case_name.as_str(), req.count)
        .await
    {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn tree_get(State(service): State<Arc<AbstractService>>, Json(req): Json<TreeGetRequest>) -> impl IntoResponse {
    let agent_role = agent_role_of(&req.agent_id, &req.role_event);
    match service.tree_get(&agent_role, req.role_event.event_id.as_ref()).await {
        Ok(response) => ok(response),
        Err(e) => err(e.to_string()),
    }
}

async fn rebuild(State(service): State<Arc<AbstractService>>, Json(req): Json<RebuildRequest>) -> impl IntoResponse {
    let agent_role = agent_role_of(&req.agent_id, &req.role_event);
    let event_id = req.role_event.event_id.clone();
    if req.rebuild {
        match service.rebuild(&agent_role, event_id.as_ref()).await {
            Ok(response) => ok(response),
            Err(e) => err(e.to_string()),
        }
    } else {
        match service.manual_trigger(&agent_role, event_id.as_ref()).await {
            Ok(outcome) => {
                let case_count = service.case_count(&agent_role).await.unwrap_or(0);
                ok(RebuildResponse { archived: outcome.archived, case_count })
            }
            Err(e) => err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_role::AgentRoleManager;
    use crate::service::AbstractService;
    use crate::stub::StubModel;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_service() -> (tempfile::TempDir, Arc<AbstractService>) {
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(AgentRoleManager::new(temp.path()));
        let model = Arc::new(StubModel::new());
        let service = Arc::new(AbstractService::new(manager, model, 4));
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

        // 空角色定位：新话题也建出 case（记忆条数 0），分组一定带 case_name
        let (status, body) = post_json(router.clone(), "/abstract/locate", serde_json::json!({
            "agent_id": "0", "role_event": {"role_name": "dev"},
            "messages": [{"time": "2026-09-22 13:48:37", "user_name": "alice", "is_self": 0,
                          "messenger_id": "web", "self_user_id": "self1", "group_id": "g1",
                          "content": {"msg_type": "Text", "data": "你好"}}]
        })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(body["data"]["cases"][0]["case_name"], "role-case");
        assert_eq!(body["data"]["groups"][0]["case_name"], "role-case");

        // 查询只读：能看到刚由定位建出的 case（记忆条数 0，摘要来自逐层判定）
        let (status, body) = post_json(router.clone(), "/abstract/tree/get", serde_json::json!({"agent_id": "0", "role_event": {"role_name": "dev"}})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["nodes"][0]["case_name"], "role-case");
        assert_eq!(body["data"]["nodes"][0]["record_count"], 0);
        assert_eq!(body["data"]["nodes"][0]["summary"], "新 case 摘要");

        // case 不存在 → 失败响应（摘要与原文一致）
        for path in ["/abstract/case/summary", "/abstract/case/records"] {
            let (status, body) = post_json(router.clone(), path, serde_json::json!({"agent_id": "0", "role_event": {"role_name": "dev"}, "case_name": "role-9"})).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body["success"], false);
        }

        // 手动触发（无记录 → 0）
        let (status, body) = post_json(router, "/abstract/rebuild", serde_json::json!({"agent_id": "0", "role_event": {"role_name": "dev"}})).await;
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
        let (status, body) = post_json(router.clone(), "/abstract/rebuild", serde_json::json!({"agent_id": "0", "role_event": {"role_name": "dev"}, "rebuild": true})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["archived"], 1);
        assert_eq!(body["data"]["case_count"], 1);

        let (_, body) = post_json(router, "/abstract/case/list", serde_json::json!({"agent_id": "0", "role_event": {"role_name": "dev"}})).await;
        assert_eq!(body["data"]["cases"][0]["case_name"], "role-case");
        assert_eq!(body["data"]["cases"][0]["record_count"], 1);
    }
}
