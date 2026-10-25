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
    /// session 运行配置（key = session_id）：首次读取时按 agent/role 配置合成并缓存，
    /// 之后可由 session 级覆盖更新（如 /model）。缺省为空，兼容尚未写入该字段的旧配置。
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
