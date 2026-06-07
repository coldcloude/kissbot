use crate::error::Error;

/// HTTP header 名称常量，用于传递 API key。
pub const HEADER_API_KEY: &str = "X-Api-Key";

/// 从 HTTP 请求头中提取 API key。
/// 返回 `Err(Error::MissingKey)` 如果 header 不存在或值为空。
pub fn extract_api_key(headers: &http::HeaderMap) -> Result<String, Error> {
    let value = headers
        .get(HEADER_API_KEY)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim())
        .filter(|v| !v.is_empty());

    match value {
        Some(key) => Ok(key.to_string()),
        None => Err(Error::MissingKey),
    }
}
