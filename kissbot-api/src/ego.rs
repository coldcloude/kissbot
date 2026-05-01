use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ========== 用户权限 ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserPrivilege {
    Owner,
    Admin,
    Normal,
}

// ========== 用户标识 ==========
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserIdentifier {
    pub channel_id: String,
    pub user_id: String,
}

// ========== 请求结构体（输入） ==========
// Agent 管理
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

// 用户识别信息
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

// 角色设定
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

// ========== 响应结构体（输出 - 简化版） ==========
// 用户关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRelation {
    pub relation: String,
    pub description: String,
}

// 用户信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub privilege: UserPrivilege,
    pub identifiers: Vec<UserIdentifier>,
    pub relations: HashMap<String, UserRelation>,
    pub description: String,
}

// 用户识别信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecognition {
    pub id: String,
    pub user_map: HashMap<String, User>,
}

// Agent 元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
}

// 角色关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRelation {
    pub relation: String,
    pub description: String,
}

// 其他角色信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtherRole {
    pub user_name: String,
    pub role_relation: RoleRelation,
    pub other_role_relations: HashMap<String, RoleRelation>,
    pub description: String,
}

// 角色设定信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlay {
    pub id: String,
    pub name: String,
    pub description: String,
    pub other_roles: HashMap<String, OtherRole>,
}
