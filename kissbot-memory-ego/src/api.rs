use axum::{
    extract::Path,
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
use crate::role_play_manager::{RoleRelation, RolePlay, RolePlayManager, OtherRole};
use crate::user_recognition_manager::{
    UserRelation, User, UserIdentifier, UserPrivilege, UserRecognition, UserRecognitionManager,
};

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
pub struct SearchRequest {
    pub keyword: String,
}

// 用户识别模块请求结构体
#[derive(Debug, Deserialize)]
pub struct ReplaceUsersRequest {
    pub remove_user_names: Vec<String>,
    pub insert_users: HashMap<String, UserRequest>,
}

#[derive(Debug, Deserialize)]
pub struct UserRequest {
    pub privilege: UserPrivilege,
    pub identifiers: Vec<UserIdentifier>,
    pub relations: HashMap<String, OtherUserRelationRequest>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct OtherUserRelationRequest {
    pub relation: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserPrivilegeRequest {
    pub privilege: UserPrivilege,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserDescriptionRequest {
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserIdentifiersRequest {
    pub remove_identifiers: Vec<UserIdentifier>,
    pub insert_identifiers: Vec<UserIdentifier>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRelationsRequest {
    pub remove_relations: Vec<String>,
    pub insert_relations: HashMap<String, OtherUserRelationRequest>,
}

// 角色设定模块请求结构体
#[derive(Debug, Deserialize)]
pub struct CreateRoleRequest {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameRoleRequest {
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleDescriptionRequest {
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleRelationsRequest {
    pub remove_relations: Vec<String>,
    pub insert_relations: HashMap<String, RolePlayRelationRequest>,
}

#[derive(Debug, Deserialize)]
pub struct RolePlayRelationRequest {
    pub user_name: String,
    pub role_relation: OtherRoleRelationRequest,
    pub other_role_relations: HashMap<String, OtherRoleRelationRequest>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct OtherRoleRelationRequest {
    pub relation: String,
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
        // Agent管理API
        .route(
            "/agent",
            post(create_agent)
        )
        .route(
            "/agent",
            get(list_agents)
        )
        .route(
            "/agent/:agent_id",
            get(get_agent)
        )
        .route(
            "/agent/:agent_id/name",
            put(update_agent_name),
        )
        .route(
            "/agent/:agent_id/description",
            put(update_agent_description),
        )
        .route(
            "/agent/search/name",
            post(search_by_name),
        )
        .route(
            "/agent/search/description",
            post(search_by_description),
        )
        // 用户识别信息API
        .route(
            "/agent/:agent_id/users",
            get(get_users),
        )
        .route(
            "/agent/:agent_id/users/:user_name",
            get(get_user),
        )
        .route(
            "/agent/:agent_id/users",
            put(replace_users),
        )
        .route(
            "/agent/:agent_id/users/:user_name/privilege",
            put(update_user_privilege),
        )
        .route(
            "/agent/:agent_id/users/:user_name/description",
            put(update_user_description),
        )
        .route(
            "/agent/:agent_id/users/:user_name/identifiers",
            put(update_user_identifiers),
        )
        .route(
            "/agent/:agent_id/users/:user_name/relations",
            put(update_user_relations),
        )
        // 角色设定API
        .route(
            "/agent/:agent_id/roles/:role_name",
            get(get_role),
        )
        .route(
            "/agent/:agent_id/roles",
            post(create_role),
        )
        .route(
            "/agent/:agent_id/roles/:role_name",
            delete(remove_role),
        )
        .route(
            "/agent/:agent_id/roles/:role_name/name",
            put(rename_role),
        )
        .route(
            "/agent/:agent_id/roles/:role_name/create/:new_name",
            post(create_role_from),
        )
        .route(
            "/agent/:agent_id/roles/:role_name/description",
            put(update_role_description),
        )
        .route(
            "/agent/:agent_id/roles/:role_name/other_roles",
            put(replace_other_roles),
        )
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
        futs.push(agent_manager.get_agent(&agent_id));
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
    match AgentManager::get().get_agent(&agent_id).await {
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
        agent_manager.update_agent_name(&agent_id, Arc::new(req.name)).await
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
        agent_manager.update_agent_description(&agent_id, Arc::new(req.description)).await
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
async fn get_users(Path(agent_id): Path<String>) -> impl IntoResponse {
    match UserRecognitionManager::get().get_users(&agent_id).await {
        Ok(users) => (StatusCode::OK, Json(ApiResponse::success(users))),
        Err(Error::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<UserRecognition>>::error(format!("Agent {} not found", agent_id))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<UserRecognition>>::error(e.to_string())),
        ),
    }
}

async fn get_user(Path((agent_id, user_name)): Path<(String, String)>) -> impl IntoResponse {
    match UserRecognitionManager::get().get_user(&agent_id, &user_name).await {
        Ok(user) => (StatusCode::OK, Json(ApiResponse::success(user))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<User>>::error(format!("User {} not found", user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<User>>::error(e.to_string())),
        ),
    }
}

async fn replace_users(
    Path(agent_id): Path<String>,
    Json(req): Json<ReplaceUsersRequest>,
) -> impl IntoResponse {
    let remove_user_name_set: HashSet<String> = req.remove_user_names.into_iter().collect();

    let mut insert_user_map = HashMap::new();
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
        insert_user_map.insert(user_name, Arc::new(user));
    }

    let result = UserRecognitionManager::get()
        .replace_users(&agent_id, remove_user_name_set, insert_user_map)
        .await;

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

async fn update_user_privilege(
    Path((agent_id, user_name)): Path<(String, String)>,
    Json(req): Json<UpdateUserPrivilegeRequest>,
) -> impl IntoResponse {
    let result = UserRecognitionManager::get()
        .update_user_privilege(&agent_id, &user_name, Arc::new(req.privilege))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_user_description(
    Path((agent_id, user_name)): Path<(String, String)>,
    Json(req): Json<UpdateUserDescriptionRequest>,
) -> impl IntoResponse {
    let result = UserRecognitionManager::get()
        .update_user_description(&agent_id, &user_name, Arc::new(req.description))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_user_identifiers(
    Path((agent_id, user_name)): Path<(String, String)>,
    Json(req): Json<UpdateUserIdentifiersRequest>,
) -> impl IntoResponse {
    let remove_identifiers: HashSet<UserIdentifier> = req.remove_identifiers.into_iter().collect();
    let insert_identifiers: HashSet<UserIdentifier> = req.insert_identifiers.into_iter().collect();

    let result = UserRecognitionManager::get()
        .replace_user_identifiers(&agent_id, &user_name, remove_identifiers, insert_identifiers)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_user_relations(
    Path((agent_id, user_name)): Path<(String, String)>,
    Json(req): Json<UpdateUserRelationsRequest>,
) -> impl IntoResponse {
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
        .replace_user_relations(&agent_id, &user_name, remove_relations, insert_relations)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("User {} not found", user_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

// 角色设定API实现
async fn get_role(Path((agent_id, role_name)): Path<(String, String)>) -> impl IntoResponse {
    match RolePlayManager::get().get_role(&agent_id, &role_name).await {
        Ok(role) => (StatusCode::OK, Json(ApiResponse::success(role))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<Arc<RolePlay>>::error(format!("Role {} not found", role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Arc<RolePlay>>::error(e.to_string())),
        ),
    }
}

async fn create_role(
    Path(agent_id): Path<String>,
    Json(req): Json<CreateRoleRequest>,
) -> impl IntoResponse {
    let result = RolePlayManager::get()
        .create_role(&agent_id, Arc::new(req.name), Arc::new(req.description))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn remove_role(Path((agent_id, role_name)): Path<(String, String)>) -> impl IntoResponse {
    let result = RolePlayManager::get().remove_role(&agent_id, &role_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn rename_role(
    Path((agent_id, role_name)): Path<(String, String)>,
    Json(req): Json<RenameRoleRequest>,
) -> impl IntoResponse {
    let result = RolePlayManager::get()
        .rename_role(&agent_id, &role_name, Arc::new(req.new_name))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn create_role_from(Path((agent_id, role_name, new_name)): Path<(String, String, String)>) -> impl IntoResponse {
    let result = RolePlayManager::get()
        .create_role_from(&agent_id, &role_name, Arc::new(new_name))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn update_role_description(
    Path((agent_id, role_name)): Path<(String, String)>,
    Json(req): Json<UpdateRoleDescriptionRequest>,
) -> impl IntoResponse {
    let result = RolePlayManager::get()
        .update_role_description(&agent_id, &role_name, Arc::new(req.description))
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}

async fn replace_other_roles(
    Path((agent_id, role_name)): Path<(String, String)>,
    Json(req): Json<UpdateRoleRelationsRequest>,
) -> impl IntoResponse {
    let remove_relations: HashSet<String> = req.remove_relations.into_iter().collect();
    let mut insert_relations = HashMap::new();

    for (identifier, rel_req) in req.insert_relations {
        let role_relation = RoleRelation {
            relation: Arc::new(rel_req.role_relation.relation),
            description: Arc::new(rel_req.role_relation.description),
        };

        let other_role_relations = {
            let map = DashMap::new();
            for (other_role, other_rel) in rel_req.other_role_relations {
                let relation = RoleRelation {
                    relation: Arc::new(other_rel.relation),
                    description: Arc::new(other_rel.description),
                };
                map.insert(other_role, Arc::new(relation));
            }
            Arc::new(map)
        };

        let relation = OtherRole {
            user_name: Arc::new(rel_req.user_name),
            role_relation: Arc::new(role_relation),
            other_role_relations,
            description: Arc::new(rel_req.description),
        };

        insert_relations.insert(identifier, Arc::new(relation));
    }

    let result = RolePlayManager::get()
        .replace_other_roles(&agent_id, &role_name, remove_relations, insert_relations)
        .await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Role {} not found", role_name))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ),
    }
}
