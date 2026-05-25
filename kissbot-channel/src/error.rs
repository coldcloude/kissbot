use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Messenger not found: {0}")]
    MessengerNotFound(String),
    
    #[error("Channel not found: {0}")]
    ChannelNotFound(String),
    
    #[error("Agent not connected: {0}")]
    AgentNotConnected(String),
    
    #[error("User not found: {0}")]
    UserNotFound(String),
    
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

    #[error("Request error: {0}")]
    RequestError(String),
}

pub type Result<T> = std::result::Result<T, Error>;
