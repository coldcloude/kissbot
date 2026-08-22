use std::collections::HashSet;

use kissbot_api::{AgentMetadata, ChannelUser, IndividualRecognition, RolePlay};

/// 由 AgentMetadata 生成系统提示词 markdown（身份）
pub fn build_ego_identity_md(metadata: &AgentMetadata) -> String {
    format!(
        "# Agent Identity\n\n- **Name**\n {}\n- **Description**\n {}\n",
        metadata.agent_id, metadata.description
    )
}

/// 由 IndividualRecognition 生成系统提示词 markdown（个体识别，按 ids 过滤展示的标识）
pub fn build_ego_individual_recognition_md(individuals: &IndividualRecognition, ids: &HashSet<ChannelUser>) -> String {
    let mut content = String::from("# Individual Recognition\n\n");
    for (individual_name, individual_arcswap) in individuals.individual_map.iter() {
        let individual = individual_arcswap.load();

        let mut identifiers = String::new();
        for id in individual.identifiers.iter() {
            if ids.contains(id) {
                identifiers.push_str(&format!("- {} {}\n", id.messenger_id, id.user_id));
            }
        }

        if !identifiers.is_empty() {
            content.push_str(&format!("## {}\n\n", individual_name));

            content.push_str(&format!("- **Relation with Agent**: {} - {}\n", individual.relation.relation, individual.relation.description));

            content.push_str("### Associated Identifiers\n");
            content.push_str(&identifiers);

            content.push_str("### Relations with Others\n");
            for (rel_name, rel_arcswap) in individual.other_relations.iter() {
                let rel = rel_arcswap.load();
                content.push_str(&format!("#### {}\n", rel_name));
                content.push_str(&format!("- **Relation**: {}\n", rel.relation));
                content.push_str(&format!("- **Description**: {}\n", rel.description));
            }
        }

        content.push('\n');
    }
    content
}

/// 由 RolePlay 生成系统提示词 markdown（角色设定，按 individual_names 过滤展示的其他角色）
pub fn build_role_play_md(role: &RolePlay, individual_names: &HashSet<String>) -> String {
    let mut content = String::from("# Role Play\n\n");
    content.push_str(&format!("- **Self Role**: {}\n\n", role.role.role_name));
    content.push_str(&format!("- **Self Description**: {}\n", role.role.description));

    for (other_role_name, other_role_arcswap) in role.other_roles.iter() {
        let other_role = other_role_arcswap.load();
        if individual_names.contains(other_role.individual_name.as_str()) {
            content.push_str(&format!("## Known role: {}\n\n", other_role_name));
            content.push_str(&format!("- **Belong to**: {}\n", other_role.individual_name));
            content.push_str(&format!("- **Description**: {}\n", other_role.description));
            content.push_str(&format!("- **Relation with {}**: {}\n", role.role.role_name, other_role.role_relation.relation));
            content.push_str(&format!("- **Relation with {} Description**: {}\n", role.role.role_name, other_role.role_relation.description));

            content.push_str(&format!("### {}'s relation with Others\n", other_role_name));
            for (rel_name, rel_arcswap) in other_role.other_role_relations.iter() {
                let rel = rel_arcswap.load();
                content.push_str(&format!("#### With {}\n\n", rel_name));
                content.push_str(&format!("- **Relation**: {}\n", rel.relation));
                content.push_str(&format!("- **Relation Description**: {}\n", rel.description));
            }
        }
    }

    content
}
