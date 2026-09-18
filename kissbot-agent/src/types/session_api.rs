use serde::{Deserialize, Serialize};

// ========== 模式状态 ==========

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    Role,
    Event(String),
}

// ========== 会话标识 ==========

/// 会话唯一标识：agent_id + role_name + mode 三元组
/// 所有绑定 channel 的信息去重，每个三元组 = 一个会话
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey {
    pub agent_id: String,
    pub role_name: String,
    pub mode: Mode,
}

/// 记忆读写边界的 role 编码：事件模式拼 {role}-{event}（对 memory-store 透明），角色模式原样
/// 调用方按需从 SessionKey 的 role_name/mode 现场计算（SessionKey 只做会话去重）
pub fn role_mode(role_name: &str, mode: &Mode) -> String {
    match mode {
        Mode::Event(event_id) => format!("{}-{}", role_name, event_id),
        Mode::Role => role_name.to_string(),
    }
}
