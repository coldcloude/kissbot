use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use futures::future;
use kissbot_memory::DirectoryManager;
use std::sync::Arc;

use crate::agent::AgentManager;
use kissbot_api::AgentMetadata;
use crate::search::SearchManager;
use crate::error::Error;
use crate::role_play::RolePlayManager;
use crate::individual_recognition::IndividualRecognitionManager;

use kissbot_api::*;

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
        .route("/agent/retrieve", post(retrieve_agents))
        .route("/agent/name-completion", post(agent_name_completion))
        // 个体识别信息 API
        .route("/individual/get-all", post(get_individuals))
        .route("/individual/get", post(get_individual))
        .route("/individual/replace", put(replace_individuals))
        .route("/individual/rename", put(rename_individual))
        .route("/individual/replace-identifiers", put(replace_individual_identifiers))
        .route("/individual/replace-relations", put(replace_individual_relations))
        // 角色设定 API
        .route("/role/list", post(list_roles))
        .route("/role/get", post(get_role))
        .route("/role/create", post(create_role))
        .route("/role/create-from", post(create_role_from))
        .route("/role/remove", delete(remove_role))
        .route("/role/rename", put(rename_role))
        .route("/role/update-description", put(update_role_description))
        .route("/role/update-full-name", put(update_role_full_name))
        .route("/role/search-name", post(search_role_by_name))
        .route("/role/search-description", post(search_role_by_description))
        .route("/role/retrieve", post(retrieve_roles))
        .route("/role/name-completion", post(role_name_completion))
        .route("/role/other/get", post(get_other_role))
        .route("/role/other/replace", put(replace_other_roles))
        .route("/role/other/rename", put(rename_other_role))
        .route("/role/other/update-individual-name", put(update_other_role_individual_name))
        .route("/role/other/update-description", put(update_other_role_description))
        .route("/role/other/update-relation", put(update_other_role_relation))
        .route("/role/other/replace-relations", put(replace_other_role_relations))
}

// ========== Agent 管理 API ==========
async fn create_agent(Json(req): Json<ego::CreateAgentRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.create_agent(req.individual_name, req.description).await
    };

    match result {
        Ok(agent_id) => (StatusCode::OK, Json(ApiResponse::success(agent_id))),
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
            agents.push(metadata);
        }
    }

    (StatusCode::OK, Json(ApiResponse::success(agents)))
}

async fn get_agent(Json(req): Json<ego::GetAgentRequest>) -> impl IntoResponse {
    match AgentManager::get().get_agent(&req.agent_id).await {
        Ok(agent) => (StatusCode::OK, Json(ApiResponse::success(agent))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<Arc<AgentMetadata>>::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Arc<AgentMetadata>>::error(e.to_string()))),
    }
}

async fn update_agent_name(Json(req): Json<ego::UpdateAgentNameRequest>) -> impl IntoResponse {
    let result = {
        let agent_manager = AgentManager::get();
        agent_manager.update_agent_name(&req.agent_id, req.individual_name).await
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
        agent_manager.update_agent_description(&req.agent_id, req.description).await
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
        Ok(agent_id) => (StatusCode::OK, Json(ApiResponse::success(agent_id))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn search_by_name(Json(req): Json<ego::SearchRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let agents = ego_manager.search_by_name(&req.keyword).await;
    (StatusCode::OK, Json(ApiResponse::success(agents)))
}

async fn search_by_description(Json(req): Json<ego::SearchRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let agents = ego_manager.search_by_description(&req.keyword).await;
    (StatusCode::OK, Json(ApiResponse::success(agents)))
}

async fn retrieve_agents(Json(req): Json<ego::RetrieveAgentsRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let agents = ego_manager.retrieve_agents(req.agent_ids).await;
    (StatusCode::OK, Json(ApiResponse::success(agents)))
}

async fn agent_name_completion(Json(req): Json<ego::NameCompletionRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let results = ego_manager.name_completion(&req.prefix).await;
    (StatusCode::OK, Json(ApiResponse::success(results)))
}

async fn search_role_by_name(Json(req): Json<ego::SearchRoleRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let roles = ego_manager.search_role_by_name(&req.keyword, req.agent_id.as_deref().map(|s| s.as_str())).await;
    (StatusCode::OK, Json(ApiResponse::success(roles)))
}

async fn search_role_by_description(Json(req): Json<ego::SearchRoleRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let roles = ego_manager.search_role_by_description(&req.keyword, req.agent_id.as_deref().map(|s| s.as_str())).await;
    (StatusCode::OK, Json(ApiResponse::success(roles)))
}

async fn retrieve_roles(Json(req): Json<ego::RetrieveRolesRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let roles = ego_manager.retrieve_roles(req.role_keys).await;
    (StatusCode::OK, Json(ApiResponse::success(roles)))
}

async fn role_name_completion(Json(req): Json<ego::RoleNameCompletionRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let results = ego_manager.role_name_completion(&req.prefix, req.agent_id.as_deref().map(|s| s.as_str())).await;
    (StatusCode::OK, Json(ApiResponse::success(results)))
}

// ========== 个体识别信息 API ==========
async fn get_individuals(Json(req): Json<ego::GetIndividualsRequest>) -> impl IntoResponse {
    match IndividualRecognitionManager::get().get_individuals(&req.agent_id).await {
        Ok(individuals) => (StatusCode::OK, Json(ApiResponse::success(individuals))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<Arc<IndividualRecognition>>::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Arc<IndividualRecognition>>::error(e.to_string()))),
    }
}

async fn get_individual(Json(req): Json<ego::GetIndividualRequest>) -> impl IntoResponse {
    match IndividualRecognitionManager::get().get_individual(&req.agent_id, &req.individual_name).await {
        Ok(individual) => (StatusCode::OK, Json(ApiResponse::success(individual))),
        Err(Error::AgentIndividualNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<Arc<Individual>>::error(format!("Individual {} not found", req.individual_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Arc<Individual>>::error(e.to_string()))),
    }
}

async fn replace_individuals(Json(req): Json<ego::ReplaceIndividualsRequest>) -> impl IntoResponse {
    let result = IndividualRecognitionManager::get().replace_individuals(&req.agent_id, req.remove_individual_names, req.insert_individuals).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Agent {} not found", req.agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn rename_individual(Json(req): Json<ego::RenameIndividualRequest>) -> impl IntoResponse {
    let result = IndividualRecognitionManager::get().rename_individual(&req.agent_id, &req.individual_name, &req.new_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentIndividualNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Individual {} not found", req.individual_name)))),
        Err(Error::AgentIndividualAlreadyExists(_, _)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("Individual {} already exists", req.new_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn replace_individual_identifiers(Json(req): Json<ego::ReplaceIndividualIdentifiersRequest>) -> impl IntoResponse {
    let result = IndividualRecognitionManager::get().replace_individual_identifiers(&req.agent_id, &req.individual_name, req.remove_identifiers, req.insert_identifiers).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentIndividualNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Individual {} not found", req.individual_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn replace_individual_relations(Json(req): Json<ego::ReplaceIndividualRelationsRequest>) -> impl IntoResponse {
    let result = IndividualRecognitionManager::get().replace_individual_other_relations(&req.agent_id, &req.individual_name, req.remove_relations, req.insert_relations).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentIndividualNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Individual {} not found", req.individual_name)))),
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
        Ok(role) => (StatusCode::OK, Json(ApiResponse::success(role))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<Arc<RolePlay>>::error(format!("Role {} not found", req.role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Arc<RolePlay>>::error(e.to_string()))),
    }
}

async fn get_other_role(Json(req): Json<ego::GetOtherRoleRequest>) -> impl IntoResponse {
    match RolePlayManager::get().get_other_role(&req.agent_id, &req.role_name, &req.other_role_name).await {
        Ok(other_role) => (StatusCode::OK, Json(ApiResponse::success(other_role))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<Arc<OtherRole>>::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::<Arc<OtherRole>>::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Arc<OtherRole>>::error(e.to_string()))),
    }
}

async fn create_role(Json(req): Json<ego::CreateRoleRequest>) -> impl IntoResponse {
    let role_name = req.role_name.clone();
    let result = RolePlayManager::get().create_role(&req.agent_id, req.role_name, req.description).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleAlreadyExists(_, _)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("Role {} already exists", role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn create_role_from(Json(req): Json<ego::CreateRoleFromRequest>) -> impl IntoResponse {
    let new_name = req.new_name.clone();
    let result = RolePlayManager::get().create_role_from(&req.agent_id, &req.role_name, req.new_name).await;

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
    let result = RolePlayManager::get().rename_role(&req.agent_id, &req.role_name, req.new_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleAlreadyExists(_, _)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("Role {} already exists", new_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_role_description(Json(req): Json<ego::UpdateRoleDescriptionRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_role_description(&req.agent_id, &req.role_name, req.description).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_role_full_name(Json(req): Json<ego::UpdateRoleFullNameRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_role_full_name(&req.agent_id, &req.role_name, req.full_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn replace_other_roles(Json(req): Json<ego::ReplaceOtherRolesRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().replace_other_roles(&req.agent_id, &req.role_name, req.remove_other_roles, req.insert_other_roles).await;

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

async fn update_other_role_individual_name(Json(req): Json<ego::UpdateOtherRoleIndividualNameRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_other_role_individual_name(&req.agent_id, &req.role_name, &req.other_role_name, req.new_individual_name).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_other_role_description(Json(req): Json<ego::UpdateOtherRoleDescriptionRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_other_role_description(&req.agent_id, &req.role_name, &req.other_role_name, req.new_description).await;
    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn update_other_role_relation(Json(req): Json<ego::UpdateOtherRoleRelationRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_other_role_relation(&req.agent_id, &req.role_name, &req.other_role_name, req.new_relation).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}

async fn replace_other_role_relations(Json(req): Json<ego::ReplaceOtherRoleRelationsRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().replace_other_role_relations(&req.agent_id, &req.role_name, &req.other_role_name, req.remove_relations, req.insert_relations).await;

    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(Error::AgentRoleOtherRoleNotFound(_, _, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Other role {} not found", req.other_role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
