use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::{RoleEvent, RoleKey};
use serde::{Deserialize, Serialize};

// ========== 记忆范围标识 ==========

/// 记忆范围唯一标识：agent_id + role_event（角色名 + 可选事件标识）。
/// 用于 memory-store / memory-summary 的读写范围与 session_id 推导；
/// pipeline 身份由 `RoleKey`（agent + role）决定，同一个 RoleKey 的多个 event 共享一条 pipeline。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryKey {
    pub agent_id: ArcStr,
    /// 角色 + 事件模式（event None = 角色模式；Some(id) = 事件模式）
    pub role_event: Arc<RoleEvent>,
}

impl MemoryKey {
    /// 由角色键与当前 event 合成记忆范围键（event None = 角色模式）
    pub fn from_role_key(role_key: &RoleKey, event_id: Option<ArcStr>) -> Arc<Self> {
        Arc::new(Self {
            agent_id: role_key.agent_id.clone(),
            role_event: Arc::new(RoleEvent { role_name: role_key.role_name.clone(), event_id }),
        })
    }
}
