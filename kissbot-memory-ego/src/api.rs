use std::sync::Arc;

use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use futures::future;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};

use crate::agent::{AgentManager, AgentMetadata};
use crate::ego_manager::EgoManager;
use crate::error::Error;

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

#[derive(Debug, Deserialize)]
pub struct UpdateAgentNameDescriptionRequest {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub keyword: String,
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
        .route("/agents/:agent_id/name-description", put(update_agent_name_description))
        .route("/agents/search/name", post(search_by_name))
        .route("/agents/search/description", post(search_by_description))
        .route("/agents/:agent_id/identity", get(get_identity))
        .route("/agents/:agent_id/user-recognition", get(get_user_recognition))
}

async fn create_agent(Json(req): Json<CreateAgentRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.create_agent(req.name, req.description).await
    };
    
    match result {
        Ok(()) => {
            (StatusCode::OK, Json(ApiResponse::<()>::success(())))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn list_agents() -> impl IntoResponse {
    //先获取所有agent id
    let agent_ids = {
        match DirectoryManager::get().list_agents().await {
            Ok(agent_ids) => agent_ids,
            Err(_) => Vec::new(),
        }
    };
    
    //并发获取所有agent metadata
    let agent_manager = AgentManager::get();
    let mut agents = Vec::new();
    let mut futs = Vec::new();
    agent_ids.iter().for_each(|agent_id| {
        futs.push(agent_manager.get_metadata(&agent_id));
    });
    let results = future::join_all(futs).await;
    for result in results {
        if let Ok(metadata) = result {
            agents.push(metadata);
        }
    }
    
    (StatusCode::OK, Json(ApiResponse::success(agents)))
}

async fn get_agent(Path(agent_id): Path<String>) -> impl IntoResponse {
    match AgentManager::get().get_metadata(&agent_id).await {
        Ok(agent) => (StatusCode::OK, Json(ApiResponse::success(agent))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<AgentMetadata>>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<AgentMetadata>>::error(e.to_string())),
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
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
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
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_agent_name_description(
    Path(agent_id): Path<String>,
    Json(req): Json<UpdateAgentNameDescriptionRequest>,
) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.update_agent_name_description(&agent_id, req.name, req.description).await
    };
    
    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn search_by_name(Json(req): Json<SearchRequest>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            let agents = ego_manager.search_by_name(&req.keyword).await;
            (StatusCode::OK, Json(ApiResponse::success(agents)))
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<Arc<AgentMetadata>>>::error(e.to_string()))),
    }
}

async fn search_by_description(Json(req): Json<SearchRequest>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            let agents = ego_manager.search_by_description(&req.keyword).await;
            (StatusCode::OK, Json(ApiResponse::success(agents)))
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<Arc<AgentMetadata>>>::error(e.to_string()))),
    }
}

async fn get_identity(Path(agent_id): Path<String>) -> impl IntoResponse {
    let result = {
        let ego_manager = EgoManager::new();
        ego_manager.get_identity_md(&agent_id).await
    };
    
    match result {
        Ok(content) => (StatusCode::OK, Json(ApiResponse::success(content))),
        Err(Error::AgentNotFound(_)) => (
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
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<String>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(Error::SettingNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<String>::error("user-recognition.md not found".to_string())),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        ),
    }
}
