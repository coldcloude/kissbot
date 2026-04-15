use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Parse error: {0}")]
    Parse(#[from] chrono::ParseError),

    #[error("Config error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;
