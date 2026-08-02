use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ArcSwapHashMap;
use crate::channel::ChannelUser;

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

// ========== Internal storage types (use Arc / DashMap / DashSet) ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndividualRelation {
    pub relation: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Individual {
    pub identifiers: Arc<HashSet<ChannelUser>>,
    pub relation: Arc<IndividualRelation>,
    pub other_relations: Arc<ArcSwapHashMap<String, IndividualRelation>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndividualRecognition {
    pub agent_id: Arc<String>,
    pub individual_map: Arc<ArcSwapHashMap<String, Individual>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub agent_id: Arc<String>,
    pub individual_name: Arc<String>,
    pub description: Arc<String>,
    pub created_at: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRelation {
    pub relation: Arc<String>,
    pub full_name: Arc<String>,    // 新增
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub full_name: Arc<String>,    // 新增
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtherRole {
    pub individual_name: Arc<String>,
    pub role_relation: Arc<RoleRelation>,
    pub other_role_relations: Arc<ArcSwapHashMap<String, RoleRelation>>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlay {
    pub role: Arc<Role>,
    pub other_roles: Arc<ArcSwapHashMap<String, OtherRole>>,
}

// ========== Request Structures ==========

// Agent Management Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub individual_name: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetAgentRequest {
    pub agent_id: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateAgentNameRequest {
    pub agent_id: Arc<String>,
    pub individual_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateAgentDescriptionRequest {
    pub agent_id: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CopyAgentRequest {
    pub agent_id: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub keyword: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRoleRequest {
    pub agent_id: Option<Arc<String>>,
    pub keyword: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RetrieveAgentsRequest {
    pub agent_ids: Vec<Arc<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RetrieveRolesRequest {
    pub role_keys: Vec<Arc<RoleKey>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NameCompletionRequest {
    pub prefix: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleNameCompletionRequest {
    pub agent_id: Option<Arc<String>>,
    pub prefix: Arc<String>,
}

// Individual Recognition Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct GetIndividualsRequest {
    pub agent_id: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetIndividualRequest {
    pub agent_id: Arc<String>,
    pub individual_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceIndividualsRequest {
    pub agent_id: Arc<String>,
    pub remove_individual_names: Vec<Arc<String>>,
    pub insert_individuals: Vec<(Arc<String>, Arc<Individual>)>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameIndividualRequest {
    pub agent_id: Arc<String>,
    pub individual_name: Arc<String>,
    pub new_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceIndividualIdentifiersRequest {
    pub agent_id: Arc<String>,
    pub individual_name: Arc<String>,
    pub remove_identifiers: Vec<ChannelUser>,
    pub insert_identifiers: Vec<ChannelUser>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndividualRelationEntry {
    pub individual_name: String,
    pub relation: Arc<IndividualRelation>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceIndividualRelationsRequest {
    pub agent_id: Arc<String>,
    pub individual_name: Arc<String>,
    pub remove_relations: Vec<String>,
    pub insert_relations: Vec<IndividualRelationEntry>,
}

// Role Play Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct ListRolesRequest {
    pub agent_id: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetRoleRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoleRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoleFromRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub new_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoveRoleRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameRoleRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub new_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateRoleDescriptionRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateRoleFullNameRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub full_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetOtherRoleRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub other_role_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OtherRoleEntry {
    pub role_name: String,
    pub other_role: Arc<OtherRole>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceOtherRolesRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub remove_other_roles: Vec<String>,
    pub insert_other_roles: Vec<OtherRoleEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameOtherRoleRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub other_role_name: Arc<String>,
    pub new_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleIndividualNameRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub other_role_name: Arc<String>,
    pub new_individual_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleDescriptionRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub other_role_name: Arc<String>,
    pub new_description: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleRelationRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub other_role_name: Arc<String>,
    pub new_relation: Arc<RoleRelation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleRelationEntry {
    pub role_name: String,
    pub relation: Arc<RoleRelation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceOtherRoleRelationsRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub other_role_name: Arc<String>,
    pub remove_relations: Vec<String>,
    pub insert_relations: Vec<RoleRelationEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_agent_metadata() {
        let obj = AgentMetadata {
            agent_id: Arc::new("a1".to_string()),
            individual_name: Arc::new("Alice".to_string()),
            description: Arc::new("An agent".to_string()),
            created_at: Arc::new("2026-01-01".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: AgentMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "a1");
    }

    // === Agent Management Requests ===

    #[test]
    fn test_serde_create_agent_request() {
        let obj = CreateAgentRequest {
            individual_name: Arc::new("Alice".to_string()),
            description: Arc::new("An agent".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: CreateAgentRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.individual_name, "Alice");
    }

    #[test]
    fn test_serde_get_agent_request() {
        let obj = GetAgentRequest { agent_id: Arc::new("a1".to_string()) };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetAgentRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_update_agent_name_request() {
        let obj = UpdateAgentNameRequest {
            agent_id: Arc::new("a1".to_string()),
            individual_name: Arc::new("Alice".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateAgentNameRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.individual_name, "Alice");
    }

    #[test]
    fn test_serde_update_agent_description_request() {
        let obj = UpdateAgentDescriptionRequest {
            agent_id: Arc::new("a1".to_string()),
            description: Arc::new("New desc".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateAgentDescriptionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.description, "New desc");
    }

    #[test]
    fn test_serde_copy_agent_request() {
        let obj = CopyAgentRequest { agent_id: Arc::new("a1".to_string()) };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: CopyAgentRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_search_request() {
        let obj = SearchRequest { keyword: Arc::new("rust".to_string()) };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: SearchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.keyword, "rust");
    }

    #[test]
    fn test_serde_search_role_request() {
        let obj = SearchRoleRequest {
            agent_id: Some(Arc::new("a1".to_string())),
            keyword: Arc::new("admin".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: SearchRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.keyword, "admin");
        assert!(deserialized.agent_id.is_some());
    }

    #[test]
    fn test_serde_retrieve_agents_request() {
        let obj = RetrieveAgentsRequest {
            agent_ids: vec![Arc::new("a1".to_string()), Arc::new("a2".to_string())],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RetrieveAgentsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_ids.len(), 2);
    }

    #[test]
    fn test_serde_retrieve_roles_request() {
        let role_key = Arc::new(RoleKey { agent_id: "a1".to_string(), role_name: "admin".to_string() });
        let obj = RetrieveRolesRequest { role_keys: vec![role_key] };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RetrieveRolesRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.role_keys.len(), 1);
    }

    #[test]
    fn test_serde_name_completion_request() {
        let obj = NameCompletionRequest { prefix: Arc::new("Al".to_string()) };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: NameCompletionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.prefix, "Al");
    }

    #[test]
    fn test_serde_role_name_completion_request() {
        let obj = RoleNameCompletionRequest {
            agent_id: None,
            prefix: Arc::new("ad".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RoleNameCompletionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.prefix, "ad");
        assert!(deserialized.agent_id.is_none());
    }

    // === Individual Recognition Requests ===

    #[test]
    fn test_serde_get_individuals_request() {
        let obj = GetIndividualsRequest { agent_id: Arc::new("a1".to_string()) };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetIndividualsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_get_individual_request() {
        let obj = GetIndividualRequest {
            agent_id: Arc::new("a1".to_string()),
            individual_name: Arc::new("Alice".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetIndividualRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.individual_name, "Alice");
    }

    #[test]
    fn test_serde_replace_individuals_request() {
        let individual = Arc::new(Individual {
            identifiers: Arc::new(HashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(ArcSwapHashMap::new()),
        });
        let obj = ReplaceIndividualsRequest {
            agent_id: Arc::new("a1".to_string()),
            remove_individual_names: vec![Arc::new("Bob".to_string())],
            insert_individuals: vec![(Arc::new("Alice".to_string()), individual)],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ReplaceIndividualsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.remove_individual_names.len(), 1);
        assert_eq!(deserialized.insert_individuals.len(), 1);
    }

    #[test]
    fn test_serde_rename_individual_request() {
        let obj = RenameIndividualRequest {
            agent_id: Arc::new("a1".to_string()),
            individual_name: Arc::new("Alice".to_string()),
            new_name: Arc::new("Alice2".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RenameIndividualRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.new_name, "Alice2");
    }

    #[test]
    fn test_serde_replace_individual_identifiers_request() {
        let identifier = ChannelUser {
            messenger_id: "m1".to_string(),
            user_id: "u1".to_string(),
        };
        let obj = ReplaceIndividualIdentifiersRequest {
            agent_id: Arc::new("a1".to_string()),
            individual_name: Arc::new("Alice".to_string()),
            remove_identifiers: vec![],
            insert_identifiers: vec![identifier],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ReplaceIndividualIdentifiersRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.insert_identifiers.len(), 1);
    }

    #[test]
    fn test_serde_replace_individual_relations_request() {
        let relation = Arc::new(IndividualRelation {
            relation: Arc::new("friend".to_string()),
            description: Arc::new("best friend".to_string()),
        });
        let obj = ReplaceIndividualRelationsRequest {
            agent_id: Arc::new("a1".to_string()),
            individual_name: Arc::new("Alice".to_string()),
            remove_relations: vec!["enemy".to_string()],
            insert_relations: vec![IndividualRelationEntry { individual_name: "friend".to_string(), relation }],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ReplaceIndividualRelationsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.remove_relations.len(), 1);
        assert_eq!(deserialized.insert_relations.len(), 1);
    }

    // === Role Play Requests ===

    #[test]
    fn test_serde_list_roles_request() {
        let obj = ListRolesRequest { agent_id: Arc::new("a1".to_string()) };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ListRolesRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_get_role_request() {
        let obj = GetRoleRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.role_name, "admin");
    }

    #[test]
    fn test_serde_create_role_request() {
        let obj = CreateRoleRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            description: Arc::new("Administrator".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: CreateRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.description, "Administrator");
    }

    #[test]
    fn test_serde_create_role_from_request() {
        let obj = CreateRoleFromRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            new_name: Arc::new("admin2".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: CreateRoleFromRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.new_name, "admin2");
    }

    #[test]
    fn test_serde_remove_role_request() {
        let obj = RemoveRoleRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RemoveRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.role_name, "admin");
    }

    #[test]
    fn test_serde_rename_role_request() {
        let obj = RenameRoleRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            new_name: Arc::new("mod".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RenameRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.new_name, "mod");
    }

    #[test]
    fn test_serde_update_role_description_request() {
        let obj = UpdateRoleDescriptionRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            description: Arc::new("New desc".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateRoleDescriptionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.description, "New desc");
    }

    #[test]
    fn test_serde_update_role_full_name_request() {
        let obj = UpdateRoleFullNameRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            full_name: Arc::new("管理员".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateRoleFullNameRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.full_name, "管理员");
    }

    #[test]
    fn test_serde_get_other_role_request() {
        let obj = GetOtherRoleRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            other_role_name: Arc::new("Bob".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetOtherRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.other_role_name, "Bob");
    }

    #[test]
    fn test_serde_replace_other_roles_request() {
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                full_name: Arc::new(String::new()),
                description: Arc::new("works together".to_string()),
            }),
            other_role_relations: Arc::new(ArcSwapHashMap::new()),
            description: Arc::new("A colleague".to_string()),
        });
        let obj = ReplaceOtherRolesRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            remove_other_roles: vec![],
            insert_other_roles: vec![OtherRoleEntry { role_name: "Bob".to_string(), other_role }],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ReplaceOtherRolesRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.insert_other_roles.len(), 1);
    }

    #[test]
    fn test_serde_rename_other_role_request() {
        let obj = RenameOtherRoleRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            other_role_name: Arc::new("Bob".to_string()),
            new_name: Arc::new("Bob2".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RenameOtherRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.new_name, "Bob2");
    }

    #[test]
    fn test_serde_update_other_role_individual_name_request() {
        let obj = UpdateOtherRoleIndividualNameRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            other_role_name: Arc::new("Bob".to_string()),
            new_individual_name: Arc::new("Robert".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateOtherRoleIndividualNameRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.new_individual_name, "Robert");
    }

    #[test]
    fn test_serde_update_other_role_description_request() {
        let obj = UpdateOtherRoleDescriptionRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            other_role_name: Arc::new("Bob".to_string()),
            new_description: Arc::new("New desc".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateOtherRoleDescriptionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.new_description, "New desc");
    }

    #[test]
    fn test_serde_update_other_role_relation_request() {
        let new_relation = Arc::new(RoleRelation {
            relation: Arc::new("friend".to_string()),
            full_name: Arc::new(String::new()),
            description: Arc::new("friend".to_string()),
        });
        let obj = UpdateOtherRoleRelationRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            other_role_name: Arc::new("Bob".to_string()),
            new_relation,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateOtherRoleRelationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.new_relation.relation, "friend");
    }

    #[test]
    fn test_serde_replace_other_role_relations_request() {
        let relation = Arc::new(RoleRelation {
            relation: Arc::new("friend".to_string()),
            full_name: Arc::new(String::new()),
            description: Arc::new("friend".to_string()),
        });
        let obj = ReplaceOtherRoleRelationsRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            other_role_name: Arc::new("Bob".to_string()),
            remove_relations: vec![],
            insert_relations: vec![RoleRelationEntry { role_name: "friend".to_string(), relation }],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ReplaceOtherRoleRelationsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.insert_relations.len(), 1);
    }
}
