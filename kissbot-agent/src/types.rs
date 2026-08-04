use serde::{Deserialize, Serialize};

use crate::config_manager::ProviderModel;

// ========== 错误类型 ==========

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Config not found: {0}")]
    ConfigNotFound(String),

    #[error("Config parse error: {0}")]
    ConfigParseError(String),

    #[error("Model API error: {0}")]
    ModelApiError(String),

    #[error("Model provider not supported: {0}")]
    ModelProviderNotSupported(String),

    #[error("Memory store error: {0}")]
    MemoryStoreError(String),

    #[allow(dead_code)]
    #[error("Memory ego error: {0}")]
    MemoryEgoError(String),

    #[allow(dead_code)]
    #[error("WS connection error: {0}")]
    WsConnectionError(String),

    #[allow(dead_code)]
    #[error("WS bind error: {0}")]
    WsBindError(String),

    #[allow(dead_code)]
    #[error("Station connection error: {0}")]
    StationConnectionError(String),

    #[error("Invalid command: {0}")]
    InvalidCommand(String),

    #[allow(dead_code)]
    #[error("Permission denied")]
    PermissionDenied,

    #[allow(dead_code)]
    #[error("Mode conflict: {0}")]
    ModeConflict(String),

    #[allow(dead_code)]
    #[error("Context overflow")]
    ContextOverflow,

    #[allow(dead_code)]
    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("Serde JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("WS error: {0}")]
    WsError(#[from] kai_ws::Error),

    #[allow(dead_code)]
    #[error("Internal error: {0}")]
    InternalError(String),
}

pub type Result<T> = std::result::Result<T, Error>;

// ========== 模式状态 ==========

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    Role,
    Event(String),
}

// ========== 会话标识 ==========

/// 会话唯一标识：agent_name + role_name + mode 三元组
/// 所有绑定 channel 的信息去重，每个三元组 = 一个会话
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub agent_name: String,
    pub role_name: String,
    pub mode: Mode,
}

/// 记忆读写边界的 role 编码：事件模式拼 {role}-{event}（对 memory-store 透明），角色模式原样
/// role_name/mode 从 Session 运行态字段读（SessionKey 只做去重）
pub fn memory_role(role_name: &str, mode: &Mode) -> String {
    match mode {
        Mode::Event(event_id) => format!("{}-{}", role_name, event_id),
        Mode::Role => role_name.to_string(),
    }
}

// ========== 管理命令类型 ==========

#[derive(Debug)]
pub enum AdminCommand {
    Bind { messenger_id: String, user_id: String },
    // unbind 按 (messenger_id, user_id) 双字段移除 ChannelUser；若移除的是 outgoing 引用身份则清空 outgoing
    Unbind { messenger_id: String, user_id: String },
    /// 设/清空 out_channel：Some 设（覆盖 + 同 agent/role 唯一），None 清空
    BindOutgoing(Option<OutChannelParams>),
    Admin { messenger_id: String, user_id: String },
    Unadmin { messenger_id: String, user_id: String },
    SetRole(Option<String>),
    ModeEvent(Option<String>),
    ModeRole,
    Reenter(String),
    Events,
    Reset,
    Model(ProviderModel, bool),   // /model <provider> <model> [true|false]；true 时写入 NexusRepo 默认模型
    /// 设置 channel 绑定的 agent 与 role（缺省用保留值：agent_name=""、role_name=""）
    SetAgent { agent_name: Option<String>, role: Option<String> },
}

/// /bind-outgoing 命令参数（转 OutChannelConfig 持久化）
#[derive(Debug, Clone)]
pub struct OutChannelParams {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
}

// ========== 管理命令执行效果 ==========

/// 命令执行后协调器需做的后续动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEffect {
    None,
    /// 绑定/模式变化，来源 channel 需按新三元组重定位会话
    Relocate,
    /// 重置来源 channel 所属会话的上下文
    ResetSession,
}

// ========== 模型相关 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool_name: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub content: String,
    /// 思考内容（DeepSeek reasoning_content / anthropic thinking block / <think> 标签兜底；无则为 None）
    pub reasoning_content: Option<String>,
    #[allow(dead_code)]
    pub tool_calls: Vec<ToolCall>,
    #[allow(dead_code)]
    pub finish_reason: String,
}

/// 模型上下文中的单条消息
#[derive(Debug, Clone)]
pub struct MessageItem {
    pub role: String,
    pub content: String,
}

// ========== 上下文消息 ==========

#[derive(Debug, Clone)]
pub enum ContextMessage {
    User {
        #[allow(dead_code)]
        messenger_id: String,
        #[allow(dead_code)]
        user_id: String,
        #[allow(dead_code)]
        group_id: String,
        content: String,
        #[allow(dead_code)]
        time: String,
    },
    Assistant {
        content: String,
        #[allow(dead_code)]
        time: String,
    },
    ToolCall {
        tool_name: String,
        parameters: serde_json::Value,
        #[allow(dead_code)]
        time: String,
    },
    ToolResult {
        tool_name: String,
        result: serde_json::Value,
        #[allow(dead_code)]
        time: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_hash_eq_by_value() {
        use std::collections::HashSet;
        let a = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let b = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let c = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 SessionKey 应命中 HashSet");
        assert!(!set.contains(&c), "不同 mode 不应命中");
    }

    #[test]
    fn memory_role_encodes_event_only() {
        assert_eq!(memory_role("dev", &Mode::Role), "dev");
        assert_eq!(memory_role("dev", &Mode::Event("e1".into())), "dev-e1");
    }
}
