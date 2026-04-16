use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::ego_manager::EgoManager;
use kissbot_memory::{AgentManager, AgentMetadata};

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentNameRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentDescriptionRequest {
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

pub fn create_router() -> Router {
    Router::new()
        .route("/agents", post(create_agent))
        .route("/agents", get(list_agents))
        .route("/agents/:agent_id", get(get_agent))
        .route("/agents/:agent_id/name", put(update_agent_name))
        .route("/agents/:agent_id/description", put(update_agent_description))
        .route("/agents/:agent_id/identity", get(get_identity))
        .route("/agents/:agent_id/user-recognition", get(get_user_recognition))
}

async fn create_agent(Json(req): Json<CreateAgentRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.create_agent(req.name, req.description).await
    };
    
    match result {
        Ok(agent) => {
            let ego_manager = EgoManager::new();
            let _ = ego_manager.ensure_identity_md(&agent.id).await;
            (StatusCode::OK, Json(ApiResponse::success(agent)))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<AgentMetadata>::error(e.to_string())),
        ),
    }
}

async fn list_agents() -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.list_agents().await
    };
    
    match result {
        Ok(agents) => (StatusCode::OK, Json(ApiResponse::success(agents))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<AgentMetadata>>::error(e.to_string())),
        ),
    }
}

async fn get_agent(Path(agent_id): Path<String>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.get_agent(&agent_id).await
    };
    
    match result {
        Ok(agent) => (StatusCode::OK, Json(ApiResponse::success(agent))),
        Err(kissbot_memory::Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<AgentMetadata>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<AgentMetadata>::error(e.to_string())),
        ),
    }
}

async fn update_agent_name(
    Path(agent_id): Path<String>,
    Json(req): Json<UpdateAgentNameRequest>,
) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.update_agent_name(&agent_id, req.name).await
    };
    
    match result {
        Ok(agent) => (StatusCode::OK, Json(ApiResponse::success(agent))),
        Err(kissbot_memory::Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<AgentMetadata>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<AgentMetadata>::error(e.to_string())),
        ),
    }
}

async fn update_agent_description(
    Path(agent_id): Path<String>,
    Json(req): Json<UpdateAgentDescriptionRequest>,
) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.update_agent_description(&agent_id, req.description).await
    };
    
    match result {
        Ok(agent) => (StatusCode::OK, Json(ApiResponse::success(agent))),
        Err(kissbot_memory::Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<AgentMetadata>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<AgentMetadata>::error(e.to_string())),
        ),
    }
}

async fn get_identity(Path(agent_id): Path<String>) -> impl IntoResponse {
    let result = {
        let ego_manager = EgoManager::new();
        ego_manager.get_identity_md(&agent_id).await
    };
    
    match result {
        Ok(content) => (StatusCode::OK, Json(ApiResponse::success(content))),
        Err(crate::error::Error::Memory(kissbot_memory::Error::AgentNotFound(_))) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<String>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        ),
    }
}

async fn get_user_recognition(Path(agent_id): Path<String>) -> impl IntoResponse {
    let result = {
        let ego_manager = EgoManager::new();
        ego_manager.get_user_recognition_md(&agent_id).await
    };
    
    match result {
        Ok(content) => (StatusCode::OK, Json(ApiResponse::success(content))),
        Err(crate::error::Error::Memory(kissbot_memory::Error::AgentNotFound(_))) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<String>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(crate::error::Error::SettingNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<String>::error("user-recognition.md not found".to_string())),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        ),
    }
}
