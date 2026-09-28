use arcstr::ArcStr;
use thiserror::Error;



#[derive(Error, Debug)]
pub enum Error {
    #[error("Connect not found: {0}")]
    ConnectNotFound(u32),

    #[error("Messenger not found: {0}")]
    MessengerNotFound(ArcStr),

    #[error("Messenger already registered: {0}")]
    MessengerAlreadyRegistered(ArcStr),

    #[error("Group not found: {0}")]
    GroupNotFound(ArcStr),
    
    #[error("User not found: {0}")]
    UserNotFound(ArcStr),

    #[error("User already bound to : {0}")]
    UserAlreadyBound(ArcStr),

    #[error("User not bound: {0}")]
    UserNotBound(ArcStr),
    
    #[error("Attachment not found: {0}")]
    AttachmentNotFound(ArcStr),
    
    #[error("Attachment not found: {0} {1} {2}")]
    AttachmentPositionOutOfOrder(ArcStr, u64, u64),
    
    #[error("Invalid message: {0}")]
    InvalidMessage(ArcStr),
    
    #[error("Tungstenite error: {0}")]
    TungsteniteError(#[from] tokio_tungstenite::tungstenite::Error),
    
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("WS error: {0}")]
    WsError(#[from] kai_ws::Error),


    #[error("Oneshot Recv error: {0}")]
    OneshotRecvError(#[from] tokio::sync::oneshot::error::RecvError),

    #[error("TryFromSliceError: {0}")]
    TryFromSliceError(#[from] std::array::TryFromSliceError),

    #[error("Request error: {0}")]
    RequestError(ArcStr),

    #[error("Request error: {0}")]
    ReponseError(ArcStr),

    #[error("Internal error: {0}")]
    InternalError(ArcStr),

    /// 接收任意 `Box<dyn std::error::Error>` 作为 cause 的外部错误
    #[error("External error: {0}")]
    ExternalError(Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T> = std::result::Result<T, Error>;
