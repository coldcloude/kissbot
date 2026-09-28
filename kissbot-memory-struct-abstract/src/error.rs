use arcstr::ArcStr;
use thiserror::Error;

/// 摘要记忆组件错误类型（见 spec memory-struct-abstract 错误类型）
#[derive(Error, Debug)]
pub enum Error {
    #[error("Notify error: {0}")]
    NotifyError(ArcStr),

    #[error("Store read error: {0}")]
    StoreReadError(ArcStr),

    #[error("Model API error: {0}")]
    ModelApiError(ArcStr),

    #[error("State error: {0}")]
    StateError(ArcStr),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("File error: {0}")]
    FileError(#[from] kai_file::Error),

    // spec 约定的错误类型（内部错误）；当前实现未产生该分支
    #[allow(dead_code)]
    #[error("Internal error: {0}")]
    InternalError(ArcStr),
}

impl From<kai_ws::Error> for Error {
    fn from(e: kai_ws::Error) -> Self {
        Error::NotifyError(e.to_string().into())
    }
}

impl From<kissbot_llm::Error> for Error {
    fn from(e: kissbot_llm::Error) -> Self {
        Error::ModelApiError(e.to_string().into())
    }
}

impl From<kissbot_memory::Error> for Error {
    fn from(e: kissbot_memory::Error) -> Self {
        Error::StoreReadError(e.to_string().into())
    }
}

impl From<chrono::ParseError> for Error {
    fn from(e: chrono::ParseError) -> Self {
        Error::StateError(format!("时间解析失败: {}", e).into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
