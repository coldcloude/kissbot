use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use dashmap::{DashMap, DashSet};
use futures::future;
use kissbot_memory::DirectoryManager;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::agent::AgentManager;
use crate::ego_manager::EgoManager;
use crate::error::Error;
use crate::role_play_manager::{OtherRole as InternalOtherRole, RolePlay as InternalRolePlay, RolePlayManager, RoleRelation as InternalRoleRelation};
use crate::user_recognition_manager::{User as InternalUser, UserIdentifier as InternalUserIdentifier, UserPrivilege as InternalUserPrivilege, UserRecognition as InternalUserRecognition, UserRecognitionManager, UserRelation as InternalUserRelation};

use kissbot_api::*;

// ========== 转换函数 ==========

// AgentMetadata 转换
fn convert_agent_metadata(internal: &crate::agent::AgentMetadata) -> kissbot_api::ego::AgentMetadata {
    kissbot_api::ego::AgentMetadata {
        id: internal.id.as_ref().clone(),
        name: internal.name.as_ref().clone(),
        description: internal.description.as_ref().clone(),
        created_at: internal.created_at.as_ref().clone(),
    }
}

// UserPrivilege 转换
fn convert_user_privilege(internal: &InternalUserPrivilege) -> kissbot_api::ego::UserPrivilege {
    match internal {
        InternalUserPrivilege::Owner => kissbot_api::ego::UserPrivilege::Owner,
        InternalUserPrivilege::Admin => kissbot_api::ego::UserPrivilege::Admin,
        InternalUserPrivilege::Normal => kissbot_api::ego::UserPrivilege::Normal,
    }
}

// UserIdentifier 转换
fn convert_user_identifier(internal: &InternalUserIdentifier) -> kissbot_api::ego::UserIdentifier {
    kissbot_api::ego::UserIdentifier {
        channel_id: internal.channel_id.clone(),
        user_id: internal.user_id.clone(),
    }
}

// UserRelation 转换
fn convert_user_relation(internal: &InternalUserRelation) -> kissbot_api::ego::UserRelation {
    kissbot_api::ego::UserRelation {
        relation: internal.relation.as_ref().clone(),
        description: internal.description.as_ref().clone(),
    }
}

// User 转换
fn convert_user(internal: &InternalUser) -> kissbot_api::ego::User {
    let identifiers: Vec<_> = internal.identifiers.iter().map(|id| convert_user_identifier(&id)).collect();
    
    let mut relations = HashMap::new();
    for item in internal.relations.iter() {
        relations.insert(item.key().clone(), convert_user_relation(item.value()));
    }
    
    kissbot_api::ego::User {
        privilege: convert_user_privilege(&internal.privilege),
        identifiers,
        relations,
        description: internal.description.as_ref().clone(),
    }
}

// UserRecognition 转换
fn convert_user_recognition(internal: &InternalUserRecognition) -> kissbot_api::ego::UserRecognition {
    let mut user_map = HashMap::new();
    for item in internal.user_map.iter() {
        user_map.insert(item.key().clone(), convert_user(item.value()));
    }
    
    kissbot_api::ego::UserRecognition {
        id: internal.id.as_ref().clone(),
        user_map,
    }
}

// RoleRelation 转换
fn convert_role_relation(internal: &InternalRoleRelation) -> kissbot_api::ego::RoleRelation {
    kissbot_api::ego::RoleRelation {
        relation: internal.relation.as_ref().clone(),
        description: internal.description.as_ref().clone(),
    }
}

// OtherRole 转换
fn convert_other_role(internal: &InternalOtherRole) -> kissbot_api::ego::OtherRole {
    let mut other_role_relations = HashMap::new();
    for item in internal.other_role_relations.iter() {
        other_role_relations.insert(item.key().clone(), convert_role_relation(item.value()));
    }
    
    kissbot_api::ego::OtherRole {
        user_name: internal.user_name.as_ref().clone(),
        role_relation: convert_role_relation(&internal.role_relation),
        other_role_relations,
        description: internal.description.as_ref().clone(),
    }
}

// RolePlay 转换
fn convert_role_play(internal: &InternalRolePlay) -> kissbot_api::ego::RolePlay {
    let mut other_roles = HashMap::new();
    for item in internal.other_roles.iter() {
        other_roles.insert(item.key().clone(), convert_other_role(item.value()));
    }
    
    kissbot_api::ego::RolePlay {
        id: internal.id.as_ref().clone(),
        name: internal.name.as_ref().clone(),
        description: internal.description.as_ref().clone(),
        other_roles,
    }
}

// ========== 路由定义 ==========
pub fn create_router() -> Router {
    Router::new()
        // Agent 管理 API
        .route("/agent/create", post(create_agent))
        .route("/agent/list", get(list_agents))
        .route("/agent/get", post(get_agent))
        .route("/agent/update-name", put(update_agent_name))
        .route("/agent/update-description", put(update_agent_description))
        .route("/agent/copy", post(copy_agent))
        .route("/agent/search-name", post(search_by_name))
        .route("/agent/search-description", post(search_by_description))
        // 用户识别信息 API
        .route("/user/get-all", post(get_users))
        .route("/user/get", post(get_user))
        .route("/user/replace", put(replace_users))
        .route("/user/rename", put(rename_user))
        .route("/user/update-privilege", put(update_user_privilege))
        .route("/user/update-description", put(update_user_description))
        .route("/user/replace-identifiers", put(replace_user_identifiers))
        .route("/user/replace-relations", put(replace_user_relations))
        // 角色设定 API
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

// ========== Agent 管理 API ==========
async fn create_agent(Json(req): Json<ego::CreateAgentRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.create_agent(Arc::new(req.name), Arc::new(req.description)).await
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
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
            agents.push(convert_agent_metadata(&metadata));
        }
    }

    (StatusCode::OK, Json(ApiResponse::success(agents)))
}

async fn get_agent(Json(req): Json<ego::GetAgentRequest>) -> impl IntoResponse {
    match AgentManager::get().get_agent(&req.agent_id).await {
        Ok(agent) => (StatusCode::OK, Json(ApiResponse::success(convert_agent_metadata(&agent)))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<ego::AgentMetadata>::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<ego::AgentMetadata>::error(e.to_string()))),
    }
}

async fn update_agent_name(Json(req): Json<ego::UpdateAgentNameRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.update_agent_name(&req.agent_id, Arc::new(req.name)).await
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_agent_description(Json(req): Json<ego::UpdateAgentDescriptionRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.update_agent_description(&req.agent_id, Arc::new(req.description)).await
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn copy_agent(Json(req): Json<ego::CopyAgentRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.copy_agent(&req.agent_id).await
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn search_by_name(Json(req): Json<ego::SearchRequest>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            let agents = ego_manager.search_by_name(&req.keyword).await;
            let converted: Vec<_> = agents.iter().map(|m| convert_agent_metadata(m)).collect();
            (StatusCode::OK, Json(ApiResponse::success(converted)))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<ego::AgentMetadata>>::error(e.to_string()))),
    }
}

async fn search_by_description(Json(req): Json<ego::SearchRequest>) -> impl IntoResponse {
    match EgoManager::get().await {
        Ok(ego_manager) => {
            let agents = ego_manager.search_by_description(&req.keyword).await;
            let converted: Vec<_> = agents.iter().map(|m| convert_agent_metadata(m)).collect();
            (StatusCode::OK, Json(ApiResponse::success(converted)))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<ego::AgentMetadata>>::error(e.to_string()))),
    }
}

// ========== 用户识别信息 API ==========
async fn get_users(Json(req): Json<ego::GetUsersRequest>) -> impl IntoResponse {
    match UserRecognitionManager::get().get_users(&req.agent_id).await {
        Ok(users) => (StatusCode::OK, Json(ApiResponse::success(convert_user_recognition(&users)))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<ego::UserRecognition>::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<ego::UserRecognition>::error(e.to_string()))),
    }
}

async fn get_user(Json(req): Json<ego::GetUserRequest>) -> impl IntoResponse {
    match UserRecognitionManager::get().get_user(&req.agent_id, &req.user_name).await {
        Ok(user) => (StatusCode::OK, Json(ApiResponse::success(convert_user(&user)))),
        Err(Error::AgentUserNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<ego::User>::error(format!("User {} not found", req.user_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<ego::User>::error(e.to_string()))),
    }
}

async fn replace_users(Json(req): Json<ego::ReplaceUsersRequest>) -> impl IntoResponse {
    let remove_user_names: HashSet<String> = req.remove_user_names.into_iter().collect();

    let mut insert_users = HashMap::new();
    for (user_name, user_req) in req.insert_users {
        let relations = {
            let map = DashMap::new();
            for (other_user, rel_req) in user_req.relations {
                let relation = InternalUserRelation {
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
                set.insert(InternalUserIdentifier { channel_id: id.channel_id, user_id: id.user_id });
            }
            Arc::new(set)
        };

        let privilege = match user_req.privilege {
            ego::UserPrivilege::Owner => InternalUserPrivilege::Owner,
            ego::UserPrivilege::Admin => InternalUserPrivilege::Admin,
            ego::UserPrivilege::Normal => InternalUserPrivilege::Normal,
        };

        let user = InternalUser {
            privilege: Arc::new(privilege),
            identifiers,
            relations,
            description: Arc::new(user_req.description),
        };
        insert_users.insert(user_name, Arc::new(user));
    }

    let result = UserRecognitionManager::get().replace_users(&req.agent_id, remove_user_names, insert_users).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn rename_user(Json(req): Json<ego::RenameUserRequest>) -> impl IntoResponse {
    let result = UserRecognitionManager::get().rename_user(&req.agent_id, &req.user_name, &req.new_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("User {} not found", req.user_name)))),
        Err(Error::AgentUserAlreadyExists(_, _)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("User {} already exists", req.new_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_user_privilege(Json(req): Json<ego::UpdateUserPrivilegeRequest>) -> impl IntoResponse {
    let privilege = match req.privilege {
        ego::UserPrivilege::Owner => InternalUserPrivilege::Owner,
        ego::UserPrivilege::Admin => InternalUserPrivilege::Admin,
        ego::UserPrivilege::Normal => InternalUserPrivilege::Normal,
    };

    let result = UserRecognitionManager::get().update_user_privilege(&req.agent_id, &req.user_name, Arc::new(privilege)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("User {} not found", req.user_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_user_description(Json(req): Json<ego::UpdateUserDescriptionRequest>) -> impl IntoResponse {
    let result = UserRecognitionManager::get().update_user_description(&req.agent_id, &req.user_name, Arc::new(req.description)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("User {} not found", req.user_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn replace_user_identifiers(Json(req): Json<ego::ReplaceUserIdentifiersRequest>) -> impl IntoResponse {
    let remove_identifiers: HashSet<_> = req.remove_identifiers.into_iter().map(|id| InternalUserIdentifier { channel_id: id.channel_id, user_id: id.user_id }).collect();
    let insert_identifiers: HashSet<_> = req.insert_identifiers.into_iter().map(|id| InternalUserIdentifier { channel_id: id.channel_id, user_id: id.user_id }).collect();

    let result = UserRecognitionManager::get().replace_user_identifiers(&req.agent_id, &req.user_name, remove_identifiers, insert_identifiers).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("User {} not found", req.user_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn replace_user_relations(Json(req): Json<ego::ReplaceUserRelationsRequest>) -> impl IntoResponse {
    let remove_relations: HashSet<String> = req.remove_relations.into_iter().collect();
    let mut insert_relations = HashMap::new();

    for (other_user, rel_req) in req.insert_relations {
        let relation = InternalUserRelation {
            relation: Arc::new(rel_req.relation),
            description: Arc::new(rel_req.description),
        };
        insert_relations.insert(other_user, Arc::new(relation));
    }

    let result = UserRecognitionManager::get().replace_user_relations(&req.agent_id, &req.user_name, remove_relations, insert_relations).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentUserNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("User {} not found", req.user_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

// ========== 角色设定 API ==========
async fn list_roles(Json(req): Json<ego::ListRolesRequest>) -> impl IntoResponse {
    match RolePlayManager::get().list_roles(&req.agent_id).await {
        Ok(roles) => (StatusCode::OK, Json(ApiResponse::success(roles))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<Vec<String>>::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<String>>::error(e.to_string()))),
    }
}

async fn get_role(Json(req): Json<ego::GetRoleRequest>) -> impl IntoResponse {
    match RolePlayManager::get().get_role(&req.agent_id, &req.role_name).await {
        Ok(role) => (StatusCode::OK, Json(ApiResponse::success(convert_role_play(&role)))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<ego::RolePlay>::error(format!("Role {} not found", req.role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<ego::RolePlay>::error(e.to_string()))),
    }
}

async fn get_other_role(Json(req): Json<ego::GetOtherRoleRequest>) -> impl IntoResponse {
    match RolePlayManager::get().get_other_role(&req.agent_id, &req.role_name, &req.other_role_name).await {
        Ok(other_role) => (StatusCode::OK, Json(ApiResponse::success(convert_other_role(&other_role)))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<ego::OtherRole>::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<ego::OtherRole>::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<ego::OtherRole>::error(e.to_string()))),
    }
}

async fn create_role(Json(req): Json<ego::CreateRoleRequest>) -> impl IntoResponse {
    let name = req.name.clone();
    let result = RolePlayManager::get().create_role(&req.agent_id, Arc::new(req.name), Arc::new(req.description)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleAlreadyExists(_, _)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("Role {} already exists", name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn create_role_from(Json(req): Json<ego::CreateRoleFromRequest>) -> impl IntoResponse {
    let new_name = req.new_name.clone();
    let result = RolePlayManager::get().create_role_from(&req.agent_id, &req.role_name, Arc::new(req.new_name)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleAlreadyExists(_, _)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("Role {} already exists", new_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn remove_role(Json(req): Json<ego::RemoveRoleRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().remove_role(&req.agent_id, &req.role_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn rename_role(Json(req): Json<ego::RenameRoleRequest>) -> impl IntoResponse {
    let new_name = req.new_name.clone();
    let result = RolePlayManager::get().rename_role(&req.agent_id, &req.role_name, Arc::new(req.new_name)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleAlreadyExists(_, _)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("Role {} already exists", new_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_role_description(Json(req): Json<ego::UpdateRoleDescriptionRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_role_description(&req.agent_id, &req.role_name, Arc::new(req.description)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn replace_other_roles(Json(req): Json<ego::ReplaceOtherRolesRequest>) -> impl IntoResponse {
    let remove_other_roles: HashSet<String> = req.remove_other_roles.into_iter().collect();
    let mut insert_other_roles = HashMap::new();

    for (other_role_name, other_role_req) in req.insert_other_roles {
        let role_relation = InternalRoleRelation {
            relation: Arc::new(other_role_req.role_relation.relation),
            description: Arc::new(other_role_req.role_relation.description),
        };

        let other_role_relations = {
            let map = DashMap::new();
            for (rel_name, rel_req) in other_role_req.other_role_relations {
                let relation = InternalRoleRelation {
                    relation: Arc::new(rel_req.relation),
                    description: Arc::new(rel_req.description),
                };
                map.insert(rel_name, Arc::new(relation));
            }
            Arc::new(map)
        };

        let other_role = InternalOtherRole {
            user_name: Arc::new(other_role_req.user_name),
            role_relation: Arc::new(role_relation),
            other_role_relations,
            description: Arc::new(other_role_req.description),
        };

        insert_other_roles.insert(other_role_name, Arc::new(other_role));
    }

    let result = RolePlayManager::get().replace_other_roles(&req.agent_id, &req.role_name, remove_other_roles, insert_other_roles).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn rename_other_role(Json(req): Json<ego::RenameOtherRoleRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().rename_other_role(&req.agent_id, &req.role_name, &req.other_role_name, &req.new_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(Error::AgentRoleOtherRoleAlreadyExists(_, _, _)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("Other role {} already exists", req.new_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_other_role_user_name(Json(req): Json<ego::UpdateOtherRoleUserNameRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_other_role_user_name(&req.agent_id, &req.role_name, &req.other_role_name, Arc::new(req.new_user_name)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_other_role_description(Json(req): Json<ego::UpdateOtherRoleDescriptionRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_other_role_description(&req.agent_id, &req.role_name, &req.other_role_name, Arc::new(req.new_description)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_other_role_relation(Json(req): Json<ego::UpdateOtherRoleRelationRequest>) -> impl IntoResponse {
    let new_relation = InternalRoleRelation {
        relation: Arc::new(req.new_relation.relation),
        description: Arc::new(req.new_relation.description),
    };

    let result = RolePlayManager::get().update_other_role_relation(&req.agent_id, &req.role_name, &req.other_role_name, Arc::new(new_relation)).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn replace_other_role_relations(Json(req): Json<ego::ReplaceOtherRoleRelationsRequest>) -> impl IntoResponse {
    let remove_relations: HashSet<String> = req.remove_relations.into_iter().collect();
    let mut insert_relations = HashMap::new();

    for (rel_name, rel_req) in req.insert_relations {
        let relation = InternalRoleRelation {
            relation: Arc::new(rel_req.relation),
            description: Arc::new(rel_req.description),
        };
        insert_relations.insert(rel_name, Arc::new(relation));
    }

    let result = RolePlayManager::get().replace_other_role_relations(&req.agent_id, &req.role_name, &req.other_role_name, remove_relations, insert_relations).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
