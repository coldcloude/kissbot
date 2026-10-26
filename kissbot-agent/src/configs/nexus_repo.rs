pub mod channel_config;
pub mod session_config;
pub mod memory_struct_config;
pub mod pipeline_config;

pub use channel_config::*;
pub use session_config::*;
pub use memory_struct_config::*;
pub use pipeline_config::*;

use std::{collections::HashMap, sync::Arc};

use arcstr::ArcStr;
use kissbot_llm::ProviderModelConfig;
use serde::{Deserialize, Serialize};

// ========== 配置数据结构 ==========

/// nexus 可改配置，持久化到 <data_dir>/nexus.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusRepo {
    pub channels: Arc<HashMap<ArcStr, Arc<ChannelConfig>>>,
    pub providers: Arc<HashMap<ArcStr, Arc<ProviderModelConfig>>>, // key = provider 名
    pub memory_structs: Arc<HashMap<ArcStr, Arc<MemoryStructConfig>>>,
    pub agent_roles: Arc<HashMap<ArcStr, Arc<AgentRoleConfig>>>,
    /// session 运行配置（key = session_id）：只存 session 级值（pipeline 固化的有效配置或 /model 等覆盖）；
    /// 缺失的配置项由 pipeline 按 agent/role 配置合成，不在本表隐式继承。缺省为空，兼容旧配置。
    #[serde(default)]
    pub sessions: Arc<HashMap<ArcStr, Arc<SessionConfigMap>>>,
}

impl Default for NexusRepo {
    fn default() -> Self {
        Self {
            channels: Arc::new(HashMap::new()),
            providers: Arc::new(HashMap::new()),
            memory_structs: Arc::new(HashMap::new()),
            agent_roles: Arc::new(HashMap::new()),
            sessions: Arc::new(HashMap::new()),
        }
    }
}
