use std::sync::Arc;

use kai_ws::WsHeaderFilter;

use crate::{ApiKeyValidator, error::Error, HEADER_API_KEY};

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
    fn filter(&self, request: &http::Request<()>) -> Result<(), kai_ws::Error> {
        let key = request
            .headers()
            .get(HEADER_API_KEY)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim())
            .filter(|v| !v.is_empty());

        let key = match key {
            Some(k) => k.to_string(),
            None => return Err(kai_ws::Error::UpgradeRejected(Error::MissingKey.to_string())),
        };

        match self.validator.validate(&key) {
            Ok(()) => Ok(()),
            Err(e) => Err(kai_ws::Error::UpgradeRejected(e.to_string())),
        }
    }
}
