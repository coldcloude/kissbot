use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("WS error: {0}")]
    WsError(#[from] kai_ws::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Request error: {0}")]
    RequestError(String),

    #[error("Response error: status_code {0}")]
    ResponseError(u32),

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Not connected")]
    NotConnected,

    #[error("Internal error: {0}")]
    InternalError(String),
}

pub type Result<T> = std::result::Result<T, Error>;
