use std::collections::HashSet;
use std::sync::Arc;

use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};

use crate::config_manager::ModelConfig;

/// nexus 可改配置，持久化到 <data_dir>/nexus.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusRepo {
    #[serde(default)]
    pub channels: Arc<ArcSwapHashMap<String, ChannelConfig>>,
    #[serde(default)]
    pub models: Arc<ArcSwapHashMap<String, ModelConfig>>,
    #[serde(default)]
    pub memory_structs: Arc<ArcSwapHashMap<String, MemoryStructConfig>>,
    #[serde(default)]
    pub default_agent_id: Arc<String>,
    #[serde(default)]
    pub default_role: Arc<String>,
    #[serde(default)]
    pub default_model: Arc<String>,
}

impl Default for NexusRepo {
    fn default() -> Self {
        Self {
            channels: Arc::new(ArcSwapHashMap::new()),
            models: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            default_agent_id: Arc::new(String::new()),
            default_role: Arc::new(String::new()),
            default_model: Arc::new(String::new()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub messenger_id: Arc<String>,
    pub ws_url: Arc<String>,
    #[serde(default)]
    pub admins: Arc<HashSet<ChannelUser>>,
    #[serde(default)]
    pub default_bind_user: Option<ChannelUser>,
    #[serde(default)]
    pub enabled_by_default: bool,
}

/// 机器人绑定身份 / 管理员身份统一结构
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct ChannelUser {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStructConfig {
    pub name: Arc<String>,
    pub url: Arc<String>,
}

/// station 可改配置，持久化到 <data_dir>/station.json（本轮占位）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StationRepo {
    #[serde(default)]
    pub stations: Arc<ArcSwapHashMap<String, StationConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationConfig {
    pub station_id: Arc<String>,
    pub base_url: Arc<String>,
    pub timeout_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_user_hash_eq_by_value() {
        let a = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
        let b = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 ChannelUser 应命中 HashSet");
    }

    #[test]
    fn nexus_repo_serde_roundtrip() {
        let repo = NexusRepo {
            channels: Arc::new(ArcSwapHashMap::new()),
            models: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            default_agent_id: Arc::new("agent-1".into()),
            default_role: Arc::new("dev".into()),
            default_model: Arc::new("gpt-4o".into()),
        };
        let json = serde_json::to_string(&repo).unwrap();
        let back: NexusRepo = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.default_agent_id, "agent-1");
        assert_eq!(*back.default_role, "dev");
        assert_eq!(*back.default_model, "gpt-4o");
    }

    #[test]
    fn nexus_repo_default_empty() {
        let repo = NexusRepo::default();
        assert!(repo.channels.is_empty());
        assert!(repo.models.is_empty());
        assert!(repo.memory_structs.is_empty());
        assert!(repo.default_agent_id.is_empty());
    }
}
