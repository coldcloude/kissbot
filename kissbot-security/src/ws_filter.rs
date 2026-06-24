use std::sync::Arc;

use kai_ws::WsHeaderFilter;

use crate::{extract_api_key, ApiKeyValidator};

/// API key WS 握手过滤器。
/// 实现 kai-ws 的 WsHeaderFilter trait，在 WS 握手阶段校验 X-Api-Key header。
pub struct ApiKeyWsFilter {
    validator: Arc<dyn ApiKeyValidator>,
}

impl ApiKeyWsFilter {
    pub fn new(validator: Arc<dyn ApiKeyValidator>) -> Self {
        Self { validator }
    }
}

impl WsHeaderFilter for ApiKeyWsFilter {
    fn filter(&self, request: &http::Request<()>) -> std::result::Result<(), http::Response<Option<String>>> {
        let key = match extract_api_key(request.headers()) {
            Ok(k) => k,
            Err(e) => return Err(
                http::Response::builder()
                    .status(http::StatusCode::UNAUTHORIZED)
                    .body(Some(e.to_string()))
                    .unwrap()
            ),
        };

        match self.validator.validate(&key) {
            Ok(()) => Ok(()),
            Err(e) => Err(
                http::Response::builder()
                    .status(http::StatusCode::UNAUTHORIZED)
                    .body(Some(e.to_string()))
                    .unwrap()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    /// Mock validator that returns a preset result.
    struct MockValidator {
        result: Result<(), Error>,
    }

    impl ApiKeyValidator for MockValidator {
        fn validate(&self, _key: &str) -> Result<(), Error> {
            self.result.clone()
        }
    }

    #[test]
    fn test_filter_accept() {
        let filter = ApiKeyWsFilter::new(Arc::new(MockValidator { result: Ok(()) }));
        let request = http::Request::builder()
            .uri("ws://example.com/ws")
            .header(crate::HEADER_API_KEY, "valid-key")
            .body(())
            .unwrap();
        let result = filter.filter(&request);
        assert!(result.is_ok());
    }

    #[test]
    fn test_filter_missing_key() {
        let filter = ApiKeyWsFilter::new(Arc::new(MockValidator { result: Ok(()) }));
        let request = http::Request::builder()
            .uri("ws://example.com/ws")
            .body(())
            .unwrap();
        let result = filter.filter(&request);
        assert!(result.is_err());
        let err_response = result.unwrap_err();
        assert_eq!(err_response.status(), http::StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_filter_invalid_key() {
        let filter = ApiKeyWsFilter::new(Arc::new(MockValidator {
            result: Err(Error::InvalidKey),
        }));
        let request = http::Request::builder()
            .uri("ws://example.com/ws")
            .header(crate::HEADER_API_KEY, "wrong-key")
            .body(())
            .unwrap();
        let result = filter.filter(&request);
        assert!(result.is_err());
        let err_response = result.unwrap_err();
        assert_eq!(err_response.status(), http::StatusCode::UNAUTHORIZED);
    }
}
