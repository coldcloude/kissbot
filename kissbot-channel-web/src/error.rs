use thiserror::Error;

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

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Image error: {0}")]
    ImageError(#[from] image::ImageError),

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Channel error: {0}")]
    ChannelError(#[from] kissbot_channel::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for kissbot_channel::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::ConfigError(msg) => kissbot_channel::Error::InternalError(format!("config: {}", msg)),
            Error::GroupNotFound(msg) => kissbot_channel::Error::GroupNotFound(msg),
            Error::UserNotFound(msg) => kissbot_channel::Error::UserNotFound(msg),
            Error::AttachmentNotFound(msg) => kissbot_channel::Error::AttachmentNotFound(msg),
            Error::InvalidMessage(msg) => kissbot_channel::Error::InvalidMessage(msg),
            Error::IoError(e) => kissbot_channel::Error::IoError(e),
            Error::JsonError(e) => kissbot_channel::Error::JsonError(e),
            Error::ImageError(e) => kissbot_channel::Error::ExternalError(Box::new(e)),
            Error::InternalError(msg) => kissbot_channel::Error::InternalError(msg),
            Error::ChannelError(e) => e,
        }
    }
}
