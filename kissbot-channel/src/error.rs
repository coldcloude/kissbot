use thiserror::Error;

use crate::{memory_store_client::MessagesRecord};

#[derive(Error, Debug)]
pub enum Error {
    #[error("Agent not bound: {0}")]
    AgentNotBound(String),

    #[error("Agent already bound: {0}")]
    AgentAlreadyBound(String),

    #[error("Messenger not found: {0}")]
    MessengerNotFound(String),

    #[error("Messenger already registered: {0}")]
    MessengerAlreadyRegistered(String),
    
    #[error("Channel not found: {0}")]
    ChannelNotFound(String),
    
    #[error("User not found: {0}")]
    UserNotFound(String),

    #[error("User already bound: {0}")]
    UserAlreadyBound(String),
    
    #[error("Group not found: {0}")]
    GroupNotFound(String),
    
    #[error("Attachment not found: {0}")]
    AttachmentNotFound(String),
    
    #[error("Invalid message: {0}")]
    InvalidMessage(String),
    
    #[error("WSS error: {0}")]
    WssError(#[from] tokio_tungstenite::tungstenite::Error),
    
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    
    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("WS error: {0}")]
    WsError(#[from] kai_ws::Error),

    #[error("Send error: {0}")]
    SendError(#[from] flume::SendError<MessagesRecord>),

    #[error("Recv error: {0}")]
    RecvError(#[from] flume::RecvError),

    #[error("Request error: {0}")]
    RequestError(String),
}

pub type Result<T> = std::result::Result<T, Error>;
