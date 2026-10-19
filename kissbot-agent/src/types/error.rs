// ========== 错误类型 ==========

use arcstr::ArcStr;

use crate::types::SessionKey;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Config not found: {0}")]
    ConfigNotFound(ArcStr),

    #[error("Model API error: {0}")]
    ModelApiError(ArcStr),

    #[error("Model provider not found: {0} {0}")]
    ModelProviderNotFound(ArcStr, ArcStr),

    #[error("Model provider not supported: {0}")]
    ModelProviderNotSupported(ArcStr),

    #[error("Memory store error: {0}")]
    MemoryStoreError(ArcStr),

    #[error("Memory time window error: {0}")]
    MemoryTimeWindow(ArcStr),

    #[error("Memory ego error: {0}")]
    MemoryEgoError(ArcStr),

    #[error("Abstract memory error: {0}")]
    AbstractError(ArcStr),

    #[error("Invalid command: {0}")]
    InvalidCommand(ArcStr),

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
    InternalError(ArcStr),

    #[error("Pipeline {0} not found for session {1:?}")]
    PipelineNotFound(ArcStr, SessionKey),

    #[error("Errors:\n{}", format_errors(.0))]
    MultiError(Vec<Error>),
}

/// kissbot-llm 错误逐变体映射到 agent 错误（保持本枚举的对外错误表面不变）
impl From<kissbot_llm::Error> for Error {
    fn from(e: kissbot_llm::Error) -> Self {
        match e {
            kissbot_llm::Error::ModelApiError(message) => Error::ModelApiError(message),
            kissbot_llm::Error::ModelProviderNotFound(provider, model) => Error::ModelProviderNotFound(provider, model),
            kissbot_llm::Error::ModelProviderNotSupported(message) => Error::ModelProviderNotSupported(message),
            kissbot_llm::Error::IoError(e) => Error::IoError(e),
            kissbot_llm::Error::ReqwestError(e) => Error::ReqwestError(e),
            kissbot_llm::Error::JsonError(e) => Error::JsonError(e),
            kissbot_llm::Error::FileError(e) => Error::FileError(e),
            kissbot_llm::Error::InternalError(message) => Error::InternalError(message),
        }
    }
}

fn format_errors(errs: &[Error]) -> String {
    errs.iter()
    .map(|e| format!("  - {}", e))
    .collect::<Vec<_>>()
    .join("\n")
}

pub type Result<T> = std::result::Result<T, Error>;
