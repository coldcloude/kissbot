use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Config error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("Memory error: {0}")]
    KissbotMemory(#[from] kissbot_memory::Error),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Agent user not found: {0} {1}")]
    AgentUserNotFound(String, String),

    #[error("Agent user already exists: {0} {1}")]
    AgentUserAlreadyExists(String, String),

    #[error("Agent role not found: {0} {1}")]
    AgentRoleNotFound(String, String),

    #[error("Agent role already exists: {0} {1}")]
    AgentRoleAlreadyExists(String, String),

    #[error("Agent role other role not found: {0} {1} {2}")]
    AgentRoleOtherRoleNotFound(String, String, String),

    #[error("Agent role other role already exists: {0} {1} {2}")]
    AgentRoleOtherRoleAlreadyExists(String, String, String),
}

pub type Result<T> = std::result::Result<T, Error>;
