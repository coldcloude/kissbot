use std::{collections::HashMap, sync::Arc};

use dashmap::{DashMap, DashSet};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoleKey {
    pub agent_id: String,
    pub role_name: String,
}

impl std::fmt::Display for RoleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.agent_id, self.role_name)
    }
}

// ========== Simple enums / structs (no Arc needed, for JSON) ==========

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserPrivilege {
    Owner,
    Admin,
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserIdentifier {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
}

// ========== Internal storage types (use Arc / DashMap / DashSet) ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRelation {
    pub relation: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub privilege: UserPrivilege,
    pub identifiers: Arc<DashSet<UserIdentifier>>,
    pub relations: Arc<DashMap<String, Arc<UserRelation>>>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecognition {
    pub agent_id: Arc<String>,
    pub user_map: Arc<DashMap<String, Arc<User>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub id: Arc<String>,
    pub name: Arc<String>,
    pub description: Arc<String>,
    pub created_at: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRelation {
    pub relation: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub id: Arc<String>,
    pub name: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtherRole {
    pub user_name: Arc<String>,
    pub role_relation: Arc<RoleRelation>,
    pub other_role_relations: Arc<DashMap<String, Arc<RoleRelation>>>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlay {
    pub role: Arc<Role>,
    pub other_roles: Arc<DashMap<String, Arc<OtherRole>>>,
}

// ========== Request Structures (keep as simple String for JSON) ==========

// Agent Management Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetAgentRequest {
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateAgentNameRequest {
    pub agent_id: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateAgentDescriptionRequest {
    pub agent_id: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CopyAgentRequest {
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub keyword: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRoleRequest {
    pub agent_id: Option<String>,
    pub keyword: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RetrieveAgentsRequest {
    pub agent_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RetrieveRolesRequest {
    pub role_keys: Vec<RoleKey>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NameCompletionRequest {
    pub prefix: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleNameCompletionRequest {
    pub agent_id: Option<String>,
    pub prefix: String,
}

// User Recognition Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct GetUsersRequest {
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetUserRequest {
    pub agent_id: String,
    pub user_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceUsersRequest {
    pub agent_id: String,
    pub remove_user_names: Vec<String>,
    pub insert_users: HashMap<String, UserRequest>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserRequest {
    pub privilege: UserPrivilege,
    pub identifiers: Vec<UserIdentifier>,
    pub relations: HashMap<String, UserRelationRequest>,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserRelationRequest {
    pub relation: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameUserRequest {
    pub agent_id: String,
    pub user_name: String,
    pub new_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateUserPrivilegeRequest {
    pub agent_id: String,
    pub user_name: String,
    pub privilege: UserPrivilege,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateUserDescriptionRequest {
    pub agent_id: String,
    pub user_name: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceUserIdentifiersRequest {
    pub agent_id: String,
    pub user_name: String,
    pub remove_identifiers: Vec<UserIdentifier>,
    pub insert_identifiers: Vec<UserIdentifier>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceUserRelationsRequest {
    pub agent_id: String,
    pub user_name: String,
    pub remove_relations: Vec<String>,
    pub insert_relations: HashMap<String, UserRelationRequest>,
}

// Role Play Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct ListRolesRequest {
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetRoleRequest {
    pub agent_id: String,
    pub role_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoleRequest {
    pub agent_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoleFromRequest {
    pub agent_id: String,
    pub role_name: String,
    pub new_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoveRoleRequest {
    pub agent_id: String,
    pub role_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameRoleRequest {
    pub agent_id: String,
    pub role_name: String,
    pub new_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateRoleDescriptionRequest {
    pub agent_id: String,
    pub role_name: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetOtherRoleRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceOtherRolesRequest {
    pub agent_id: String,
    pub role_name: String,
    pub remove_other_roles: Vec<String>,
    pub insert_other_roles: HashMap<String, OtherRoleRequest>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OtherRoleRequest {
    pub user_name: String,
    pub role_relation: RoleRelationRequest,
    pub other_role_relations: HashMap<String, RoleRelationRequest>,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleRelationRequest {
    pub relation: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameOtherRoleRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub new_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleUserNameRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub new_user_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleDescriptionRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub new_description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleRelationRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub new_relation: RoleRelationRequest,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceOtherRoleRelationsRequest {
    pub agent_id: String,
    pub role_name: String,
    pub other_role_name: String,
    pub remove_relations: Vec<String>,
    pub insert_relations: HashMap<String, RoleRelationRequest>,
}
