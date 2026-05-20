use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("bin parse error: {0}")]
    BinParse(#[from] std::array::TryFromSliceError),
}

pub type Result<T> = std::result::Result<T, ApiError>;
