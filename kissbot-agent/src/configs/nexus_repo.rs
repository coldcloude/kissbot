pub mod channel_config;
pub mod session_config;
pub mod provider_model_config;
pub mod memory_struct_config;
pub mod pipeline_config;

pub use channel_config::*;
pub use session_config::*;
pub use provider_model_config::*;
pub use memory_struct_config::*;
pub use pipeline_config::*;

use std::{collections::HashMap, sync::Arc};

use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};

use crate::types::SessionKey;

// ========== 配置数据结构 ==========

/// nexus 可改配置，持久化到 <data_dir>/nexus.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusRepo {
    pub channels: Arc<ArcSwapHashMap<String, ChannelConfig>>,
    pub providers: Arc<ArcSwapHashMap<String, ProviderModelConfig>>, // key = provider 名
    pub memory_structs: Arc<ArcSwapHashMap<String, MemoryStructConfig>>,
    pub agents: Arc<HashMap<String, Arc<AgentRoleConfig>>>,
    /// session 运行配置（key = SessionKey，三元组结构体）：JSON 的 map 键必须是字符串，
    /// 故经 sessions_serde 以 [[key, value], ...] 数组对承载（不是 JSON 对象）
    #[serde(with = "sessions_serde")]
    pub sessions: Arc<HashMap<SessionKey, Arc<SessionConfigMap>>>,
}

/// sessions 字段的 (反)序列化：SessionKey 是结构体，不能作 JSON map 键（serde_json 要求字符串键，
/// 结构体作键会报 "key must be a string"），因此改用数组对承载 key/value。
/// 改形状无存量数据迁移负担：此前 sessions 非空时写盘必然失败，nexus.json 里只可能出现空的 sessions。
mod sessions_serde {
    use super::*;

    pub fn serialize<S>(
        sessions: &Arc<HashMap<SessionKey, Arc<SessionConfigMap>>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Vec<(&SessionKey, &Arc<SessionConfigMap>)> → [[key, value], ...]
        sessions.iter().collect::<Vec<_>>().serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Arc<HashMap<SessionKey, Arc<SessionConfigMap>>>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let entries = Vec::<(SessionKey, Arc<SessionConfigMap>)>::deserialize(deserializer)?;
        Ok(Arc::new(entries.into_iter().collect()))
    }
}

impl Default for NexusRepo {
    fn default() -> Self {
        Self {
            channels: Arc::new(ArcSwapHashMap::new()),
            providers: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            agents: Arc::new(HashMap::new()),
            sessions: Arc::new(HashMap::new()),
        }
    }
}
