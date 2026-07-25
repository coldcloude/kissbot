use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("File error: {0}")]
    File(#[from] kai_file::Error),

    #[error("Memory error: {0}")]
    KissbotMemory(#[from] kissbot_memory::Error),

    #[error("UTF8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("Record not in order: key='{0}' latest='{1}' new='{2}'")]
    RecordNotInOrder(String, String, String),
}

pub type Result<T> = std::result::Result<T, Error>;
