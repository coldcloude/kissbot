use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::RoleEvent;
use serde::{Deserialize, Serialize};

// ========== pipeline 标识 ==========

/// pipeline 唯一标识：agent_id + role_event（角色名 + 可选事件标识）二元组。
/// 所有绑定 channel 的信息按该二元组去重，每个二元组 = 一条 pipeline；
/// 同时也是记忆读写的范围标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryKey {
    pub agent_id: ArcStr,
    /// 角色 + 事件模式（event None = 角色模式；Some(id) = 事件模式）
    pub role_event: Arc<RoleEvent>,
}
