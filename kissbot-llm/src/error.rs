use arcstr::ArcStr;

// ========== 错误类型 ==========

/// kissbot-llm 错误：只覆盖 provider 调用与会话上下文持久化
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Model API error: {0}")]
    ModelApiError(ArcStr),

    #[error("Model provider not found: {0} {1}")]
    ModelProviderNotFound(ArcStr, ArcStr),

    #[error("Model provider not supported: {0}")]
    ModelProviderNotSupported(ArcStr),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("Serde JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("File error: {0}")]
    FileError(#[from] kai_file::Error),

    #[error("Internal error: {0}")]
    InternalError(ArcStr),
}

pub type Result<T> = std::result::Result<T, Error>;
