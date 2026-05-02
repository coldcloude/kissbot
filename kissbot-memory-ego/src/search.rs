use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use futures::future;
use kai_index::document::to_document;
use kai_index::{Document, SubstringIndex};
use kissbot_memory::DirectoryManager;
use tokio::sync::{OnceCell, RwLock};

use crate::error::Result;
use crate::agent::{AgentManager, AgentMetadata};

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

pub struct SearchManager {
    identity_dirty: DashSet<String>,
    name_index: Arc<RwLock<SubstringIndex<String>>>,
    name_descr_index: Arc<RwLock<SubstringIndex<String>>>,
    search_metadata: DashMap<String, SearchMetadata>,
}

static SEARCH_MANAGER_INSTANCE: OnceCell<SearchManager> = OnceCell::const_new();

impl SearchManager {
    pub fn new() -> Self {
        Self {
            identity_dirty: DashSet::new(),
            name_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            name_descr_index: Arc::new(RwLock::new(SubstringIndex::new(32))),
            search_metadata: DashMap::new(),
        }
    }

    pub async fn get() -> Result<&'static Self> {
        SEARCH_MANAGER_INSTANCE.get_or_try_init(|| async {
            let instance = SearchManager::new();
            let agents = DirectoryManager::get().list_agents().await?;
            for agent_id in agents {
                instance.force_sync_identity(&agent_id).await?;
            }
            Ok(instance)
        }).await
    }

    pub async fn force_sync_identity(&self, agent_id: &str) -> Result<()> {
        let metadata = AgentManager::get().get_agent(agent_id).await?;
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
        Ok(())
    }

    pub async fn sync_identity(&self, agent_id: &str) -> Result<()> {
        match self.identity_dirty.remove(&agent_id.to_string()) {
            Some(_) => {
                self.force_sync_identity(agent_id).await
            },
            None => {
                Ok(())
            }
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

    pub async fn retrieve_agents(&self, agent_ids: Vec<String>) -> Vec<Arc<AgentMetadata>> {
        let mut results = Vec::new();
        let mut futs = Vec::new();
        agent_ids.iter().for_each(|id| {
            let fut = AgentManager::get().get_agent(id.as_str());
            futs.push(fut);
        });
        for result in future::join_all(futs).await {
            if let Ok(metadata) = result {
                results.push(metadata);
            }
        }
        results
    }

    pub async fn search_by_name(&self, query: &str) -> Vec<Arc<AgentMetadata>> {
        //先同步脏数据
        self.sync_all_identity().await;
        //搜索
        let agent_ids: Vec<String> = {
            let guard = self.name_index.read().await;
            guard.find_all_keys(query, false).iter().map(|id| id.to_string()).collect()
        };
        //反查结果
        self.retrieve_agents(agent_ids).await
    }

    pub async fn search_by_description(&self, query: &str) -> Vec<Arc<AgentMetadata>> {
        //先同步脏数据
        self.sync_all_identity().await;
        //搜索
        let agent_ids: Vec<String> = {
            let guard = self.name_descr_index.read().await;
            guard.find_all_keys(query, true).iter().map(|id| id.to_string()).collect()
        };
        //反查结果
        self.retrieve_agents(agent_ids).await
    }
}
