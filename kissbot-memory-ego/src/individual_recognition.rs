use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use kissbot_memory::DirectoryManager;
use tokio::sync::RwLock;

use crate::error::Result;
use crate::error::Error;
use kissbot_api::{Individual, IndividualIdentifier, IndividualRecognition, IndividualRelation};

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

    async fn read_individual_recognition_ref<F>(&self, agent_id: &str, mut op: F) -> Result<()>
    where
        F: FnMut(Arc<IndividualRecognition>) -> Result<()>,
    {
        let lock = self.get_or_create_lock(agent_id).await;

        {
            let guard = lock.read().await;
            if let Some(individuals) = guard.as_ref() {
                return op(individuals.clone());
            }
        }

        {
            let mut guard = lock.write().await;

            if let Some(individuals) = guard.as_ref() {
                return op(individuals.clone());
            }

            let ego_dir = DirectoryManager::get().ensure_agent_ego_dir(agent_id).await?;
            let json_path = ego_individual_recognition_path(&ego_dir);

            if !json_path.exists() {
                let individuals = IndividualRecognition {
                    agent_id: Arc::new(agent_id.to_string()),
                    individual_map: Arc::new(dashmap::DashMap::new()),
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
            Some(individuals) => op(individuals.clone()),
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
            match individuals_or_none {
                Some(individuals) => {
                    if let Some(mut individual) = individuals.individual_map.get_mut(individual_name) {
                        *individual = op(individual.clone())?;
                    }
                    Ok(individuals)
                },
                None => return Err(Error::AgentNotFound(agent_id.to_string())),
            }
        }).await
    }

    pub async fn get_individuals(&self, agent_id: &str) -> Result<Arc<IndividualRecognition>> {
        let mut result = Err(Error::AgentNotFound(agent_id.to_string()));
        self.read_individual_recognition_ref(agent_id, |individuals| {
            result = Ok(individuals.clone());
            Ok(())
        }).await?;
        result
    }

    pub async fn get_individual(&self, agent_id: &str, individual_name: &str) -> Result<Arc<Individual>> {
        let mut result = Err(Error::AgentIndividualNotFound(agent_id.to_string(), individual_name.to_string()));
        self.read_individual_recognition_ref(agent_id, |individuals| {
            if let Some(individual) = individuals.individual_map.get(individual_name) {
                result = Ok(individual.clone());
            }
            Ok(())
        }).await?;
        result
    }

    pub async fn replace_individuals(&self, agent_id: &str, mut remove_individual_names: HashSet<String>, mut insert_individuals: HashMap<String, Arc<Individual>>) -> Result<()> {
        self.write_individual_recognition_ref(agent_id, |individuals_or_none| {
            if let Some(individuals) = individuals_or_none {
                for individual_name in remove_individual_names.drain() {
                    individuals.individual_map.remove(&individual_name);
                }
                for (individual_name, individual) in insert_individuals.drain() {
                    individuals.individual_map.insert(individual_name, individual);
                }
                Ok(individuals)
            }
            else {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }

    pub async fn rename_individual(&self, agent_id: &str, individual_name: &str, new_name: &str) -> Result<()> {
        self.write_individual_recognition_ref(agent_id, |individuals_or_none| {
            if let Some(individuals) = individuals_or_none {
                if individuals.individual_map.contains_key(new_name) {
                    Err(Error::AgentIndividualAlreadyExists(agent_id.to_string(), new_name.to_string()))
                } else if let Some((_, individual)) = individuals.individual_map.remove(individual_name) {
                    individuals.individual_map.insert(new_name.to_string(), individual);
                    Ok(individuals)
                } else {
                    Err(Error::AgentIndividualNotFound(agent_id.to_string(), individual_name.to_string()))
                }
            }
            else {
                Err(Error::AgentNotFound(agent_id.to_string()))
            }
        }).await
    }

    pub async fn replace_individual_identifiers(&self, agent_id: &str, individual_name: &str, mut remove_identifiers: HashSet<IndividualIdentifier>, mut insert_identifiers: HashSet<IndividualIdentifier>) -> Result<()> {
        self.write_individual_ref(agent_id, individual_name, |individual| {
            for identifier in remove_identifiers.drain() {
                individual.identifiers.remove(&identifier);
            }
            for identifier in insert_identifiers.drain() {
                individual.identifiers.insert(identifier);
            }
            Ok(individual)
        }).await
    }

    pub async fn replace_individual_other_relations(&self, agent_id: &str, individual_name: &str, mut remove_relations: HashSet<String>, mut insert_relations: HashMap<String, Arc<IndividualRelation>>) -> Result<()> {
        self.write_individual_ref(agent_id, individual_name, |individual| {
            for identifier in remove_relations.drain() {
                individual.other_relations.remove(&identifier);
            }
            for (identifier, relation) in insert_relations.drain() {
                individual.other_relations.insert(identifier, relation);
            }
            Ok(individual)
        }).await
    }
}
