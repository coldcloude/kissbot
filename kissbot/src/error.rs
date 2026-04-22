use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Duplicated document key: {0}")]
    DuplicatedDocumentKey(String),
}

pub type Result<T> = std::result::Result<T, Error>;
