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
