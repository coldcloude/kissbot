use std::{collections::HashSet};

use kissbot_api::UserIdentifier;

use crate::{agent::AgentMetadata, role_play::RolePlay, user_recognition::UserRecognition};

#[allow(dead_code)]
pub fn build_ego_identity_md(metadata: &AgentMetadata) -> String {
    format!(
        "# Agent Identity\n\n- **Name**\n {}\n- **Created At**\n {}\n- **Description**\n {}\n",
        metadata.name, metadata.created_at, metadata.description
    )
}

#[allow(dead_code)]
pub fn build_ego_user_recognition_md(users: &UserRecognition, ids: &HashSet<UserIdentifier>) -> String {
    let mut content = String::from("# User Recognition\n\n");
    for user in users.user_map.iter() {

        let mut identifiers = String::new();
        for id in user.identifiers.iter() {
            if ids.contains(id.key()) {
                identifiers.push_str(&format!("- {} {} {}\n", id.messenger_id, id.user_id, id.group_id));
            }
        }

        if !identifiers.is_empty() {
            content.push_str(&format!("## {}\n\n", user.key()));
    
            content.push_str(&format!("- **Description**: {}\n", user.description));

            content.push_str("### Associated Identifiers\n");
            content.push_str(&identifiers);

            content.push_str("### Relations\n");
            for rel in user.relations.iter() {
                content.push_str(&format!("#### {}\n", rel.key()));
                content.push_str(&format!("- **Relation**: {}\n", rel.value().relation));
                content.push_str(&format!("- **Description**: {}\n", rel.value().description));
            }
        }

        content.push('\n');
    }
    content    
}

#[allow(dead_code)]
pub fn build_role_play_md(role: &RolePlay, user_names: &HashSet<String>) -> String {
    let mut content = String::from("# Role Play\n\n");
    content.push_str(&format!("- **Self Role**: {}\n\n", role.role.name));
    content.push_str(&format!("- **Self Description**: {}\n", role.role.description));

    for relation in role.other_roles.iter() {
        if user_names.contains(relation.user_name.as_str()) {
            content.push_str(&format!("## Known role: {}\n\n", relation.key()));
            content.push_str(&format!("- **Belong to**: {}\n", relation.user_name));
            content.push_str(&format!("- **Description**: {}\n", relation.description));
            content.push_str(&format!("- **Relation with {}**: {}\n", role.role.name, relation.role_relation.relation));
            content.push_str(&format!("- **Relation with {} Description**: {}\n", role.role.name, relation.role_relation.description));

            content.push_str(&format!("### {}'s relation with Others\n", relation.key()));
            for rel in relation.other_role_relations.iter() {
                content.push_str(&format!("#### With {}\n\n", rel.key()));
                content.push_str(&format!("- **Relation**: {}\n", rel.relation));
                content.push_str(&format!("- **Relation Description**: {}\n", rel.description));
            }
        }
    }

    content
}
