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
    pub sessions: Arc<HashMap<SessionKey, Arc<SessionConfigMap>>>,
    pub default_model: Arc<ProviderModel>,   // (provider, model) 打包
    /// 保留 agent 的默认系统提示词（不调 memory-ego 时用），nexus.json 可持久化修改
    pub default_system_prompt: Arc<String>,
}

impl Default for NexusRepo {
    fn default() -> Self {
        Self {
            channels: Arc::new(ArcSwapHashMap::new()),
            providers: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            agents: Arc::new(HashMap::new()),
            sessions: Arc::new(HashMap::new()),
            default_model: Arc::new(ProviderModel {
                provider: Arc::new(String::new()),
                model: Arc::new(String::new())
            }),
            default_system_prompt: Arc::new(String::new()),
        }
    }
}
