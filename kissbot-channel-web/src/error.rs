use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Group not found: {0}")]
    GroupNotFound(String),

    #[error("User not found: {0}")]
    UserNotFound(String),

    #[error("Admin-user group cannot be modified: {0}")]
    AdminUserGroupNotModifiable(String),

    #[error("Admin-user group cannot be deleted: {0}")]
    AdminUserGroupNotDeletable(String),

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

    #[error("Channel error: {0}")]
    ChannelError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

pub type Result<T> = std::result::Result<T, Error>;
