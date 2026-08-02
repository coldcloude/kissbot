use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use futures::future;
use kai_index::document::to_document;
use kai_index::prefix_completion::{SimplePrefixCompletion, CompletionResult, PrefixCompletion};
use kai_index::{Document, SubstringIndex};
use kissbot_api::RoleKey;
use kissbot_memory::DirectoryManager;
use tokio::sync::{OnceCell, RwLock};

use crate::agent::AgentManager;
use kissbot_api::AgentMetadata;
use crate::role_play::RolePlayManager;
use kissbot_api::Role;

struct SearchMetadata {
    value: Vec<Arc<String>>,
}

impl SearchMetadata {
    pub fn new(metadata: &AgentMetadata) -> Self {
        Self {
            value: vec![metadata.individual_name.clone(), metadata.description.clone()],
        }
    }
}

impl Document<Arc<String>> for SearchMetadata {
    fn contents(&self) -> &Vec<Arc<String>> {
        &self.value
    }
}

struct RoleSearchMetadata {
    value: Vec<Arc<String>>,
}

impl RoleSearchMetadata {
    pub fn new(role: &Role) -> Self {
        Self {
            value: vec![role.role_name.clone(), role.description.clone()],
        }
    }
}

impl Document<Arc<String>> for RoleSearchMetadata {
    fn contents(&self) -> &Vec<Arc<String>> {
        &self.value
    }
}

trait AsRoleKey {
    fn as_role_key(&self) -> &RoleKey;
}

impl AsRoleKey for RoleKey {
    fn as_role_key(&self) -> &RoleKey {
        self
    }
}

impl AsRoleKey for &RoleKey {
    fn as_role_key(&self) -> &RoleKey {
        self
    }
}

impl AsRoleKey for CompletionResult<RoleKey> {
    fn as_role_key(&self) -> &RoleKey {
        &self.key
    }
}

fn filter_results<R: AsRoleKey>(mut results: Vec<R>, agent_id: Option<&str>) -> Vec<R> {
    if let Some(agent_id) = agent_id {
        let mut filtered_results = Vec::new();
        for result in results.drain(0..results.len()) {
            if result.as_role_key().agent_id == agent_id {
                filtered_results.push(result);
            }
        }
        filtered_results
    }
    else {
        results
    }
}

pub struct SearchManager {
    identity_dirty: DashSet<String>,
    name_index: Arc<RwLock<SubstringIndex<String>>>,
    name_descr_index: Arc<RwLock<SubstringIndex<String>>>,
    name_completion: SimplePrefixCompletion<String>,
    search_metadata: DashMap<String, SearchMetadata>,
    role_dirty: DashSet<RoleKey>,
    role_name_index: Arc<RwLock<SubstringIndex<RoleKey>>>,
    role_name_descr_index: Arc<RwLock<SubstringIndex<RoleKey>>>,
    role_name_completion: SimplePrefixCompletion<RoleKey>,
    role_search_metadata: DashMap<RoleKey, RoleSearchMetadata>,
}

static SEARCH_MANAGER_INSTANCE: OnceCell<SearchManager> = OnceCell::const_new();

impl SearchManager {
    pub fn new() -> Self {
        Self {
            identity_dirty: DashSet::new(),
            name_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            name_descr_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            name_completion: SimplePrefixCompletion::new(),
            search_metadata: DashMap::new(),
            role_dirty: DashSet::new(),
            role_name_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            role_name_descr_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            role_name_completion: SimplePrefixCompletion::new(),
            role_search_metadata: DashMap::new(),
        }
    }

    pub async fn get() -> &'static Self {
        SEARCH_MANAGER_INSTANCE.get_or_init(|| async {
            let instance = SearchManager::new();
            if let Ok(agents) = DirectoryManager::get().list_agents().await {
                for agent_id in agents {
                    instance.force_sync_identity(&agent_id).await;
                    if let Ok(role_names) = RolePlayManager::get().list_roles(&agent_id).await {
                        for role_name in role_names {
                            instance.force_sync_role(&agent_id, &role_name).await;
                        }
                    }
                }
            }
            instance
        }).await
    }

    pub async fn force_sync_identity(&self, agent_id: &str) {
        let old_search_metadata_or_none = self.search_metadata.remove(agent_id);
        let mut old_name_or_none = match old_search_metadata_or_none.as_ref() {
            Some((_, old_search_metadata)) => Some(old_search_metadata.value[0].clone()),
            None => None,
        };
        if let Ok(metadata) = AgentManager::get().get_agent(agent_id).await {
            //存在agent，更新索引
            let new_search_metadata = SearchMetadata::new(&metadata);
            let new_name = metadata.individual_name.clone();
            let new_descr = metadata.description.clone();
            let mut name_obsolute = true;
            let mut descr_obsolute = true;
            //没旧值，或新旧值不同，则需要变更索引
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
                    let old_name_document = to_document(old_name);
                    guard.remove(&agent_id.to_string(), &old_name_document);
                    self.name_completion.remove(&agent_id.to_string(), &old_name_document);
                }
                //插入新值
                let new_name_document = to_document(new_name);
                guard.insert(&agent_id.to_string(), &new_name_document);
                self.name_completion.insert(&agent_id.to_string(), &new_name_document);
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
        }
        else {
            //移除旧名称索引
            if let Some(old_name) = old_name_or_none {
                let old_name_document = to_document(old_name);
                let mut guard = self.name_index.write().await;
                guard.remove(&agent_id.to_string(), &old_name_document);
                self.name_completion.remove(&agent_id.to_string(), &old_name_document);
            }
            //移除旧全文索引
            if let Some((_, old_metadata)) = old_search_metadata_or_none {
                let mut guard = self.name_descr_index.write().await;
                guard.remove(&agent_id.to_string(), &old_metadata);
            }
        }
    }

    pub async fn sync_identity(&self, agent_id: &str) {
        if self.identity_dirty.remove(&agent_id.to_string()).is_some() {
            self.force_sync_identity(agent_id).await;
        }
    }

    pub async fn sync_all_identity(&self) {
        while !self.identity_dirty.is_empty() {
            let agent_ids: Vec<String> = self.identity_dirty.iter().map(|id| id.clone()).collect();
            let mut futs = Vec::new();
            agent_ids.iter().for_each(|id| {
                let fut = self.sync_identity(id.as_str());
                futs.push(fut);
            });
            future::join_all(futs).await;
        }
    }

    pub async fn retrieve_agents(&self, mut agent_ids: Vec<Arc<String>>) -> Vec<Arc<AgentMetadata>> {
        let mut results = Vec::new();
        let mut futs = Vec::new();
        for id in agent_ids.drain(..) {
            let fut = AgentManager::get().get_agent_arc(id);
            futs.push(fut);
        }
        for result in future::join_all(futs).await {
            if let Ok(metadata) = result {
                results.push(metadata);
            }
        }
        results
    }

    pub fn mark_identity_dirty(&self, agent_id: &str) {
        self.identity_dirty.insert(agent_id.to_string());
    }

    pub async fn search_by_name(&self, query: &str) -> Vec<String> {
        //先同步脏数据
        self.sync_all_identity().await;
        //搜索
        let guard = self.name_index.read().await;
        guard.find_all_keys(query, false).iter().map(|id| id.to_string()).collect()
    }

    pub async fn search_by_description(&self, query: &str) -> Vec<String> {
        //先同步脏数据
        self.sync_all_identity().await;
        //搜索
        let guard = self.name_descr_index.read().await;
        guard.find_all_keys(query, true).iter().map(|id| id.to_string()).collect()
    }

    pub async fn name_completion(&self, query: &str) -> Vec<CompletionResult<String>> {
        //先同步脏数据
        self.sync_all_identity().await;
        //搜索
        self.name_completion.complete(query)
    }

    pub async fn force_sync_role(&self, agent_id: &str, role_name: &str) {
        let role_key = RoleKey {
            agent_id: agent_id.to_string(),
            role_name: role_name.to_string(),
        };
        //取得旧索引值
        let old_search_metadata_or_none = self.role_search_metadata.remove(&role_key);
        let old_name_or_none = match old_search_metadata_or_none.as_ref() {
            Some((_, old_search_metadata)) => Some(old_search_metadata.value[0].clone()),
            None => None,
        };
        if let Ok(role_play) = RolePlayManager::get().get_role(agent_id, role_name).await {
            //存在角色，更新索引
            let role = role_play.role.clone();
            //变更索引
            let new_search_metadata = RoleSearchMetadata::new(&role);
            let new_name = role.role_name.clone();
            let new_descr = role.description.clone();
            let mut name_obsolute = true;
            let mut descr_obsolute = true;
                
            //没旧值，或新旧值不同，则需要变更索引
            if let Some(old_name) = old_name_or_none.as_ref() {
                //检查name是否变化
                if old_name.as_str() == new_name.as_str() {
                    name_obsolute = false;
                }
            }
            if let Some((_, old_search_metadata)) = old_search_metadata_or_none.as_ref() {
                //检查description是否变化
                if old_search_metadata.value[1].as_str() == new_descr.as_str() {
                    descr_obsolute = false;
                }
            }
            //name变更
            if name_obsolute {
                let mut guard = self.role_name_index.write().await;
                //有旧值，先移除
                if let Some(old_name) = old_name_or_none {
                    let old_name_document = to_document(old_name);
                    guard.remove(&role_key, &old_name_document);
                    self.role_name_completion.remove(&role_key, &old_name_document);
                }
                //插入新值
                let new_name_document = to_document(new_name);
                guard.insert(&role_key, &new_name_document);
                self.role_name_completion.insert(&role_key, &new_name_document);
            }
            //name或description变更
            if name_obsolute || descr_obsolute {
                let mut guard = self.role_name_descr_index.write().await;
                //有旧值，先移除
                if let Some((_, old_metadata)) = old_search_metadata_or_none {
                    guard.remove(&role_key, &old_metadata);
                }
                //插入新值
                guard.insert(&role_key, &new_search_metadata);
            }
            //保存search_metadata
            self.role_search_metadata.insert(role_key, new_search_metadata);
        }
        else {
            //移除旧名称索引
            if let Some(old_name) = old_name_or_none {
                let old_name_document = to_document(old_name);
                let mut guard = self.role_name_index.write().await;
                guard.remove(&role_key, &old_name_document);
                self.role_name_completion.remove(&role_key, &old_name_document);
            }
            //移除旧全文索引
            if let Some((_, old_metadata)) = old_search_metadata_or_none {
                let mut guard = self.role_name_descr_index.write().await;
                guard.remove(&role_key, &old_metadata);
            }
        }
    }

    pub async fn sync_role(&self, agent_id: &str, role_name: &str) {
        let role_key = RoleKey {
            agent_id: agent_id.to_string(),
            role_name: role_name.to_string(),
        };
        if self.role_dirty.remove(&role_key).is_some() {
            self.force_sync_role(agent_id, role_name).await;
        }
    }

    pub async fn sync_all_roles(&self) {
        while !self.role_dirty.is_empty() {
            let keys: Vec<RoleKey> = self.role_dirty.iter().map(|r| r.clone()).collect();
            for role_key in keys {
                self.sync_role(&role_key.agent_id, &role_key.role_name).await;
            }
        }
    }

    pub async fn retrieve_roles(&self, mut keys: Vec<Arc<RoleKey>>) -> Vec<Arc<Role>> {
        let mut results = Vec::new();
        let mut futs = Vec::new();
        for role_key in keys.drain(..) {
            let fut = RolePlayManager::get().get_role_arc(role_key);
            futs.push(fut);
        }
        for result in future::join_all(futs).await {
            if let Ok(role_play) = result {
                results.push(role_play.role.clone());
            }
        }
        results
    }

    pub fn mark_role_dirty(&self, agent_id: &str, role_name: &str) {
        let role_key = RoleKey {
            agent_id: agent_id.to_string(),
            role_name: role_name.to_string(),
        };
        self.role_dirty.insert(role_key);
    }

    pub async fn search_role_by_name(&self, query: &str, agent_id: Option<&str>) -> Vec<RoleKey> {
        //先同步脏数据
        self.sync_all_roles().await;
        //搜索
        let guard = self.role_name_index.read().await;
        let results = guard.find_all_keys(query, false);
        filter_results(results, agent_id)
    }

    pub async fn search_role_by_description(&self, query: &str, agent_id: Option<&str>) -> Vec<RoleKey> {
        //先同步脏数据
        self.sync_all_roles().await;
        //搜索
        let guard = self.role_name_descr_index.read().await;
        let results = guard.find_all_keys(query, true);
        filter_results(results, agent_id)
    }

    pub async fn role_name_completion(&self, query: &str, agent_id: Option<&str>) -> Vec<CompletionResult<RoleKey>> {
        //先同步脏数据
        self.sync_all_roles().await;
        //搜索
        let results = self.role_name_completion.complete(query);
        filter_results(results, agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_memory::DirectoryManager;

    async fn create_test_agent(agent_id: &str, name: &str, description: &str) {
        let dm = DirectoryManager::get();
        let agent_dir = dm.ensure_agent_dir(agent_id).await.unwrap();
        let metadata = serde_json::json!({
            "agent_id": agent_id,
            "individual_name": name,
            "description": description,
            "created_at": "2026-06-25 10:00:00"
        });
        tokio::fs::write(
            agent_dir.join("metadata.json"),
            serde_json::to_string_pretty(&metadata).unwrap(),
        ).await.unwrap();
    }

    async fn create_test_role(agent_id: &str, role_name: &str, description: &str) {
        let dm = DirectoryManager::get();
        let ego_dir = dm.ensure_agent_ego_dir(agent_id).await.unwrap();
        let role_play = serde_json::json!({
            "role": {
                "agent_id": agent_id,
                "role_name": role_name,
                "full_name": "",
                "description": description
            },
            "other_roles": {}
        });
        let file_name = format!("role-play-{}.json", role_name);
        tokio::fs::write(
            ego_dir.join(&file_name),
            serde_json::to_string_pretty(&role_play).unwrap(),
        ).await.unwrap();
    }

    #[tokio::test]
    async fn test_search_by_name() {
        crate::test_util::init_test_config();
        create_test_agent("name-agt1", "Alice", "Test user").await;
        create_test_agent("name-agt2", "Bob", "Another user").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("name-agt1").await;
        manager.force_sync_identity("name-agt2").await;
        let results = manager.search_by_name("Alice").await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
        assert_eq!(results[0], "name-agt1");
    }

    #[tokio::test]
    async fn test_search_by_name_no_match() {
        crate::test_util::init_test_config();
        create_test_agent("noname-agt", "Alice", "Test").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("noname-agt").await;
        let results = manager.search_by_name("Nonexistent").await;
        assert!(results.is_empty(), "expected empty, got {:?}", results);
    }

    #[tokio::test]
    async fn test_search_by_description() {
        crate::test_util::init_test_config();
        create_test_agent("desc-agt", "Alice", "Some searchable text here").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("desc-agt").await;
        let results = manager.search_by_description("searchable").await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
        assert_eq!(results[0], "desc-agt");
    }

    #[tokio::test]
    async fn test_agent_name_completion() {
        crate::test_util::init_test_config();
        create_test_agent("comp-agt1", "Alice", "").await;
        create_test_agent("comp-agt2", "Albert", "").await;
        create_test_agent("comp-agt3", "Bob", "").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("comp-agt1").await;
        manager.force_sync_identity("comp-agt2").await;
        manager.force_sync_identity("comp-agt3").await;
        let results = manager.name_completion("Al").await;
        assert_eq!(results.len(), 2, "expected 2, got {:?}", results);
        let ids: Vec<&str> = results.iter().map(|r| r.key.as_str()).collect();
        assert!(ids.contains(&"comp-agt1"));
        assert!(ids.contains(&"comp-agt2"));
    }

    #[tokio::test]
    async fn test_search_role_by_name() {
        crate::test_util::init_test_config();
        create_test_agent("role-name-agt", "Alice", "").await;
        create_test_role("role-name-agt", "admin", "Administrator").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("role-name-agt").await;
        manager.force_sync_role("role-name-agt", "admin").await;
        let results = manager.search_role_by_name("admin", None).await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
        assert_eq!(results[0].role_name, "admin");
    }

    #[tokio::test]
    async fn test_search_role_by_description() {
        crate::test_util::init_test_config();
        create_test_agent("role-desc-agt", "Alice", "").await;
        create_test_role("role-desc-agt", "admin", "Special role description").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("role-desc-agt").await;
        manager.force_sync_role("role-desc-agt", "admin").await;
        let results = manager.search_role_by_description("Special", None).await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
    }

    #[tokio::test]
    async fn test_retrieve_agents() {
        crate::test_util::init_test_config();
        create_test_agent("ret-agt1", "Alice", "Desc1").await;
        create_test_agent("ret-agt2", "Bob", "Desc2").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("ret-agt1").await;
        manager.force_sync_identity("ret-agt2").await;
        let results = manager.retrieve_agents(vec![
            Arc::new("ret-agt1".to_string()),
            Arc::new("ret-agt2".to_string()),
        ]).await;
        assert_eq!(results.len(), 2, "expected 2, got {:?}", results);
        let names: Vec<&str> = results.iter().map(|a| a.individual_name.as_str()).collect();
        assert!(names.contains(&"Alice"));
        assert!(names.contains(&"Bob"));
    }
}
