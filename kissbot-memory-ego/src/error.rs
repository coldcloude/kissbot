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

    #[error("Agent role not found: {0} {1}")]
    AgentRoleNotFound(String, String),
}

pub type Result<T> = std::result::Result<T, Error>;
