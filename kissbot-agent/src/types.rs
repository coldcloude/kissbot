use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Mode {
    Role,
    Event(String),
}

// ========== 管理命令类型 ==========

#[derive(Debug)]
pub enum AdminCommand {
    Bind { messenger_id: String, user_id: String },
    Unbind { messenger_id: String },
    Admin { messenger_id: String, user_id: String },
    Unadmin { messenger_id: String, user_id: String },
    SetRole(Option<String>),
    ModeEvent(Option<String>),
    ModeRole,
    Reenter(String),
    Events,
    Reset,
    Model(String),   // 新增：/model <name>
    Agent(String),   // 新增：/agent <id>
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

// ========== MemoryWriter 写入队列 ==========

#[derive(Debug, Clone)]
pub enum WriteTask {
    Think {
        agent_id: String,
        role_name: Option<String>,
        content: String,
        time: String,
    },
    #[allow(dead_code)]
    ToolCall {
        agent_id: String,
        role_name: Option<String>,
        tool_name: String,
        tool_params: serde_json::Value,
        time: String,
    },
    #[allow(dead_code)]
    ToolResult {
        agent_id: String,
        role_name: Option<String>,
        tool_result: serde_json::Value,
        time: String,
    },
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
