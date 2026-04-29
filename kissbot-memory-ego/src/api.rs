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
use crate::user_recognition_manager::{UserRecognition, UserIdentity, UserRelation, UserRecognitionManager};
use crate::role_play_manager::{RolePlay, RolePlayManager};
use crate::role_play_relation_manager::{RoleRelation, RolePlayRelation, RolePlayRelationManager};
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

// 用户识别信息请求
#[derive(Debug, Deserialize)]
pub struct AddUserRequest {
    pub name: String,
    pub identity: String,
    pub associated_identifiers: Vec<String>,
    pub relations: Vec<UserRelation>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub name: String,
    pub identity: String,
    pub associated_identifiers: Vec<String>,
    pub relations: Vec<UserRelation>,
    pub description: Option<String>,
}

// 角色扮演请求
#[derive(Debug, Deserialize)]
pub struct AddRoleRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleRequest {
    pub name: String,
    pub description: Option<String>,
}

// 角色扮演关系请求
#[derive(Debug, Deserialize)]
pub struct AddRoleRelationRequest {
    pub name: String,
    pub associated_user_name: Option<String>,
    pub relation_with_agent_role: String,
    pub relations_with_other_roles: Vec<RoleRelation>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleRelationRequest {
    pub name: String,
    pub associated_user_name: Option<String>,
    pub relation_with_agent_role: String,
    pub relations_with_other_roles: Vec<RoleRelation>,
    pub description: Option<String>,
}

// 辅助函数：将字符串转换为 UserIdentity
fn parse_identity(identity_str: &str) -> Result<UserIdentity, String> {
    match identity_str.to_lowercase().as_str() {
        "owner" => Ok(UserIdentity::Owner),
        "administrator" => Ok(UserIdentity::Administrator),
        "other" => Ok(UserIdentity::Other),
        _ => Err(format!("Invalid identity: {}", identity_str)),
    }
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
        // 用户识别信息路由
        .route("/agents/:agent_id/user-recognition/json", get(get_user_recognition_json))
        .route("/agents/:agent_id/user-recognition/md", get(get_user_recognition_md))
        .route("/agents/:agent_id/user-recognition", post(add_user))
        .route("/agents/:agent_id/user-recognition/:user_name", put(update_user))
        // 角色扮演路由
        .route("/agents/:agent_id/roles", get(list_roles))
        .route("/agents/:agent_id/roles/:role_id/json", get(get_role_json))
        .route("/agents/:agent_id/roles/:role_id/md", get(get_role_md))
        .route("/agents/:agent_id/roles", post(add_role))
        .route("/agents/:agent_id/roles/:role_id", put(update_role))
        // 角色扮演关系路由
        .route("/agents/:agent_id/role-relations", get(list_role_relations))
        .route("/agents/:agent_id/role-relations/:relation_id/json", get(get_role_relation_json))
        .route("/agents/:agent_id/role-relations/:relation_id/md", get(get_role_relation_md))
        .route("/agents/:agent_id/role-relations", post(add_role_relation))
        .route("/agents/:agent_id/role-relations/:relation_id", put(update_role_relation))
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
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<AgentMetadata>>::error(e.to_string()))),
    }
}

async fn search_by_description(Json(req): Json<SearchRequest>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            let agents = ego_manager.search_by_description(&req.keyword).await;
            (StatusCode::OK, Json(ApiResponse::success(agents)))
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<AgentMetadata>>::error(e.to_string()))),
    }
}

async fn get_identity(Path(agent_id): Path<String>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            match ego_manager.get_identity_md(&agent_id).await {
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
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        ),
    }
}

// 用户识别信息处理函数
async fn get_user_recognition_json(Path(agent_id): Path<String>) -> impl IntoResponse {
    match UserRecognitionManager::get().get_users(&agent_id).await {
        Ok(users) => (StatusCode::OK, Json(ApiResponse::success(users))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Vec<UserRecognition>>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<UserRecognition>>::error(e.to_string())),
        ),
    }
}

async fn get_user_recognition_md(Path(agent_id): Path<String>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            match ego_manager.get_user_recognition_md(&agent_id).await {
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
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        ),
    }
}

async fn add_user(
    Path(agent_id): Path<String>,
    Json(req): Json<AddUserRequest>,
) -> impl IntoResponse {
    let identity = match parse_identity(&req.identity) {
        Ok(i) => i,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(e))),
    };
    
    let user = UserRecognition {
        name: req.name,
        identity,
        associated_identifiers: req.associated_identifiers,
        relations: req.relations,
        description: req.description,
    };
    
    match UserRecognitionManager::get().add_user(&agent_id, user).await {
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

async fn update_user(
    Path((agent_id, user_name)): Path<(String, String)>,
    Json(req): Json<UpdateUserRequest>,
) -> impl IntoResponse {
    let identity = match parse_identity(&req.identity) {
        Ok(i) => i,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(e))),
    };
    
    let user = UserRecognition {
        name: req.name,
        identity,
        associated_identifiers: req.associated_identifiers,
        relations: req.relations,
        description: req.description,
    };
    
    match UserRecognitionManager::get().update_user(&agent_id, &user_name, user).await {
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

// 角色扮演处理函数
async fn list_roles(Path(agent_id): Path<String>) -> impl IntoResponse {
    match RolePlayManager::get().get_roles(&agent_id).await {
        Ok(roles) => (StatusCode::OK, Json(ApiResponse::success(roles))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Vec<RolePlay>>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<RolePlay>>::error(e.to_string())),
        ),
    }
}

async fn get_role_json(Path((agent_id, role_id)): Path<(String, String)>) -> impl IntoResponse {
    match RolePlayManager::get().get_role(&agent_id, &role_id).await {
        Ok(role) => (StatusCode::OK, Json(ApiResponse::success(role))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<RolePlay>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(Error::SettingNotFound(msg)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<RolePlay>::error(msg)),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<RolePlay>::error(e.to_string())),
        ),
    }
}

async fn get_role_md(Path((agent_id, role_id)): Path<(String, String)>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            match ego_manager.get_role_play_md(&agent_id, &role_id).await {
                Ok(content) => (StatusCode::OK, Json(ApiResponse::success(content))),
                Err(Error::AgentNotFound(_)) => (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<String>::error(format!("Agent {} not found", agent_id))),
                ),
                Err(Error::SettingNotFound(msg)) => (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<String>::error(msg)),
                ),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<String>::error(e.to_string())),
                ),
            }
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        ),
    }
}

async fn add_role(
    Path(agent_id): Path<String>,
    Json(req): Json<AddRoleRequest>,
) -> impl IntoResponse {
    let role = RolePlay {
        id: String::new(),
        name: req.name,
        description: req.description,
    };
    
    match RolePlayManager::get().add_role(&agent_id, role).await {
        Ok(role_id) => (StatusCode::OK, Json(ApiResponse::success(role_id))),
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

async fn update_role(
    Path((agent_id, role_id)): Path<(String, String)>,
    Json(req): Json<UpdateRoleRequest>,
) -> impl IntoResponse {
    let role = RolePlay {
        id: role_id.clone(),
        name: req.name,
        description: req.description,
    };
    
    match RolePlayManager::get().update_role(&agent_id, &role_id, role).await {
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

// 角色扮演关系处理函数
async fn list_role_relations(Path(agent_id): Path<String>) -> impl IntoResponse {
    match RolePlayRelationManager::get().get_relations(&agent_id).await {
        Ok(relations) => (StatusCode::OK, Json(ApiResponse::success(relations))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Vec<RolePlayRelation>>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<RolePlayRelation>>::error(e.to_string())),
        ),
    }
}

async fn get_role_relation_json(Path((agent_id, relation_id)): Path<(String, String)>) -> impl IntoResponse {
    match RolePlayRelationManager::get().get_relation(&agent_id, &relation_id).await {
        Ok(relation) => (StatusCode::OK, Json(ApiResponse::success(relation))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<RolePlayRelation>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(Error::SettingNotFound(msg)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<RolePlayRelation>::error(msg)),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<RolePlayRelation>::error(e.to_string())),
        ),
    }
}

async fn get_role_relation_md(Path((agent_id, relation_id)): Path<(String, String)>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            match ego_manager.get_role_play_relation_md(&agent_id, &relation_id).await {
                Ok(content) => (StatusCode::OK, Json(ApiResponse::success(content))),
                Err(Error::AgentNotFound(_)) => (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<String>::error(format!("Agent {} not found", agent_id))),
                ),
                Err(Error::SettingNotFound(msg)) => (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<String>::error(msg)),
                ),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<String>::error(e.to_string())),
                ),
            }
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        ),
    }
}

async fn add_role_relation(
    Path(agent_id): Path<String>,
    Json(req): Json<AddRoleRelationRequest>,
) -> impl IntoResponse {
    let relation = RolePlayRelation {
        id: String::new(),
        name: req.name,
        associated_user_name: req.associated_user_name,
        relation_with_agent_role: req.relation_with_agent_role,
        relations_with_other_roles: req.relations_with_other_roles,
        description: req.description,
    };
    
    match RolePlayRelationManager::get().add_relation(&agent_id, relation).await {
        Ok(relation_id) => (StatusCode::OK, Json(ApiResponse::success(relation_id))),
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

async fn update_role_relation(
    Path((agent_id, relation_id)): Path<(String, String)>,
    Json(req): Json<UpdateRoleRelationRequest>,
) -> impl IntoResponse {
    let relation = RolePlayRelation {
        id: relation_id.clone(),
        name: req.name,
        associated_user_name: req.associated_user_name,
        relation_with_agent_role: req.relation_with_agent_role,
        relations_with_other_roles: req.relations_with_other_roles,
        description: req.description,
    };
    
    match RolePlayRelationManager::get().update_relation(&agent_id, &relation_id, relation).await {
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
