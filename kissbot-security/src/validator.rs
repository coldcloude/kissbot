use arcstr::ArcStr;

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
    configured_key: ArcStr,
}

impl SimpleApiKeyValidator {
    pub fn new(configured_key: ArcStr) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_match() {
        let validator = SimpleApiKeyValidator::new(ArcStr::from("secret"));
        assert!(validator.validate("secret").is_ok());
    }

    #[test]
    fn test_validate_mismatch() {
        let validator = SimpleApiKeyValidator::new(ArcStr::from("secret"));
        let result = validator.validate("wrong-key");
        assert!(matches!(result, Err(crate::Error::InvalidKey)));
    }

    #[test]
    fn test_validate_empty() {
        let validator = SimpleApiKeyValidator::new(ArcStr::from("secret"));
        let result = validator.validate("");
        assert!(matches!(result, Err(crate::Error::InvalidKey)));
    }
}
