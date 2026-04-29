use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use futures::future;
use kai_index::document::to_document;
use kai_index::{Document, SubstringIndex};
use kissbot_memory::DirectoryManager;
use tokio::sync::{OnceCell, RwLock};

use crate::error::Result;
use crate::agent::{AgentManager, AgentMetadata};
use crate::user_recognition_manager::{UserRecognitionManager, ego_user_recognition_md_path};
use crate::role_play_manager::{RolePlayManager, ego_role_play_md_path};
use crate::role_play_relation_manager::{RolePlayRelationManager, ego_role_play_relation_md_path};

struct SearchMetadata {
    value: Vec<Arc<String>>,
}

impl SearchMetadata {
    pub fn new(metadata: &AgentMetadata) -> Self {
        Self {
            value: vec![metadata.name.clone(), metadata.description.clone()],
        }
    }
}

impl Document<Arc<String>> for SearchMetadata {
    fn contents(&self) -> &Vec<Arc<String>> {
        &self.value
    }
}

pub const EGO_IDENTITY_MD: &str = "identity.md";

pub fn ego_identity_md_path(ego_dir: impl AsRef<Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(EGO_IDENTITY_MD)
}

pub struct EgoManager {
    identity_dirty: DashSet<String>,
    name_index: Arc<RwLock<SubstringIndex<String>>>,
    name_descr_index: Arc<RwLock<SubstringIndex<String>>>,
    search_metadata: DashMap<String, SearchMetadata>,
}

static EGO_MANAGER_INSTANCE: OnceCell<EgoManager> = OnceCell::const_new();

impl EgoManager {
    pub fn new() -> Self {
        Self {
            identity_dirty: DashSet::new(),
            name_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            name_descr_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            search_metadata: DashMap::new(),
        }
    }

    pub async fn get() -> Result<&'static Self> {
        EGO_MANAGER_INSTANCE.get_or_try_init(|| async {
            let instance = EgoManager::new();
            let agents = DirectoryManager::get().list_agents().await?;
            for agent_id in agents {
                instance.force_sync_identity_md(&agent_id).await?;
            }
            Ok(instance)
        }).await
    }

    pub async fn force_sync_identity_md(&self, agent_id: &str) -> Result<()> {
        let metadata = AgentManager::get().get_metadata(agent_id).await?;
        //变更索引
        let new_search_metadata = SearchMetadata::new(&metadata);
        let new_name = metadata.name.clone();
        let new_descr = metadata.description.clone();
        let mut name_obsolute = true;
        let mut old_name_or_none = None;
        let mut descr_obsolute = true;
        //没旧值，或新旧值不同，则需要变更索引
        let old_search_metadata_or_none = self.search_metadata.remove(agent_id);
        if let Some((_,old_search_metadata)) = old_search_metadata_or_none.as_ref() {
            //检查name是否变化
            let old_name = old_search_metadata.value[0].clone();
            if old_name == new_name {
                name_obsolute = false;
            }
            else {
                old_name_or_none = Some(old_name);
            }
            //检查description是否变化
            let old_descr = old_search_metadata.value[1].clone();
            if old_descr == new_descr {
                descr_obsolute = false;
            }
        }
        //name变更
        if name_obsolute {
            let mut guard = self.name_index.write().await;
            //有旧值，先移除
            if let Some(old_name) = old_name_or_none {
                guard.remove(&agent_id.to_string(), &to_document(old_name));
            }
            //插入新值
            guard.insert(&agent_id.to_string(), &to_document(new_name));
        }
        //name或description变更
        if name_obsolute || descr_obsolute {
            let mut guard = self.name_descr_index.write().await;
            //有旧值，先移除
            if let Some((_,old_metadata)) = old_search_metadata_or_none {
                guard.remove(&agent_id.to_string(), &old_metadata);
            }
            //插入新值
            guard.insert(&agent_id.to_string(), &new_search_metadata);
        }
        //保存search_metadata
        self.search_metadata.insert(agent_id.to_string(), new_search_metadata);
        //构造MD
        let content = format!(
            "# Agent Identity\n\n- **Name**\n {}\n- **Created At**\n {}\n- **Description**\n {}\n",
            metadata.name, metadata.created_at, metadata.description
        );
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;        
        let identity_path = ego_identity_md_path(&ego_dir);
        tokio::fs::write(identity_path, content).await?;
        Ok(())
    }

    pub async fn sync_identity_md(&self, agent_id: &str) -> Result<()> {
        match self.identity_dirty.remove(&agent_id.to_string()) {
            Some(_) => {
                self.force_sync_identity_md(agent_id).await
            },
            None => {
                Ok(())
            }
        }
    }

    pub async fn sync_all_identity_md(&self) {
        while !self.identity_dirty.is_empty() {
            let agent_ids: Vec<String> = self.identity_dirty.iter().map(|id| id.clone()).collect();
            let mut futs = Vec::new();
            agent_ids.iter().for_each(|id| {
                let fut = self.sync_identity_md(id.as_str());
                futs.push(fut);
            });
            future::join_all(futs).await;
        }
    }

    pub async fn retrieve_agents(&self, agent_ids: Vec<String>) -> Vec<AgentMetadata> {
        let mut results = Vec::new();
        let mut futs = Vec::new();
        agent_ids.iter().for_each(|id| {
            let fut = AgentManager::get().get_metadata(id.as_str());
            futs.push(fut);
        });
        for result in future::join_all(futs).await {
            if let Ok(metadata) = result {
                results.push(metadata);
            }
        }
        results
    }

    pub async fn get_identity_md(&self, agent_id: &str) -> Result<String> {
        self.sync_identity_md(agent_id).await?;
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;        
        let identity_path = ego_identity_md_path(&ego_dir);
        let content = tokio::fs::read_to_string(identity_path).await?;
        Ok(content)
    }

    pub async fn get_user_recognition_md(&self, agent_id: &str) -> Result<String> {
        let users = UserRecognitionManager::get().get_users(agent_id).await?;
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let md_path = ego_user_recognition_md_path(&ego_dir);

        let mut content = String::from("# User Recognition\n\n");
        for user in &users {
            let identity_str = match user.identity {
                crate::user_recognition_manager::UserIdentity::Owner => "Owner",
                crate::user_recognition_manager::UserIdentity::Administrator => "Administrator",
                crate::user_recognition_manager::UserIdentity::Other => "Other",
            };
            
            content.push_str(&format!("## {}\n\n", user.name));
            content.push_str(&format!("- **Identity**: {}\n", identity_str));
            
            if !user.associated_identifiers.is_empty() {
                content.push_str("- **Associated Identifiers**:\n");
                for id in &user.associated_identifiers {
                    content.push_str(&format!("  - {}\n", id));
                }
            }
            
            if !user.relations.is_empty() {
                content.push_str("- **Relations**:\n");
                for rel in &user.relations {
                    content.push_str(&format!("  - With {}: {}\n", rel.other_user, rel.relation));
                    if let Some(desc) = &rel.description {
                        content.push_str(&format!("    - Description: {}\n", desc));
                    }
                }
            }
            
            if let Some(desc) = &user.description {
                content.push_str(&format!("- **Description**: {}\n", desc));
            }
            
            content.push('\n');
        }

        tokio::fs::write(md_path, &content).await?;
        Ok(content)
    }

    pub async fn get_role_play_md(&self, agent_id: &str, role_id: &str) -> Result<String> {
        let role = RolePlayManager::get().get_role(agent_id, role_id).await?;
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let md_path = ego_role_play_md_path(&ego_dir, role_id);

        let mut content = String::from("# Role Play\n\n");
        content.push_str(&format!("## {}\n\n", role.name));
        
        if let Some(desc) = &role.description {
            content.push_str(&format!("- **Description**: {}\n", desc));
        }

        tokio::fs::write(md_path, &content).await?;
        Ok(content)
    }

    pub async fn get_role_play_relation_md(&self, agent_id: &str, relation_id: &str) -> Result<String> {
        let relation = RolePlayRelationManager::get().get_relation(agent_id, relation_id).await?;
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let md_path = ego_role_play_relation_md_path(&ego_dir, relation_id);

        let mut content = String::from("# Role Play Relation\n\n");
        content.push_str(&format!("## {}\n\n", relation.name));
        
        if let Some(user_name) = &relation.associated_user_name {
            content.push_str(&format!("- **Associated User**: {}\n", user_name));
        }
        
        content.push_str(&format!("- **Relation with Agent Role**: {}\n", relation.relation_with_agent_role));
        
        if !relation.relations_with_other_roles.is_empty() {
            content.push_str("- **Relations with Other Roles**:\n");
            for rel in &relation.relations_with_other_roles {
                content.push_str(&format!("  - With {}: {}\n", rel.other_role_name, rel.relation));
                if let Some(desc) = &rel.description {
                    content.push_str(&format!("    - Description: {}\n", desc));
                }
            }
        }
        
        if let Some(desc) = &relation.description {
            content.push_str(&format!("- **Description**: {}\n", desc));
        }

        tokio::fs::write(md_path, &content).await?;
        Ok(content)
    }

    pub async fn search_by_name(&self, query: &str) -> Vec<AgentMetadata> {
        //先同步脏数据
        self.sync_all_identity_md().await;
        //搜索
        let agent_ids: Vec<String> = {
            let guard = self.name_index.read().await;
            guard.find_all_keys(query, false).iter().map(|id| id.to_string()).collect()
        };
        //反查结果
        self.retrieve_agents(agent_ids).await
    }

    pub async fn search_by_description(&self, query: &str) -> Vec<AgentMetadata> {
        //先同步脏数据
        self.sync_all_identity_md().await;
        //搜索
        let agent_ids: Vec<String> = {
            let guard = self.name_descr_index.read().await;
            guard.find_all_keys(query, true).iter().map(|id| id.to_string()).collect()
        };
        //反查结果
        self.retrieve_agents(agent_ids).await
    }
}
