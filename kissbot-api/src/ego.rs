use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Display};

use crate::kinds::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoleKey {
    pub id: String,
    pub name: String,
}

impl Display for RoleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.id, self.name)
    }
}

// ========== UserPrivilege (simple enum, no generics) ==========
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserPrivilege {
    Owner,
    Admin,
    Normal,
}

// ========== UserIdentifier (simple struct, no generics) ==========
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserIdentifier {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
}

// ========== UserRelation - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRelationGeneric<S>
where
    S: StringKind,
{
    pub relation: S::Type,
    pub description: S::Type,
}

// SetKind trait for set type abstraction
pub trait UserRelationKind<S>
where
    S: StringKind,
{
    type Type: Clone;
}

// Aliases for internal use and API use
pub type UserRelationDTO = UserRelationGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalUserRelation;

impl UserRelationKind<LocalString> for LocalUserRelation {
    type Type = UserRelationDTO;
}

// ========== User - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGeneric<S,M,T,UR>
where
    S: StringKind,
    M: MapKind,
    T: SetKind,
    UR: UserRelationKind<S>,
{
    pub privilege: UserPrivilege,
    pub identifiers: T::Set<UserIdentifier>,
    pub relations: M::Map<String, UR::Type>,
    pub description: S::Type,
}

pub trait UserKind<S, M, T, UR>
where
    S: StringKind,
    M: MapKind,
    T: SetKind,
    UR: UserRelationKind<S>,
{
    type Type: Clone;
}

pub type UserDTO = UserGeneric<LocalString, LocalMap, LocalSet, LocalUserRelation>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalUser;

impl UserKind<LocalString, LocalMap, LocalSet, LocalUserRelation> for LocalUser {
    type Type = UserDTO;
}

// ========== UserRecognition - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecognitionGeneric<S,M,T,UR,U>
where
    S: StringKind,
    M: MapKind,
    T: SetKind,
    UR: UserRelationKind<S>,
    U: UserKind<S, M, T, UR>,
{
    pub id: S::Type,
    pub user_map: M::Map<String, U::Type>,
}

pub type UserRecognitionDTO = UserRecognitionGeneric<LocalString, LocalMap, LocalSet, LocalUserRelation, LocalUser>;

// ========== AgentMetadata - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadataGeneric<S>
where
    S: StringKind,
{
    pub id: S::Type,
    pub name: S::Type,
    pub description: S::Type,
    pub created_at: S::Type,
}

pub type AgentMetadataDTO = AgentMetadataGeneric<LocalString>;

// ========== RoleRelation - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRelationGeneric<S>
where
    S: StringKind,
{
    pub relation: S::Type,
    pub description: S::Type,
}

pub type RoleRelationDTO = RoleRelationGeneric<LocalString>;

pub trait RoleRelationKind<S>
where
    S: StringKind,
{
    type Type: Clone;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRoleRelation;

impl RoleRelationKind<LocalString> for LocalRoleRelation {
    type Type = RoleRelationDTO;
}

// ========== OtherRole - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtherRoleGeneric<S, M, RR>
where
    S: StringKind,
    M: MapKind,
    RR: RoleRelationKind<S>,
{
    pub user_name: S::Type,
    pub role_relation: RR::Type,
    pub other_role_relations: M::Map<String, RR::Type>,
    pub description: S::Type,
}

pub trait OtherRoleKind<S, M, RR>
where
    S: StringKind,
    M: MapKind,
    RR: RoleRelationKind<S>,
{
    type Type: Clone;
}

pub type OtherRoleDTO = OtherRoleGeneric<LocalString, LocalMap, LocalRoleRelation>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalOtherRole;

impl OtherRoleKind<LocalString, LocalMap, LocalRoleRelation> for LocalOtherRole {
    type Type = OtherRoleDTO;
}

// ========== Role - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleGeneric<S>
where
    S: StringKind,
{
    pub id: S::Type,
    pub name: S::Type,
    pub description: S::Type,
}

pub trait RoleKind<S>
where
    S: StringKind,
{
    type Type: Clone;
}

pub type RoleDTO = RoleGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRole;

impl RoleKind<LocalString> for LocalRole {
    type Type = RoleDTO;
}

// ========== RolePlay - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlayGeneric<S, M, R, RR, OR>
where
    S: StringKind,
    M: MapKind,
    R: RoleKind<S>,
    RR: RoleRelationKind<S>,
    OR: OtherRoleKind<S, M, RR>,
{
    pub role: R::Type,
    pub other_roles: M::Map<String, OR::Type>,
}

pub type RolePlayDTO = RolePlayGeneric<LocalString, LocalMap, LocalRole, LocalRoleRelation, LocalOtherRole>;

// ========== Request Structures (simple, no generics) ==========

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
