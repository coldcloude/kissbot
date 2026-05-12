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
}

pub type Result<T> = std::result::Result<T, Error>;
