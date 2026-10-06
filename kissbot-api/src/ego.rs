use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arcstr::ArcStr;
use serde::{Deserialize, Serialize};

use crate::channel::ChannelUser;

/// 保留 agent 的 memory-store/ego agent_id（"0"）
pub const RESERVED_AGENT_ID: ArcStr = arcstr::literal!("0");

/// 角色键：agent + 角色名（ego 无事件概念，保持纯角色名）
///
/// 内部字段用 `ArcStr`，并实现 `PartialEq` / `Hash`，因此 `Arc<RoleKey>`
/// 可直接作为 Set / Map 的键。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoleKey {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
}

impl std::fmt::Display for RoleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.agent_id, self.role_name)
    }
}

// ========== Simple enums / structs ==========

// ========== Internal storage types ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndividualRelation {
    pub relation: ArcStr,
    pub description: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Individual {
    pub identifiers: Arc<HashSet<Arc<ChannelUser>>>,
    pub relation: Arc<IndividualRelation>,
    pub other_relations: Arc<HashMap<ArcStr, Arc<IndividualRelation>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndividualRecognition {
    pub agent_id: ArcStr,
    pub individual_map: Arc<HashMap<ArcStr, Arc<Individual>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub agent_id: ArcStr,
    pub description: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRelation {
    pub relation: ArcStr,
    pub full_name: ArcStr,    // 新增
    pub description: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub full_name: ArcStr,    // 新增
    pub description: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtherRole {
    pub individual_name: ArcStr,
    pub role_relation: Arc<RoleRelation>,
    pub other_role_relations: Arc<HashMap<ArcStr, Arc<RoleRelation>>>,
    pub description: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlay {
    pub role: Arc<Role>,
    pub other_roles: Arc<HashMap<ArcStr, Arc<OtherRole>>>,
}

// ========== Request Structures ==========

// Agent Management Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub agent_id: ArcStr,
    pub description: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetAgentRequest {
    pub agent_id: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateAgentDescriptionRequest {
    pub agent_id: ArcStr,
    pub description: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CopyAgentRequest {
    pub agent_id: ArcStr,
    pub new_agent_id: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub keyword: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRoleRequest {
    pub agent_id: Option<ArcStr>,
    pub keyword: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RetrieveAgentsRequest {
    pub agent_ids: Vec<ArcStr>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RetrieveRolesRequest {
    pub role_keys: Vec<Arc<RoleKey>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NameCompletionRequest {
    pub prefix: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleNameCompletionRequest {
    pub agent_id: Option<ArcStr>,
    pub prefix: ArcStr,
}

// Individual Recognition Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct GetIndividualsRequest {
    pub agent_id: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetIndividualRequest {
    pub agent_id: ArcStr,
    pub individual_name: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndividualEntry {
    pub name: ArcStr,
    pub individual: Arc<Individual>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceIndividualsRequest {
    pub agent_id: ArcStr,
    pub remove_individual_names: Vec<ArcStr>,
    pub insert_individuals: Vec<IndividualEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameIndividualRequest {
    pub agent_id: ArcStr,
    pub individual_name: ArcStr,
    pub new_name: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceIndividualIdentifiersRequest {
    pub agent_id: ArcStr,
    pub individual_name: ArcStr,
    pub remove_identifiers: Vec<ChannelUser>,
    pub insert_identifiers: Vec<ChannelUser>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndividualRelationEntry {
    pub individual_name: ArcStr,
    pub relation: Arc<IndividualRelation>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceIndividualRelationsRequest {
    pub agent_id: ArcStr,
    pub individual_name: ArcStr,
    pub remove_relations: Vec<ArcStr>,
    pub insert_relations: Vec<IndividualRelationEntry>,
}

// Role Play Requests
#[derive(Debug, Serialize, Deserialize)]
pub struct ListRolesRequest {
    pub agent_id: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetRoleRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoleRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub description: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoleFromRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub new_name: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateRoleDescriptionRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub description: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateRoleFullNameRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub full_name: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetOtherRoleRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub other_role_name: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OtherRoleEntry {
    pub role_name: ArcStr,
    pub other_role: Arc<OtherRole>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceOtherRolesRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub remove_other_roles: Vec<ArcStr>,
    pub insert_other_roles: Vec<OtherRoleEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameOtherRoleRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub other_role_name: ArcStr,
    pub new_name: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleIndividualNameRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub other_role_name: ArcStr,
    pub new_individual_name: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleDescriptionRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub other_role_name: ArcStr,
    pub new_description: ArcStr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateOtherRoleRelationRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub other_role_name: ArcStr,
    pub new_relation: Arc<RoleRelation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleRelationEntry {
    pub role_name: ArcStr,
    pub relation: Arc<RoleRelation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceOtherRoleRelationsRequest {
    pub agent_id: ArcStr,
    pub role_name: ArcStr,
    pub other_role_name: ArcStr,
    pub remove_relations: Vec<ArcStr>,
    pub insert_relations: Vec<RoleRelationEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_role_key_works_as_set_key() {
        // 内部字段用 ArcStr，结构体实现 PartialEq + Hash，Arc<RoleKey> 可直接作 Set / Map 键
        use std::collections::HashSet;
        let a = Arc::new(RoleKey { agent_id: ArcStr::from("a1"), role_name: ArcStr::from("admin") });
        let b = Arc::new(RoleKey { agent_id: ArcStr::from("a1"), role_name: ArcStr::from("admin") });
        let c = Arc::new(RoleKey { agent_id: ArcStr::from("a1"), role_name: ArcStr::from("guest") });
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b), "等值 Arc<RoleKey> 应命中 HashSet");
        assert!(!set.contains(&c), "不同角色不应命中");
    }

    #[test]
    fn test_serde_agent_metadata() {
        let obj = AgentMetadata {
            agent_id: ArcStr::from("a1"),
            description: ArcStr::from("An agent"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: AgentMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "a1");
        assert_eq!(deserialized.description, "An agent");
    }

    // === Agent Management Requests ===

    #[test]
    fn test_serde_create_agent_request() {
        let obj = CreateAgentRequest {
            agent_id: ArcStr::from("a1"),
            description: ArcStr::from("An agent"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: CreateAgentRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_get_agent_request() {
        let obj = GetAgentRequest { agent_id: ArcStr::from("a1") };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetAgentRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_update_agent_description_request() {
        let obj = UpdateAgentDescriptionRequest {
            agent_id: ArcStr::from("a1"),
            description: ArcStr::from("New desc"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateAgentDescriptionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.description, "New desc");
    }

    #[test]
    fn test_serde_copy_agent_request() {
        let obj = CopyAgentRequest {
            agent_id: ArcStr::from("a1"),
            new_agent_id: ArcStr::from("a2"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: CopyAgentRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "a1");
        assert_eq!(deserialized.new_agent_id, "a2");
    }

    #[test]
    fn test_serde_search_request() {
        let obj = SearchRequest { keyword: ArcStr::from("rust") };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: SearchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.keyword, "rust");
    }

    #[test]
    fn test_serde_search_role_request() {
        let obj = SearchRoleRequest {
            agent_id: Some(ArcStr::from("a1")),
            keyword: ArcStr::from("admin"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: SearchRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.keyword, "admin");
        assert!(deserialized.agent_id.is_some());
    }

    #[test]
    fn test_serde_retrieve_agents_request() {
        let obj = RetrieveAgentsRequest {
            agent_ids: vec![ArcStr::from("a1"), ArcStr::from("a2")],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RetrieveAgentsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_ids.len(), 2);
    }

    #[test]
    fn test_serde_retrieve_roles_request() {
        let role_key = Arc::new(RoleKey { agent_id: ArcStr::from("a1"), role_name: ArcStr::from("admin") });
        let obj = RetrieveRolesRequest { role_keys: vec![role_key] };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RetrieveRolesRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.role_keys.len(), 1);
    }

    #[test]
    fn test_serde_name_completion_request() {
        let obj = NameCompletionRequest { prefix: ArcStr::from("Al") };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: NameCompletionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.prefix, "Al");
    }

    #[test]
    fn test_serde_role_name_completion_request() {
        let obj = RoleNameCompletionRequest {
            agent_id: None,
            prefix: ArcStr::from("ad"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RoleNameCompletionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.prefix, "ad");
        assert!(deserialized.agent_id.is_none());
    }

    // === Individual Recognition Requests ===

    #[test]
    fn test_serde_get_individuals_request() {
        let obj = GetIndividualsRequest { agent_id: ArcStr::from("a1") };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetIndividualsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_get_individual_request() {
        let obj = GetIndividualRequest {
            agent_id: ArcStr::from("a1"),
            individual_name: ArcStr::from("Alice"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetIndividualRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.individual_name, "Alice");
    }

    #[test]
    fn test_serde_replace_individuals_request() {
        let identifier = Arc::new(ChannelUser {
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
        });
        let individual = Arc::new(Individual {
            identifiers: Arc::new(HashSet::from([identifier])),
            relation: Arc::new(IndividualRelation {
                relation: ArcStr::from("friend"),
                description: ArcStr::from("best friend"),
            }),
            other_relations: Arc::new(HashMap::new()),
        });
        let obj = ReplaceIndividualsRequest {
            agent_id: ArcStr::from("a1"),
            remove_individual_names: vec![ArcStr::from("Bob")],
            insert_individuals: vec![IndividualEntry { name: ArcStr::from("Alice"), individual }],
        };
        let json = serde_json::to_value(&obj).unwrap();
        // Set 元素是 Arc<ChannelUser>，serde 表示仍是数组
        assert_eq!(json["insert_individuals"][0]["individual"]["identifiers"][0]["user_id"], "u1");
        let deserialized: ReplaceIndividualsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.remove_individual_names.len(), 1);
        assert_eq!(deserialized.insert_individuals.len(), 1);
        assert!(deserialized.insert_individuals[0].individual.identifiers.contains(
            &ChannelUser { messenger_id: ArcStr::from("m1"), user_id: ArcStr::from("u1") }
        ));
    }

    #[test]
    fn test_serde_rename_individual_request() {
        let obj = RenameIndividualRequest {
            agent_id: ArcStr::from("a1"),
            individual_name: ArcStr::from("Alice"),
            new_name: ArcStr::from("Alice2"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RenameIndividualRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.new_name, "Alice2");
    }

    #[test]
    fn test_serde_replace_individual_identifiers_request() {
        let identifier = ChannelUser {
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
        };
        let obj = ReplaceIndividualIdentifiersRequest {
            agent_id: ArcStr::from("a1"),
            individual_name: ArcStr::from("Alice"),
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
            relation: ArcStr::from("friend"),
            description: ArcStr::from("best friend"),
        });
        let obj = ReplaceIndividualRelationsRequest {
            agent_id: ArcStr::from("a1"),
            individual_name: ArcStr::from("Alice"),
            remove_relations: vec![ArcStr::from("enemy")],
            insert_relations: vec![IndividualRelationEntry { individual_name: ArcStr::from("friend"), relation }],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ReplaceIndividualRelationsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.remove_relations.len(), 1);
        assert_eq!(deserialized.insert_relations.len(), 1);
    }

    // === Role Play Requests ===

    #[test]
    fn test_serde_list_roles_request() {
        let obj = ListRolesRequest { agent_id: ArcStr::from("a1") };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ListRolesRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_get_role_request() {
        let obj = GetRoleRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.role_name, "admin");
    }

    #[test]
    fn test_serde_create_role_request() {
        let obj = CreateRoleRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            description: ArcStr::from("Administrator"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: CreateRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.description, "Administrator");
    }

    #[test]
    fn test_serde_create_role_from_request() {
        let obj = CreateRoleFromRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            new_name: ArcStr::from("admin2"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: CreateRoleFromRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.new_name, "admin2");
    }

    #[test]
    fn test_serde_update_role_description_request() {
        let obj = UpdateRoleDescriptionRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            description: ArcStr::from("New desc"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateRoleDescriptionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.description, "New desc");
    }

    #[test]
    fn test_serde_update_role_full_name_request() {
        let obj = UpdateRoleFullNameRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            full_name: ArcStr::from("管理员"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateRoleFullNameRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.full_name, "管理员");
    }

    #[test]
    fn test_serde_get_other_role_request() {
        let obj = GetOtherRoleRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            other_role_name: ArcStr::from("Bob"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: GetOtherRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.other_role_name, "Bob");
    }

    #[test]
    fn test_serde_replace_other_roles_request() {
        let other_role = Arc::new(OtherRole {
            individual_name: ArcStr::from("Bob"),
            role_relation: Arc::new(RoleRelation {
                relation: ArcStr::from("colleague"),
                full_name: ArcStr::new(),
                description: ArcStr::from("works together"),
            }),
            other_role_relations: Arc::new(HashMap::new()),
            description: ArcStr::from("A colleague"),
        });
        let obj = ReplaceOtherRolesRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            remove_other_roles: vec![],
            insert_other_roles: vec![OtherRoleEntry { role_name: ArcStr::from("Bob"), other_role }],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ReplaceOtherRolesRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.insert_other_roles.len(), 1);
    }

    #[test]
    fn test_serde_rename_other_role_request() {
        let obj = RenameOtherRoleRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            other_role_name: ArcStr::from("Bob"),
            new_name: ArcStr::from("Bob2"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RenameOtherRoleRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.new_name, "Bob2");
    }

    #[test]
    fn test_serde_update_other_role_individual_name_request() {
        let obj = UpdateOtherRoleIndividualNameRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            other_role_name: ArcStr::from("Bob"),
            new_individual_name: ArcStr::from("Robert"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateOtherRoleIndividualNameRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.new_individual_name, "Robert");
    }

    #[test]
    fn test_serde_update_other_role_description_request() {
        let obj = UpdateOtherRoleDescriptionRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            other_role_name: ArcStr::from("Bob"),
            new_description: ArcStr::from("New desc"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateOtherRoleDescriptionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.new_description, "New desc");
    }

    #[test]
    fn test_serde_update_other_role_relation_request() {
        let new_relation = Arc::new(RoleRelation {
            relation: ArcStr::from("friend"),
            full_name: ArcStr::new(),
            description: ArcStr::from("friend"),
        });
        let obj = UpdateOtherRoleRelationRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            other_role_name: ArcStr::from("Bob"),
            new_relation,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: UpdateOtherRoleRelationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.new_relation.relation, "friend");
    }

    #[test]
    fn test_serde_replace_other_role_relations_request() {
        let relation = Arc::new(RoleRelation {
            relation: ArcStr::from("friend"),
            full_name: ArcStr::new(),
            description: ArcStr::from("friend"),
        });
        let obj = ReplaceOtherRoleRelationsRequest {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("admin"),
            other_role_name: ArcStr::from("Bob"),
            remove_relations: vec![],
            insert_relations: vec![RoleRelationEntry { role_name: ArcStr::from("friend"), relation }],
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ReplaceOtherRoleRelationsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.insert_relations.len(), 1);
    }
}
