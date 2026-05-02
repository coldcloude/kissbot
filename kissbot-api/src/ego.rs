use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::kinds::*;

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
    pub channel_id: String,
    pub user_id: String,
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
pub type UserRelationEntity = UserRelationGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalUserRelation;

impl UserRelationKind<LocalString> for LocalUserRelation {
    type Type = UserRelationEntity;
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

pub type UserEntity = UserGeneric<LocalString, LocalMap, LocalSet, LocalUserRelation>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalUser;

impl UserKind<LocalString, LocalMap, LocalSet, LocalUserRelation> for LocalUser {
    type Type = UserEntity;
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

pub type UserRecognitionEntity = UserRecognitionGeneric<LocalString, LocalMap, LocalSet, LocalUserRelation, LocalUser>;

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

pub type AgentMetadataEntity = AgentMetadataGeneric<LocalString>;

// ========== RoleRelation - Generic with trait bounds ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRelationGeneric<S>
where
    S: StringKind,
{
    pub relation: S::Type,
    pub description: S::Type,
}

pub type RoleRelationEntity = RoleRelationGeneric<LocalString>;

pub trait RoleRelationKind<S>
where
    S: StringKind,
{
    type Type: Clone;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRoleRelation;

impl RoleRelationKind<LocalString> for LocalRoleRelation {
    type Type = RoleRelationEntity;
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

pub type OtherRoleEntity = OtherRoleGeneric<LocalString, LocalMap, LocalRoleRelation>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalOtherRole;

impl OtherRoleKind<LocalString, LocalMap, LocalRoleRelation> for LocalOtherRole {
    type Type = OtherRoleEntity;
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

pub type RoleEntity = RoleGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRole;

impl RoleKind<LocalString> for LocalRole {
    type Type = RoleEntity;
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

pub type RolePlayEntity = RolePlayGeneric<LocalString, LocalMap, LocalRole, LocalRoleRelation, LocalOtherRole>;

// ========== Request Structures (simple, no generics) ==========

// Agent Management Requests
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

// User Recognition Requests
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

// Role Play Requests
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
