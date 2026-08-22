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
            value: vec![metadata.agent_id.clone(), metadata.description.clone()],
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
            value: vec![role.role_name.clone(), role.full_name.clone(), role.description.clone()],
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
    name_descr_index: Arc<RwLock<SubstringIndex<String>>>,
    name_completion: SimplePrefixCompletion<String>,
    search_metadata: DashMap<String, SearchMetadata>,
    role_dirty: DashSet<RoleKey>,
    role_name_descr_index: Arc<RwLock<SubstringIndex<RoleKey>>>,
    role_name_completion: SimplePrefixCompletion<RoleKey>,
    role_search_metadata: DashMap<RoleKey, RoleSearchMetadata>,
}

static SEARCH_MANAGER_INSTANCE: OnceCell<SearchManager> = OnceCell::const_new();

impl SearchManager {
    pub fn new() -> Self {
        Self {
            identity_dirty: DashSet::new(),
            name_descr_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            name_completion: SimplePrefixCompletion::new(),
            search_metadata: DashMap::new(),
            role_dirty: DashSet::new(),
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
        let old_metadata = self.search_metadata.remove(agent_id).map(|(_, m)| m);
        let was_indexed = old_metadata.is_some();
        if let Ok(metadata) = AgentManager::get().get_agent(agent_id).await {
            //存在agent，更新索引
            let new_search_metadata = SearchMetadata::new(&metadata);
            let mut fulltext_obsolute = true;
            //没旧值，或新旧值不同，则需要变更全文索引
            if let Some(old) = old_metadata.as_ref() {
                if old.value == new_search_metadata.value {
                    fulltext_obsolute = false;
                }
            }
            if fulltext_obsolute {
                let mut guard = self.name_descr_index.write().await;
                //有旧值，先移除
                if let Some(old) = old_metadata {
                    guard.remove(&agent_id.to_string(), &old);
                }
                //插入新值
                guard.insert(&agent_id.to_string(), &new_search_metadata);
            }
            //name_completion（索引 agent_id，不可变，仅首次索引时插入）
            if !was_indexed {
                let new_id_document = to_document(metadata.agent_id.clone());
                self.name_completion.insert(&agent_id.to_string(), &new_id_document);
            }
            //保存search_metadata
            self.search_metadata.insert(agent_id.to_string(), new_search_metadata);
        }
        else {
            //移除旧全文索引与补全索引
            if let Some(old) = old_metadata {
                let mut guard = self.name_descr_index.write().await;
                guard.remove(&agent_id.to_string(), &old);
                let old_id_document = to_document(old.value[0].clone());
                self.name_completion.remove(&agent_id.to_string(), &old_id_document);
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
            let new_full_name = role.full_name.clone();
            let new_descr = role.description.clone();
            let mut name_obsolute = true;
            let mut full_name_obsolute = true;
            let mut descr_obsolute = true;
                
            //没旧值，或新旧值不同，则需要变更索引
            if let Some(old_name) = old_name_or_none.as_ref() {
                //检查name是否变化
                if old_name.as_str() == new_name.as_str() {
                    name_obsolute = false;
                }
            }
            if let Some((_, old_search_metadata)) = old_search_metadata_or_none.as_ref() {
                //检查full_name是否变化（value[1]；全文索引含 full_name，变更需重建）
                if old_search_metadata.value[1].as_str() == new_full_name.as_str() {
                    full_name_obsolute = false;
                }
                //检查description是否变化（value[2]；value[1] 现为 full_name）
                if old_search_metadata.value[2].as_str() == new_descr.as_str() {
                    descr_obsolute = false;
                }
            }
            //name变更（role_name_index 已删除，仅维护 role_name_completion）
            if name_obsolute {
                //有旧值，先移除
                if let Some(old_name) = old_name_or_none {
                    let old_name_document = to_document(old_name);
                    self.role_name_completion.remove(&role_key, &old_name_document);
                }
                //插入新值
                let new_name_document = to_document(new_name);
                self.role_name_completion.insert(&role_key, &new_name_document);
            }
            //name或full_name或description变更（全文索引 value 含 role_name/full_name/description）
            if name_obsolute || full_name_obsolute || descr_obsolute {
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
            //移除旧名称补全索引（role_name_index 已删除）
            if let Some(old_name) = old_name_or_none {
                let old_name_document = to_document(old_name);
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

    async fn create_test_agent(agent_id: &str, description: &str) {
        let dm = DirectoryManager::get();
        let agent_dir = dm.ensure_agent_dir(agent_id).await.unwrap();
        let metadata = serde_json::json!({
            "agent_id": agent_id,
            "description": description
        });
        tokio::fs::write(
            agent_dir.join("metadata.json"),
            serde_json::to_string_pretty(&metadata).unwrap(),
        ).await.unwrap();
    }

    async fn create_test_role(agent_id: &str, role_name: &str, full_name: &str, description: &str) {
        let dm = DirectoryManager::get();
        let ego_dir = dm.ensure_agent_ego_dir(agent_id).await.unwrap();
        let role_play = serde_json::json!({
            "role": {
                "agent_id": agent_id,
                "role_name": role_name,
                "full_name": full_name,
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
    async fn test_search_by_description() {
        crate::test_util::init_test_config();
        create_test_agent("desc-agt", "Some searchable text here").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("desc-agt").await;
        let results = manager.search_by_description("searchable").await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
        assert_eq!(results[0], "desc-agt");
    }

    #[tokio::test]
    async fn test_agent_name_completion() {
        crate::test_util::init_test_config();
        // 注意：agent_id 须与其他测试（agent.rs 的 alice/Alice/dup_alice/alice_orig 等）互不冲突
        create_test_agent("alice_comp", "").await;
        create_test_agent("albert", "").await;
        create_test_agent("bob", "").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("alice_comp").await;
        manager.force_sync_identity("albert").await;
        manager.force_sync_identity("bob").await;
        let results = manager.name_completion("al").await;
        assert_eq!(results.len(), 2, "expected 2, got {:?}", results);
        let ids: Vec<&str> = results.iter().map(|r| r.key.as_str()).collect();
        assert!(ids.contains(&"alice_comp"));
        assert!(ids.contains(&"albert"));
    }

    #[tokio::test]
    async fn test_search_role_by_description() {
        crate::test_util::init_test_config();
        create_test_agent("role-desc-agt", "").await;
        create_test_role("role-desc-agt", "admin", "", "Special role description").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("role-desc-agt").await;
        manager.force_sync_role("role-desc-agt", "admin").await;
        let results = manager.search_role_by_description("Special", None).await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
    }

    #[tokio::test]
    async fn test_search_role_by_description_matches_full_name() {
        crate::test_util::init_test_config();
        create_test_agent("role-fn-agt", "").await;
        // full_name 含可搜索文本，description 不含
        create_test_role("role-fn-agt", "admin", "超级管理员", "Administrator").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("role-fn-agt").await;
        manager.force_sync_role("role-fn-agt", "admin").await;
        let results = manager.search_role_by_description("超级", None).await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
        assert_eq!(results[0].role_name, "admin");
    }

    #[tokio::test]
    async fn test_role_description_only_change_reindexes() {
        crate::test_util::init_test_config();
        create_test_agent("role-desc-chg-agt", "").await;
        // 初始：旧描述可搜
        create_test_role("role-desc-chg-agt", "admin", "", "旧描述").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("role-desc-chg-agt").await;
        manager.force_sync_role("role-desc-chg-agt", "admin").await;
        let results = manager.search_role_by_description("旧描述", None).await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
        // 仅 description 变更（name/full_name 不变）-> 重索引
        RolePlayManager::get().update_role_description("role-desc-chg-agt", "admin", Arc::new("新描述".into())).await.unwrap();
        manager.force_sync_role("role-desc-chg-agt", "admin").await;
        let results = manager.search_role_by_description("新描述", None).await;
        assert_eq!(results.len(), 1, "expected 1 after desc change, got {:?}", results);
        // 旧描述不再命中
        let results = manager.search_role_by_description("旧描述", None).await;
        assert_eq!(results.len(), 0, "expected 0 for old desc, got {:?}", results);
    }

    #[tokio::test]
    async fn test_role_full_name_only_change_reindexes() {
        crate::test_util::init_test_config();
        create_test_agent("role-fn-chg-agt", "").await;
        // 初始：旧全名可搜（全文索引含 full_name）
        create_test_role("role-fn-chg-agt", "admin", "旧全名", "Administrator").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("role-fn-chg-agt").await;
        manager.force_sync_role("role-fn-chg-agt", "admin").await;
        let results = manager.search_role_by_description("旧全名", None).await;
        assert_eq!(results.len(), 1, "expected 1, got {:?}", results);
        // 仅 full_name 变更（name/description 不变）-> 重索引
        RolePlayManager::get().update_role_full_name("role-fn-chg-agt", "admin", Arc::new("新全名".into())).await.unwrap();
        manager.force_sync_role("role-fn-chg-agt", "admin").await;
        let results = manager.search_role_by_description("新全名", None).await;
        assert_eq!(results.len(), 1, "expected 1 after full_name change, got {:?}", results);
        // 旧全名不再命中
        let results = manager.search_role_by_description("旧全名", None).await;
        assert_eq!(results.len(), 0, "expected 0 for old full_name, got {:?}", results);
    }

    #[tokio::test]
    async fn test_retrieve_agents() {
        crate::test_util::init_test_config();
        create_test_agent("ret-agt1", "Desc1").await;
        create_test_agent("ret-agt2", "Desc2").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("ret-agt1").await;
        manager.force_sync_identity("ret-agt2").await;
        let results = manager.retrieve_agents(vec![
            Arc::new("ret-agt1".to_string()),
            Arc::new("ret-agt2".to_string()),
        ]).await;
        assert_eq!(results.len(), 2, "expected 2, got {:?}", results);
        let names: Vec<&str> = results.iter().map(|a| a.agent_id.as_str()).collect();
        assert!(names.contains(&"ret-agt1"));
        assert!(names.contains(&"ret-agt2"));
    }
}
