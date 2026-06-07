use thiserror::Error;

/// 认证错误类型。
#[derive(Error, Debug, Clone)]
pub enum Error {
    /// 请求未携带 API key header。
    #[error("missing api key")]
    MissingKey,

    /// API key 不匹配。
    #[error("invalid api key")]
    InvalidKey,
}
