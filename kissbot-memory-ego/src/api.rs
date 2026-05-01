use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use dashmap::{DashMap, DashSet};
use futures::future;
use kissbot_memory::DirectoryManager;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::agent::{AgentManager, AgentMetadata};
use crate::ego_manager::EgoManager;
use crate::error::Error;
use crate::role_play_manager::{
    OtherRole, RolePlay, RolePlayManager, RoleRelation,
};
use crate::user_recognition_manager::{
    User, UserIdentifier, UserPrivilege, UserRecognition, UserRecognitionManager, UserRelation,
};

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct GetAgentRequest {
    pub agent_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentNameRequest {
    pub agent_id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentDescriptionRequest {
    pub agent_id: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct CopyAgentRequest {
    pub agent_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub keyword: String,
}

// 用户识别信息请求结构体
#[derive(Debug, Deserialize)]
pub struct GetUsersRequest {
    pub agent_id: String,
}

#[derive(Debug, Deserialize)]
pub struct GetUserRequest {
    pub agent_id: String,
    pub user_name: String,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceUsersRequest {
    pub agent_id: String,
    pub remove_user_names: Vec<String>,
    pub insert_users: HashMap<String, UserRequest>,
}

#[derive(Debug, Deserialize)]
pub struct UserRequest {
    pub privilege: UserPrivilege,
    pub identifiers: Vec<UserIdentifier>,
    pub relations: HashMap<String, UserRelationRequest>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct UserRelationRequest {
    pub relation: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameUserRequest {
    pub agent_id: String,
    pub user_name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserPrivilegeRequest {
    pub agent_id: String,
    pub user_name: String,
    pub privilege: UserPrivilege,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserDescriptionRequest {
    pub agent_id: String,
    pub user_name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceUserIdentifiersRequest {
    pub agent_id: String,
    pub user_name: String,
    pub remove_identifiers: Vec<UserIdentifier>,
    pub insert_identifiers: Vec<UserIdentifier>,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceUserRelationsRequest {
    pub agent_id: String,
    pub user_name: String,
    pub remove_relations: Vec<String>,
    pub insert_relations: HashMap<String, UserRelationRequest>,
}

// 角色设定请求结构体
#[derive(Debug, Deserialize)]
pub struct ListRolesRequest {
    pub agent_id: String,
}

#[derive(Debug, Deserialize)]
pub struct GetRoleRequest {
    pub agent_id: String,
    pub role_name: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateRoleRequest {
    pub agent_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateRoleFromRequest {
    pub agent_id: String,
    pub role_name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveRoleRequest {
    pub agent_id: String,
    pub role_name: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameRoleRequest {
    pub agent_id: String,
    pub role_name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleDescriptionRequest {
    pub agent_id: String,
    pub role_name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct GetOtherRoleRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceOtherRolesRequest {
    pub agent_id: String,
    pub role_name: String,
    pub remove_other_roles: Vec<String>,
    pub insert_other_roles: HashMap<String, OtherRoleRequest>,
}

#[derive(Debug, Deserialize)]
pub struct OtherRoleRequest {
    pub user_name: String,
    pub role_relation: RoleRelationRequest,
    pub other_role_relations: HashMap<String, RoleRelationRequest>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct RoleRelationRequest {
    pub relation: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameOtherRoleRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateOtherRoleUserNameRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub new_user_name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateOtherRoleDescriptionRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub new_description: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateOtherRoleRelationRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub new_relation: RoleRelationRequest,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceOtherRoleRelationsRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub remove_relations: Vec<String>,
    pub insert_relations: HashMap<String, RoleRelationRequest>,
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
        // Agent管理API
        .route("/agent/create", post(create_agent))
        .route("/agent/list", get(list_agents))
        .route("/agent/get", post(get_agent))
        .route("/agent/update-name", put(update_agent_name))
        .route("/agent/update-description", put(update_agent_description))
        .route("/agent/copy", post(copy_agent))
        .route("/agent/search-name", post(search_by_name))
        .route("/agent/search-description", post(search_by_description))
        // 用户识别信息API
        .route("/user/get-all", post(get_users))
        .route("/user/get", post(get_user))
        .route("/user/replace", put(replace_users))
        .route("/user/rename", put(rename_user))
        .route("/user/update-privilege", put(update_user_privilege))
        .route("/user/update-description", put(update_user_description))
        .route("/user/replace-identifiers", put(replace_user_identifiers))
        .route("/user/replace-relations", put(replace_user_relations))
        // 角色设定API
        .route("/role/list", post(list_roles))
        .route("/role/get", post(get_role))
        .route("/role/create", post(create_role))
        .route("/role/create-from", post(create_role_from))
        .route("/role/remove", delete(remove_role))
        .route("/role/rename", put(rename_role))
        .route("/role/update-description", put(update_role_description))
        .route("/role/other/get", post(get_other_role))
        .route("/role/other/replace", put(replace_other_roles))
        .route("/role/other/rename", put(rename_other_role))
        .route("/role/other/update-user-name", put(update_other_role_user_name))
        .route("/role/other/update-description", put(update_other_role_description))
        .route("/role/other/update-relation", put(update_other_role_relation))
        .route("/role/other/replace-relations", put(replace_other_role_relations))
}

async fn create_agent(Json(req): Json<CreateAgentRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.create_agent(Arc::new(req.name), Arc::new(req.description)).await
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
    let agent_ids = {
        match DirectoryManager::get().list_agents().await {
            Ok(agent_ids) => agent_ids,
            Err(_) => Vec::new(),
        }
    };

    let agent_manager = AgentManager::get();
    let mut agents = Vec::new();
    let mut futs = Vec::new();
    agent_ids.iter().for_each(|agent_id| {
        futs.push(agent_manager.get_agent(agent_id));
    });
    let results = future::join_all(futs).await;
    for result in results {
        if let Ok(metadata) = result {
            agents.push(metadata);
        }
    }

    (StatusCode::OK, Json(ApiResponse::success(agents)))
}

async fn get_agent(Json(req): Json<GetAgentRequest>) -> impl IntoResponse {
    match AgentManager::get().get_agent(&req.agent_id).await {
        Ok(agent) => (StatusCode::OK, Json(ApiResponse::success(agent))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<AgentMetadata>>::error(format!("Agent {} not found", req.agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<AgentMetadata>>::error(e.to_string())),
        ),
    }
}

async fn update_agent_name(Json(req): Json<UpdateAgentNameRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.update_agent_name(&req.agent_id, Arc::new(req.name)).await
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Agent {} not found", req.agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_agent_description(Json(req): Json<UpdateAgentDescriptionRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.update_agent_description(&req.agent_id, Arc::new(req.description)).await
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Agent {} not found", req.agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn copy_agent(Json(req): Json<CopyAgentRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.copy_agent(&req.agent_id).await
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Agent {} not found", req.agent_id))),
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
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Vec<Arc<AgentMetadata>>>::error(e.to_string())),
            )
        }
    }
}

async fn search_by_description(Json(req): Json<SearchRequest>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            let agents = ego_manager.search_by_description(&req.keyword).await;
            (StatusCode::OK, Json(ApiResponse::success(agents)))
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Vec<Arc<AgentMetadata>>>::error(e.to_string())),
            )
        }
    }
}

// 用户识别信息API实现
async fn get_users(Json(req): Json<GetUsersRequest>) -> impl IntoResponse {
    match UserRecognitionManager::get().get_users(&req.agent_id).await {
        Ok(users) => (StatusCode::OK, Json(ApiResponse::success(users))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<UserRecognition>>::error(format!("Agent {} not found", req.agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<UserRecognition>>::error(e.to_string())),
        ),
    }
}

async fn get_user(Json(req): Json<GetUserRequest>) -> impl IntoResponse {
    match UserRecognitionManager::get().get_user(&req.agent_id, &req.user_name).await {
        Ok(user) => (StatusCode::OK, Json(ApiResponse::success(user))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<User>>::error(format!("User {} not found", req.user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<User>>::error(e.to_string())),
        ),
    }
}

async fn replace_users(Json(req): Json<ReplaceUsersRequest>) -> impl IntoResponse {
    let remove_user_names: HashSet<String> = req.remove_user_names.into_iter().collect();

    let mut insert_users = HashMap::new();
    for (user_name, user_req) in req.insert_users {
        let relations = {
            let map = DashMap::new();
            for (other_user, rel_req) in user_req.relations {
                let relation = UserRelation {
                    relation: Arc::new(rel_req.relation),
                    description: Arc::new(rel_req.description),
                };
                map.insert(other_user, Arc::new(relation));
            }
            Arc::new(map)
        };

        let identifiers = {
            let set = DashSet::new();
            for id in user_req.identifiers {
                set.insert(id);
            }
            Arc::new(set)
        };

        let user = User {
            privilege: Arc::new(user_req.privilege),
            identifiers,
            relations,
            description: Arc::new(user_req.description),
        };
        insert_users.insert(user_name, Arc::new(user));
    }

    let result = UserRecognitionManager::get()
        .replace_users(&req.agent_id, remove_user_names, insert_users)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Agent {} not found", req.agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn rename_user(Json(req): Json<RenameUserRequest>) -> impl IntoResponse {
    let result = UserRecognitionManager::get()
        .rename_user(&req.agent_id, &req.user_name, &req.new_name)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", req.user_name))),
        ),
        Err(Error::AgentUserAlreadyExists(_, _)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::<()>::error(format!("User {} already exists", req.new_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_user_privilege(Json(req): Json<UpdateUserPrivilegeRequest>) -> impl IntoResponse {
    let result = UserRecognitionManager::get()
        .update_user_privilege(&req.agent_id, &req.user_name, Arc::new(req.privilege))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", req.user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_user_description(Json(req): Json<UpdateUserDescriptionRequest>) -> impl IntoResponse {
    let result = UserRecognitionManager::get()
        .update_user_description(&req.agent_id, &req.user_name, Arc::new(req.description))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", req.user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn replace_user_identifiers(Json(req): Json<ReplaceUserIdentifiersRequest>) -> impl IntoResponse {
    let remove_identifiers: HashSet<UserIdentifier> = req.remove_identifiers.into_iter().collect();
    let insert_identifiers: HashSet<UserIdentifier> = req.insert_identifiers.into_iter().collect();

    let result = UserRecognitionManager::get()
        .replace_user_identifiers(&req.agent_id, &req.user_name, remove_identifiers, insert_identifiers)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", req.user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn replace_user_relations(Json(req): Json<ReplaceUserRelationsRequest>) -> impl IntoResponse {
    let remove_relations: HashSet<String> = req.remove_relations.into_iter().collect();
    let mut insert_relations = HashMap::new();

    for (other_user, rel_req) in req.insert_relations {
        let relation = UserRelation {
            relation: Arc::new(rel_req.relation),
            description: Arc::new(rel_req.description),
        };
        insert_relations.insert(other_user, Arc::new(relation));
    }

    let result = UserRecognitionManager::get()
        .replace_user_relations(&req.agent_id, &req.user_name, remove_relations, insert_relations)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", req.user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

// 角色设定API实现
async fn list_roles(Json(req): Json<ListRolesRequest>) -> impl IntoResponse {
    match RolePlayManager::get().list_roles(&req.agent_id).await {
        Ok(roles) => (StatusCode::OK, Json(ApiResponse::success(roles))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Vec<String>>::error(format!("Agent {} not found", req.agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<String>>::error(e.to_string())),
        ),
    }
}

async fn get_role(Json(req): Json<GetRoleRequest>) -> impl IntoResponse {
    match RolePlayManager::get().get_role(&req.agent_id, &req.role_name).await {
        Ok(role) => (StatusCode::OK, Json(ApiResponse::success(role))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<RolePlay>>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<RolePlay>>::error(e.to_string())),
        ),
    }
}

async fn get_other_role(Json(req): Json<GetOtherRoleRequest>) -> impl IntoResponse {
    match RolePlayManager::get().get_other_role(&req.agent_id, &req.role_name, &req.other_role_name).await {
        Ok(other_role) => (StatusCode::OK, Json(ApiResponse::success(other_role))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<OtherRole>>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<OtherRole>>::error(format!("Other role {} not found", req.other_role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<OtherRole>>::error(e.to_string())),
        ),
    }
}

async fn create_role(Json(req): Json<CreateRoleRequest>) -> impl IntoResponse {
    let name = req.name.clone();
    let result = RolePlayManager::get()
        .create_role(&req.agent_id, Arc::new(req.name), Arc::new(req.description))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleAlreadyExists(_, _)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::<()>::error(format!("Role {} already exists", name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn create_role_from(Json(req): Json<CreateRoleFromRequest>) -> impl IntoResponse {
    let new_name = req.new_name.clone();
    let result = RolePlayManager::get()
        .create_role_from(&req.agent_id, &req.role_name, Arc::new(req.new_name))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(Error::AgentRoleAlreadyExists(_, _)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::<()>::error(format!("Role {} already exists", new_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn remove_role(Json(req): Json<RemoveRoleRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().remove_role(&req.agent_id, &req.role_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn rename_role(Json(req): Json<RenameRoleRequest>) -> impl IntoResponse {
    let new_name = req.new_name.clone();
    let result = RolePlayManager::get()
        .rename_role(&req.agent_id, &req.role_name, Arc::new(req.new_name))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(Error::AgentRoleAlreadyExists(_, _)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::<()>::error(format!("Role {} already exists", new_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_role_description(Json(req): Json<UpdateRoleDescriptionRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get()
        .update_role_description(&req.agent_id, &req.role_name, Arc::new(req.description))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn replace_other_roles(Json(req): Json<ReplaceOtherRolesRequest>) -> impl IntoResponse {
    let remove_other_roles: HashSet<String> = req.remove_other_roles.into_iter().collect();
    let mut insert_other_roles = HashMap::new();

    for (other_role_name, other_role_req) in req.insert_other_roles {
        let role_relation = RoleRelation {
            relation: Arc::new(other_role_req.role_relation.relation),
            description: Arc::new(other_role_req.role_relation.description),
        };

        let other_role_relations = {
            let map = DashMap::new();
            for (rel_name, rel_req) in other_role_req.other_role_relations {
                let relation = RoleRelation {
                    relation: Arc::new(rel_req.relation),
                    description: Arc::new(rel_req.description),
                };
                map.insert(rel_name, Arc::new(relation));
            }
            Arc::new(map)
        };

        let other_role = OtherRole {
            user_name: Arc::new(other_role_req.user_name),
            role_relation: Arc::new(role_relation),
            other_role_relations,
            description: Arc::new(other_role_req.description),
        };

        insert_other_roles.insert(other_role_name, Arc::new(other_role));
    }

    let result = RolePlayManager::get()
        .replace_other_roles(&req.agent_id, &req.role_name, remove_other_roles, insert_other_roles)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn rename_other_role(Json(req): Json<RenameOtherRoleRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get()
        .rename_other_role(&req.agent_id, &req.role_name, &req.other_role_name, &req.new_name)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Other role {} not found", req.other_role_name))),
        ),
        Err(Error::AgentRoleOtherRoleAlreadyExists(_, _, _)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::<()>::error(format!("Other role {} already exists", req.new_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_other_role_user_name(Json(req): Json<UpdateOtherRoleUserNameRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get()
        .update_other_role_user_name(&req.agent_id, &req.role_name, &req.other_role_name, Arc::new(req.new_user_name))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Other role {} not found", req.other_role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_other_role_description(Json(req): Json<UpdateOtherRoleDescriptionRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get()
        .update_other_role_description(&req.agent_id, &req.role_name, &req.other_role_name, Arc::new(req.new_description))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Other role {} not found", req.other_role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_other_role_relation(Json(req): Json<UpdateOtherRoleRelationRequest>) -> impl IntoResponse {
    let new_relation = RoleRelation {
        relation: Arc::new(req.new_relation.relation),
        description: Arc::new(req.new_relation.description),
    };

    let result = RolePlayManager::get()
        .update_other_role_relation(&req.agent_id, &req.role_name, &req.other_role_name, Arc::new(new_relation))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Other role {} not found", req.other_role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn replace_other_role_relations(Json(req): Json<ReplaceOtherRoleRelationsRequest>) -> impl IntoResponse {
    let remove_relations: HashSet<String> = req.remove_relations.into_iter().collect();
    let mut insert_relations = HashMap::new();

    for (rel_name, rel_req) in req.insert_relations {
        let relation = RoleRelation {
            relation: Arc::new(rel_req.relation),
            description: Arc::new(rel_req.description),
        };
        insert_relations.insert(rel_name, Arc::new(relation));
    }

    let result = RolePlayManager::get()
        .replace_other_role_relations(&req.agent_id, &req.role_name, &req.other_role_name, remove_relations, insert_relations)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", req.role_name))),
        ),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Other role {} not found", req.other_role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}
