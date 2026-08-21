use crate::error::{Error, Result};

/// 校验代号：仅字母/数字/下划线，且非空（等价于 `^[A-Za-z0-9_]+$`）。
/// 用于 agent_id / role_name 等代号字段的写入入口。
pub fn validate_code(code: &str) -> Result<()> {
    let valid = !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidCode(code.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_code_accepts_valid() {
        assert!(validate_code("alice_01").is_ok());
        assert!(validate_code("Alice").is_ok());
        assert!(validate_code("aBc_123").is_ok());
    }

    #[test]
    fn test_validate_code_rejects_invalid() {
        assert!(matches!(validate_code("a b"), Err(Error::InvalidCode(_))));
        assert!(matches!(validate_code(""), Err(Error::InvalidCode(_))));
        assert!(matches!(validate_code("a-b"), Err(Error::InvalidCode(_))));
        assert!(matches!(validate_code("a.b"), Err(Error::InvalidCode(_))));
        assert!(matches!(validate_code("你好"), Err(Error::InvalidCode(_))));
    }
}
