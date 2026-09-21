// ========== 错误类型 ==========

use crate::types::SessionKey;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Config not found: {0}")]
    ConfigNotFound(String),

    #[error("Model API error: {0}")]
    ModelApiError(String),

    #[error("Model provider not found: {0} {0}")]
    ModelProviderNotFound(String, String),

    #[error("Model provider not supported: {0}")]
    ModelProviderNotSupported(String),

    #[error("Memory store error: {0}")]
    MemoryStoreError(String),

    #[error("Memory time window error: {0}")]
    MemoryTimeWindow(String),

    #[error("Memory ego error: {0}")]
    MemoryEgoError(String),

    #[error("Invalid command: {0}")]
    InvalidCommand(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("Serde JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("File error: {0}")]
    FileError(#[from] kai_file::Error),

    #[error("WS error: {0}")]
    WsError(#[from] kai_ws::Error),

    #[error("MCP error: {0}")]
    McpError(#[from] kissbot_mcp::McpError),

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Pipeline {0}: {1} not found for session {2:?}")]
    PipelineNotFound(String, String, SessionKey)
}

pub type Result<T> = std::result::Result<T, Error>;
