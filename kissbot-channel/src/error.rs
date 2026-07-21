use thiserror::Error;

use crate::{memory_store_client::MessageRecord};

#[derive(Error, Debug)]
pub enum Error {
    #[error("Connect not found: {0}")]
    ConnectNotFound(u32),

    #[error("Messenger not found: {0}")]
    MessengerNotFound(String),

    #[error("Messenger already registered: {0}")]
    MessengerAlreadyRegistered(String),

    #[error("Group not found: {0}")]
    GroupNotFound(String),
    
    #[error("User not found: {0}")]
    UserNotFound(String),

    #[error("User already bound to : {0}")]
    UserAlreadyBound(String),

    #[error("User not bound: {0}")]
    UserNotBound(String),
    
    #[error("Attachment not found: {0}")]
    AttachmentNotFound(String),
    
    #[error("Attachment not found: {0} {1} {2}")]
    AttachmentPositionOutOfOrder(String, u64, u64),
    
    #[error("Invalid message: {0}")]
    InvalidMessage(String),
    
    #[error("Tungstenite error: {0}")]
    TungsteniteError(#[from] tokio_tungstenite::tungstenite::Error),
    
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    
    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("WS error: {0}")]
    WsError(#[from] kai_ws::Error),

    #[error("Flume Send error: {0}")]
    SendError(#[from] flume::SendError<MessageRecord>),

    #[error("Flume Recv error: {0}")]
    RecvError(#[from] flume::RecvError),

    #[error("Oneshot Recv error: {0}")]
    OneshotRecvError(#[from] tokio::sync::oneshot::error::RecvError),

    #[error("TryFromSliceError: {0}")]
    TryFromSliceError(#[from] std::array::TryFromSliceError),

    #[error("Request error: {0}")]
    RequestError(String),

    #[error("Request error: {0}")]
    ReponseError(String),

    #[error("Internal error: {0}")]
    InternalError(String),

    /// 接收任意 `Box<dyn std::error::Error>` 作为 cause 的外部错误
    #[error("External error: {0}")]
    ExternalError(Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T> = std::result::Result<T, Error>;
