use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Parse date error: {0}")]
    ParseDate(#[from] chrono::ParseError),

    #[error("File error: {0}")]
    File(#[from] kai_file::Error),

    #[error("Path not exist: {0}")]
    PathNotExist(String),

    #[error("Invalid time range: {0} > {1}")]
    InvalidTimeRange(String, String),
}

pub type Result<T> = std::result::Result<T, Error>;
