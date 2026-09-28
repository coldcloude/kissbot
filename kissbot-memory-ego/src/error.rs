use arcstr::ArcStr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Memory error: {0}")]
    KissbotMemory(#[from] kissbot_memory::Error),

    #[error("Agent not found: {0}")]
    AgentNotFound(ArcStr),

    #[error("Agent already exists: {0}")]
    AgentAlreadyExists(ArcStr),

    #[error("Agent individual not found: {0} {1}")]
    AgentIndividualNotFound(ArcStr, ArcStr),

    #[error("Agent individual already exists: {0} {1}")]
    AgentIndividualAlreadyExists(ArcStr, ArcStr),

    #[error("Agent role not found: {0} {1}")]
    AgentRoleNotFound(ArcStr, ArcStr),

    #[error("Agent role already exists: {0} {1}")]
    AgentRoleAlreadyExists(ArcStr, ArcStr),

    #[error("Agent role other role not found: {0} {1} {2}")]
    AgentRoleOtherRoleNotFound(ArcStr, ArcStr, ArcStr),

    #[error("Agent role other role already exists: {0} {1} {2}")]
    AgentRoleOtherRoleAlreadyExists(ArcStr, ArcStr, ArcStr),

    #[error("Invalid code (only [A-Za-z0-9_] allowed, non-empty): {0}")]
    InvalidCode(ArcStr),
}

pub type Result<T> = std::result::Result<T, Error>;
