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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use http::HeaderMap;

    #[test]
    fn test_extract_key_present() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_API_KEY, "my-secret-key".parse().unwrap());
        let result = extract_api_key(&headers);
        assert_eq!(result.unwrap(), "my-secret-key");
    }

    #[test]
    fn test_extract_key_missing() {
        let headers = HeaderMap::new();
        let result = extract_api_key(&headers);
        assert!(matches!(result, Err(Error::MissingKey)));
    }

    #[test]
    fn test_extract_key_empty() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_API_KEY, "".parse().unwrap());
        let result = extract_api_key(&headers);
        assert!(matches!(result, Err(Error::MissingKey)));
    }

    #[test]
    fn test_extract_key_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_API_KEY, "   ".parse().unwrap());
        let result = extract_api_key(&headers);
        assert!(matches!(result, Err(Error::MissingKey)));
    }
}
