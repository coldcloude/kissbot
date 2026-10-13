use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::RoleEvent;
use serde::{Deserialize, Serialize};

// ========== 会话标识 ==========

/// 会话唯一标识：agent_id + role_event（角色名 + 可选事件标识）二元组
/// 所有绑定 channel 的信息去重，每个二元组 = 一个会话
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryKey {
    pub agent_id: ArcStr,
    /// 角色 + 事件模式（event None = 角色模式；Some(id) = 事件模式）
    pub role_event: Arc<RoleEvent>,
}

impl MemoryKey {
    /// 会话文件标识（kissbot-llm 会话上下文的 session_id）：{agent_id}-{role}（角色模式）
    /// 或 {agent_id}-{role}-{event}（事件模式）；磁盘布局
    /// <data_dir>/context/<session_id>.jsonl 由该编码决定
    pub fn encode(&self) -> ArcStr {
        match self.role_event.event_id.as_ref() {
            Some(event_id) => ArcStr::from(format!("{}-{}-{}", self.agent_id, self.role_event.role_name, event_id)),
            None => ArcStr::from(format!("{}-{}", self.agent_id, self.role_event.role_name)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionKey {
    Memory(Arc<MemoryKey>),
    Raw(ArcStr),
}
