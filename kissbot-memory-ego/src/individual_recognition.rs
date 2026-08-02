use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use arc_swap::ArcSwap;
use kissbot_api::ArcSwapHashMap;
use kissbot_api::IndividualRelationEntry;
use kissbot_memory::DirectoryManager;
use tokio::sync::RwLock;

use crate::error::Result;
use crate::error::Error;
use crate::code::validate_code;
use kissbot_api::{Individual, IndividualIdentifier, IndividualRecognition, ArcUnwrapOrClone};

pub const EGO_INDIVIDUAL_RECOGNITION_PREFIX: &str = "individual-recognition-";

pub fn ego_individual_recognition_path(ego_dir: impl AsRef<std::path::Path>) -> PathBuf {
    ego_dir.as_ref().to_path_buf().join(format!("{}.json", EGO_INDIVIDUAL_RECOGNITION_PREFIX))
}

type IndividualRecognitionLock = Arc<RwLock<Option<Arc<IndividualRecognition>>>>;

pub struct IndividualRecognitionManager {
    manager_lock: dashmap::DashMap<String, IndividualRecognitionLock>,
}

static INDIVIDUAL_RECOGNITION_MANAGER_INSTANCE: OnceLock<IndividualRecognitionManager> = OnceLock::new();

impl IndividualRecognitionManager {
    pub fn new() -> Self {
        Self {
            manager_lock: dashmap::DashMap::new(),
        }
    }

    pub fn get() -> &'static Self {
        INDIVIDUAL_RECOGNITION_MANAGER_INSTANCE.get_or_init(|| {
            IndividualRecognitionManager::new()
        })
    }

    async fn get_or_create_lock(&self, agent_id: &str) -> IndividualRecognitionLock {
        self.manager_lock
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(None)))
            .clone()
    }

    async fn read_individual_recognition(&self, agent_id: &str) -> Result<Arc<IndividualRecognition>> {
        let lock = self.get_or_create_lock(agent_id).await;

        {
            let guard = lock.read().await;
            if let Some(individuals) = guard.as_ref() {
                return Ok(individuals.clone());
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(individuals) = guard.as_ref() {
                return Ok(individuals.clone());
            }

            let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
            let json_path = ego_individual_recognition_path(&ego_dir);

            if !json_path.exists() {
                let individuals = IndividualRecognition {
                    agent_id: Arc::new(agent_id.to_string()),
                    individual_map: Arc::new(ArcSwapHashMap::new()),
                };
                let content = serde_json::to_string_pretty(&individuals)?;
                tokio::fs::write(&json_path, content).await?;
            }

            if !json_path.exists() {
                return Err(Error::AgentNotFound(agent_id.to_string()));
            }

            let content = tokio::fs::read_to_string(json_path).await?;
            let individuals: IndividualRecognition = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(individuals));
        }

        let guard = lock.read().await;
        match guard.as_ref() {
            Some(individuals) => Ok(individuals.clone()),
            None => Err(Error::AgentNotFound(agent_id.to_string())),
        }
    }

    async fn write_individual_recognition_ref<F>(&self, agent_id: &str, op: F) -> Result<()>
    where
        F: FnOnce(Option<Arc<IndividualRecognition>>) -> Result<Arc<IndividualRecognition>>,
    {
        let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
        let json_path = ego_individual_recognition_path(&ego_dir);

        let lock = self.get_or_create_lock(agent_id).await;
        let mut guard = lock.write().await;

        if guard.is_none() && json_path.exists() {
            let content = tokio::fs::read_to_string(&json_path).await?;
            let entity = serde_json::from_str(&content)?;
            *guard = Some(Arc::new(entity));
        }

        let individuals = guard.take();
        match op(individuals.clone()) {
            Ok(new_individuals) => {
                *guard = Some(new_individuals);
            }
            Err(e) => {
                *guard = individuals;
                return Err(e);
            }
        }

        match guard.as_ref() {
            Some(individuals) => {
                let content = serde_json::to_string_pretty(individuals)?;
                tokio::fs::write(json_path, content).await?;
                Ok(())
            }
            None => {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }
    }

    async fn write_individual_ref<F>(&self, agent_id: &str, individual_name: &str, op: F) -> Result<()>
    where
        F: FnOnce(Arc<Individual>) -> Result<Arc<Individual>>,
    {
        self.write_individual_recognition_ref(agent_id, |individuals_or_none| {
            if let Some(individuals) = individuals_or_none {
                if let Some(entry) = individuals.individual_map.get(individual_name) {
                    let current = entry.load_full();
                    let updated = op(current)?;
                    entry.store(updated);
                    Ok(individuals)
                } else {
                    Err(Error::AgentIndividualNotFound(agent_id.to_string(), individual_name.to_string()))
                }
            } else {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }

    pub async fn get_individuals(&self, agent_id: &str) -> Result<Arc<IndividualRecognition>> {
        self.read_individual_recognition(agent_id).await
    }

    pub async fn get_individual(&self, agent_id: &str, individual_name: &str) -> Result<Arc<Individual>> {
        let individuals = self.read_individual_recognition(agent_id).await?;
        let entry = individuals.individual_map.get(individual_name)
        .ok_or_else(|| Error::AgentIndividualNotFound(agent_id.to_string(), individual_name.to_string()))?;
        Ok(entry.load().clone())
    }

    pub async fn replace_individuals(&self, agent_id: &str, mut remove_individual_names: Vec<Arc<String>>, mut insert_individuals: Vec<(Arc<String>, Arc<Individual>)>) -> Result<()> {
        for (name, _) in insert_individuals.iter() {
            validate_code(name.as_str())?;
        }
        self.write_individual_recognition_ref(agent_id, |individuals_or_none| {
            if let Some(individuals) = individuals_or_none {
                let mut individuals_new_arc = individuals.clone();
                let individuals_new = Arc::make_mut(&mut individuals_new_arc);
                let individual_map = Arc::make_mut(&mut individuals_new.individual_map);
                for name in remove_individual_names.drain(..) {
                    individual_map.remove(name.as_str());
                }
                for (name, individual) in insert_individuals.drain(..) {
                    individual_map.insert(name.unwrap_or_clone(), ArcSwap::new(individual));
                }
                Ok(individuals_new_arc)
            }
            else {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }

    pub async fn rename_individual(&self, agent_id: &str, individual_name: &str, new_name: &str) -> Result<()> {
        validate_code(new_name)?;
        self.write_individual_recognition_ref(agent_id, |individuals_or_none| {
            if let Some(individuals) = individuals_or_none {
                if individuals.individual_map.contains_key(new_name) {
                    Err(Error::AgentIndividualAlreadyExists(agent_id.to_string(), new_name.to_string()))
                } else {
                    if let Some(individual) = individuals.individual_map.get(individual_name) {
                        let mut individuals_new_arc = individuals.clone();
                        let individuals_new = Arc::make_mut(&mut individuals_new_arc);
                        let individual_map = Arc::make_mut(&mut individuals_new.individual_map);
                        individual_map.remove(individual_name);
                        individual_map.insert(new_name.to_string(), ArcSwap::new(individual.load_full()));
                        Ok(individuals_new_arc)
                    } else {
                        Err(Error::AgentIndividualNotFound(agent_id.to_string(), individual_name.to_string()))
                    }
                }
            }
            else {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }

    pub async fn replace_individual_identifiers(&self, agent_id: &str, individual_name: &str, mut remove_identifiers: Vec<IndividualIdentifier>, mut insert_identifiers: Vec<IndividualIdentifier>) -> Result<()> {
        self.write_individual_ref(agent_id, individual_name, |mut individual_arc| {
            let individual = Arc::make_mut(&mut individual_arc);
            let identifiers = Arc::make_mut(&mut individual.identifiers);
            for identifier in remove_identifiers.drain(..) {
                identifiers.remove(&identifier);
            }
            for identifier in insert_identifiers.drain(..) {
                identifiers.insert(identifier);
            }
            Ok(individual_arc)
        }).await
    }

    pub async fn replace_individual_other_relations(&self, agent_id: &str, individual_name: &str, mut remove_relations: Vec<String>, mut insert_relations: Vec<IndividualRelationEntry>) -> Result<()> {
        self.write_individual_ref(agent_id, individual_name, |mut individual_arc| {
            let individual = Arc::make_mut(&mut individual_arc);
            let other_relations = Arc::make_mut(&mut individual.other_relations);
            for name in remove_relations.drain(..) {
                other_relations.remove(name.as_str());
            }
            for entry in insert_relations.drain(..) {
                other_relations.insert(entry.individual_name, ArcSwap::new(entry.relation));
            }
            Ok(individual_arc)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use kissbot_api::IndividualRelation;

use super::*;
    use std::collections::HashSet;

    async fn setup() {
        crate::test_util::init_test_config();
        static SETUP: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        SETUP.get_or_init(|| async {
            let dm = kissbot_memory::DirectoryManager::get();
            dm.ensure_agent_dir("setup-agent").await.unwrap();
            dm.ensure_agent_ego_dir("setup-agent").await.unwrap();
        }).await;
    }

    #[test]
    fn test_ego_individual_recognition_path() {
        let path = ego_individual_recognition_path("/tmp/ego");
        assert_eq!(path, std::path::Path::new("/tmp/ego").join("individual-recognition-.json"));
    }

    #[tokio::test]
    async fn test_get_individuals_new_agent() {
        setup().await;
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
        let result = IndividualRecognitionManager::get().get_individuals("agent1").await.unwrap();
        assert!(result.individual_map.is_empty());
    }

    #[tokio::test]
    async fn test_get_individuals_not_found() {
        setup().await;
        let manager = IndividualRecognitionManager::get();
        // get_individuals 会懒创建文件，对存在的 agent 返回 Ok(空数据)
        // 但 get_individual 对不存在的 individual 返回 AgentIndividualNotFound
        let result = manager.get_individual("setup-agent", "nonexistent").await;
        assert!(matches!(result, Err(Error::AgentIndividualNotFound(_, _))));
    }

    #[tokio::test]
    async fn test_replace_individuals_insert() {
        setup().await;
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        // 先触发文件自动创建
        manager.get_individuals("agent1").await.unwrap();
        let individual = Arc::new(Individual {
            identifiers: Arc::new(HashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(ArcSwapHashMap::new()),
        });
        manager.replace_individuals(
            "agent1",
            vec![],
            vec![(Arc::new("Alice".to_string()), individual)],
        ).await.unwrap();
        let alice = manager.get_individual("agent1", "Alice").await.unwrap();
        assert_eq!(*alice.relation.relation, "friend");
    }

    #[tokio::test]
    async fn test_replace_individuals_remove() {
        setup().await;
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent-remove").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        // 先触发文件自动创建
        manager.get_individuals("agent-remove").await.unwrap();
        let individual = Arc::new(Individual {
            identifiers: Arc::new(HashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(ArcSwapHashMap::new()),
        });
        manager.replace_individuals(
            "agent-remove",
            vec![],
            vec![(Arc::new("Alice".to_string()), individual)],
        ).await.unwrap();
        manager.replace_individuals(
            "agent-remove",
            vec![Arc::new("Alice".to_string())],
            vec![],
        ).await.unwrap();
        let result = manager.get_individual("agent-remove", "Alice").await;
        assert!(matches!(result, Err(Error::AgentIndividualNotFound(_, _))));
    }

    #[tokio::test]
    async fn test_rename_individual() {
        setup().await;
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent-rename").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        // 先触发文件自动创建
        manager.get_individuals("agent-rename").await.unwrap();
        let individual = Arc::new(Individual {
            identifiers: Arc::new(HashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(ArcSwapHashMap::new()),
        });
        manager.replace_individuals(
            "agent-rename",
            vec![],
            vec![(Arc::new("Alice".to_string()), individual)],
        ).await.unwrap();
        manager.rename_individual("agent-rename", "Alice", "Bob").await.unwrap();
        let bob = manager.get_individual("agent-rename", "Bob").await.unwrap();
        assert_eq!(*bob.relation.relation, "friend");
        let result = manager.get_individual("agent-rename", "Alice").await;
        assert!(matches!(result, Err(Error::AgentIndividualNotFound(_, _))));
    }

    #[tokio::test]
    async fn test_rename_individual_already_exists() {
        setup().await;
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent-rename-exists").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        // 先触发文件自动创建
        manager.get_individuals("agent-rename-exists").await.unwrap();
        let alice = Arc::new(Individual {
            identifiers: Arc::new(HashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("".to_string()),
            }),
            other_relations: Arc::new(ArcSwapHashMap::new()),
        });
        let bob = Arc::new(Individual {
            identifiers: Arc::new(HashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("colleague".to_string()),
                description: Arc::new("".to_string()),
            }),
            other_relations: Arc::new(ArcSwapHashMap::new()),
        });
        manager.replace_individuals(
            "agent-rename-exists",
            vec![],
            vec![
                (Arc::new("Alice".to_string()), alice),
                (Arc::new("Bob".to_string()), bob),
            ],
        ).await.unwrap();
        let result = manager.rename_individual("agent-rename-exists", "Alice", "Bob").await;
        assert!(matches!(result, Err(Error::AgentIndividualAlreadyExists(_, _))));
    }

    #[tokio::test]
    async fn test_replace_individuals_rejects_invalid_key() {
        setup().await;
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent-invalid-ind").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        manager.get_individuals("agent-invalid-ind").await.unwrap();
        let individual = Arc::new(Individual {
            identifiers: Arc::new(HashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(ArcSwapHashMap::new()),
        });
        let result = manager.replace_individuals(
            "agent-invalid-ind",
            vec![],
            vec![(Arc::new("a b".to_string()), individual)],
        ).await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
    }

    #[tokio::test]
    async fn test_rename_individual_rejects_empty_name() {
        setup().await;
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent-empty-rename").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        manager.get_individuals("agent-empty-rename").await.unwrap();
        let individual = Arc::new(Individual {
            identifiers: Arc::new(HashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(ArcSwapHashMap::new()),
        });
        manager.replace_individuals(
            "agent-empty-rename",
            vec![],
            vec![(Arc::new("Alice".to_string()), individual)],
        ).await.unwrap();
        let result = manager.rename_individual("agent-empty-rename", "Alice", "").await;
        assert!(matches!(result, Err(Error::InvalidCode(_))));
    }
}
