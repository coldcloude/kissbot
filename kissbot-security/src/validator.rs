use std::sync::Arc;

use crate::error::Error;

/// API key 校验器 trait。
/// 各进程可通过实现此 trait 自定义校验逻辑。
pub trait ApiKeyValidator: Send + Sync {
    /// 校验 API key，通过返回 Ok(())，失败返回 Error。
    fn validate(&self, key: &str) -> Result<(), Error>;
}

/// 简单的字符串比对 API key 校验器。
/// 持有预配置的 key，与请求中的 key 直接比对。
pub struct SimpleApiKeyValidator {
    configured_key: Arc<String>,
}

impl SimpleApiKeyValidator {
    pub fn new(configured_key: Arc<String>) -> Self {
        Self { configured_key }
    }
}

impl ApiKeyValidator for SimpleApiKeyValidator {
    fn validate(&self, key: &str) -> Result<(), Error> {
        if key == self.configured_key.as_str() {
            Ok(())
        } else {
            Err(Error::InvalidKey)
        }
    }
}
