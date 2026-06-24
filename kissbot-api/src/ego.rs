use std::sync::Arc;

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
pub struct IndividualIdentifier {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
}

// ========== Internal storage types (use Arc / DashMap / DashSet) ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndividualRelation {
    pub relation: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Individual {
    pub identifiers: Arc<DashSet<IndividualIdentifier>>,
    pub relation: Arc<IndividualRelation>,
    pub other_relations: Arc<DashMap<String, Arc<IndividualRelation>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndividualRecognition {
    pub agent_id: Arc<String>,
    pub individual_map: Arc<DashMap<String, Arc<Individual>>>,
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
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtherRole {
    pub individual_name: Arc<String>,
    pub role_relation: Arc<RoleRelation>,
    pub other_role_relations: Arc<DashMap<String, Arc<RoleRelation>>>,
    pub description: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlay {
    pub role: Arc<Role>,
    pub other_roles: Arc<DashMap<String, Arc<OtherRole>>>,
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
    pub remove_identifiers: Vec<Arc<IndividualIdentifier>>,
    pub insert_identifiers: Vec<Arc<IndividualIdentifier>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceIndividualRelationsRequest {
    pub agent_id: Arc<String>,
    pub individual_name: Arc<String>,
    pub remove_relations: Vec<Arc<String>>,
    pub insert_relations: Vec<(Arc<String>, Arc<IndividualRelation>)>,
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
pub struct GetOtherRoleRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub other_role_name: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceOtherRolesRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub remove_other_roles: Vec<Arc<String>>,
    pub insert_other_roles: Vec<(Arc<String>, Arc<OtherRole>)>,
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
pub struct ReplaceOtherRoleRelationsRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub other_role_name: Arc<String>,
    pub remove_relations: Vec<Arc<String>>,
    pub insert_relations: Vec<(Arc<String>, Arc<RoleRelation>)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_role_key() {
        let obj = RoleKey { agent_id: "a1".to_string(), role_name: "admin".to_string() };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RoleKey = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "a1");
        assert_eq!(deserialized.role_name, "admin");
    }

    #[test]
    fn test_serde_individual_identifier() {
        let obj = IndividualIdentifier {
            messenger_id: "m1".to_string(),
            user_id: "u1".to_string(),
            group_id: "g1".to_string(),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: IndividualIdentifier = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.messenger_id, "m1");
    }

    #[test]
    fn test_serde_individual_relation() {
        let obj = IndividualRelation {
            relation: Arc::new("friend".to_string()),
            description: Arc::new("best friend".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: IndividualRelation = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.relation, "friend");
    }

    #[test]
    fn test_serde_individual() {
        let identifiers = Arc::new(DashSet::new());
        identifiers.insert(IndividualIdentifier {
            messenger_id: "m1".to_string(), user_id: "u1".to_string(), group_id: "g1".to_string(),
        });
        let obj = Individual {
            identifiers,
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(DashMap::new()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: Individual = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.identifiers.len(), 1);
        assert_eq!(*deserialized.relation.relation, "friend");
    }

    #[test]
    fn test_serde_individual_recognition() {
        let individual = Arc::new(Individual {
            identifiers: Arc::new(DashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(DashMap::new()),
        });
        let individual_map = Arc::new(DashMap::new());
        individual_map.insert("Alice".to_string(), individual);

        let obj = IndividualRecognition {
            agent_id: Arc::new("a1".to_string()),
            individual_map,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: IndividualRecognition = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "a1");
        assert_eq!(deserialized.individual_map.len(), 1);
    }

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

    #[test]
    fn test_serde_role_relation() {
        let obj = RoleRelation {
            relation: Arc::new("parent".to_string()),
            description: Arc::new("parent role".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RoleRelation = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.relation, "parent");
    }

    #[test]
    fn test_serde_role() {
        let obj = Role {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            description: Arc::new("Administrator".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: Role = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.role_name, "admin");
    }

    #[test]
    fn test_serde_other_role() {
        let role_relation = Arc::new(RoleRelation {
            relation: Arc::new("colleague".to_string()),
            description: Arc::new("works together".to_string()),
        });
        let obj = OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation,
            other_role_relations: Arc::new(DashMap::new()),
            description: Arc::new("A colleague".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: OtherRole = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.individual_name, "Bob");
        assert_eq!(deserialized.other_role_relations.len(), 0);
    }

    #[test]
    fn test_serde_role_play() {
        let role = Arc::new(Role {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("admin".to_string()),
            description: Arc::new("Admin".to_string()),
        });
        let obj = RolePlay {
            role,
            other_roles: Arc::new(DashMap::new()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RolePlay = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.role.role_name, "admin");
        assert_eq!(deserialized.other_roles.len(), 0);
    }
}
