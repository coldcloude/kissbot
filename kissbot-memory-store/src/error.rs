use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Config error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("File error: {0}")]
    File(#[from] kai_file::Error),

    #[error("Memory error: {0}")]
    KissbotMemory(#[from] kissbot_memory::Error),

    #[error("Record not in order: latest='{0}' new='{1}'")]
    RecordNotInOrder(String, String),
}

pub type Result<T> = std::result::Result<T, Error>;
