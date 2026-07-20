use thiserror::Error;

use crate::attachment::UploadCommand;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Group not found: {0}")]
    GroupNotFound(String),

    #[error("User not found: {0}")]
    UserNotFound(String),

    #[error("Attachment not found: {0}")]
    AttachmentNotFound(String),

    #[error("Attachment position out of order: {0} {1} {2}")]
    AttachmentPositionOutOfOrder(String, u64, u64),

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Image error: {0}")]
    ImageError(#[from] image::ImageError),

    #[error("Flume Send error: {0}")]
    SendError(#[from] flume::SendError<UploadCommand>),

    #[error("Flume Recv error: {0}")]
    RecvError(#[from] flume::RecvError),

    #[error("Oneshot Recv error: {0}")]
    OneshotRecvError(#[from] tokio::sync::oneshot::error::RecvError),

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Channel error: {0}")]
    ChannelError(#[from] kissbot_channel::Error),

    #[error("KaiFile error: {0}")]
    KaiFileError(#[from] kai_file::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for kissbot_channel::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::ConfigError(msg) => kissbot_channel::Error::InternalError(format!("config: {}", msg)),
            Error::GroupNotFound(msg) => kissbot_channel::Error::GroupNotFound(msg),
            Error::UserNotFound(msg) => kissbot_channel::Error::UserNotFound(msg),
            Error::AttachmentNotFound(msg) => kissbot_channel::Error::AttachmentNotFound(msg),
            Error::AttachmentPositionOutOfOrder(key, cpos, pos) => kissbot_channel::Error::AttachmentPositionOutOfOrder(key,cpos,pos),
            Error::InvalidMessage(msg) => kissbot_channel::Error::InvalidMessage(msg),
            Error::IoError(e) => kissbot_channel::Error::IoError(e),
            Error::SendError(e) => kissbot_channel::Error::ExternalError(Box::new(e)),
            Error::RecvError(e) => kissbot_channel::Error::RecvError(e),
            Error::OneshotRecvError(e) => kissbot_channel::Error::OneshotRecvError(e),
            Error::JsonError(e) => kissbot_channel::Error::JsonError(e),
            Error::ImageError(e) => kissbot_channel::Error::ExternalError(Box::new(e)),
            Error::InternalError(msg) => kissbot_channel::Error::InternalError(msg),
            Error::ChannelError(e) => e,
            Error::KaiFileError(e) => kissbot_channel::Error::ExternalError(Box::new(e)),
        }
    }
}
