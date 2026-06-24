# kissbot-security 单元测试实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 kissbot-security crate 编写 14 个单元测试，覆盖 auth_types / validator / ws_filter / axum_middleware，同时重构 ws_filter.rs 复用 extract_api_key()

**Architecture:** 测试内联在各模块末尾的 `#[cfg(test)] mod tests` 中。不新增 dev-dependencies。组 1-3 用同步 `#[test]`，组 4 用同步 `#[test]` + `tower::service_fn` mock inner service。使用内联 mock struct 替代 `Arc<dyn ApiKeyValidator>`。

**Tech Stack:** Rust, tower, axum, http, kai-ws

**设计文档:** `kissbot-security/docs/superpowers/specs/2026-06-24-kissbot-security-test-design.md`

---

## 文件结构

- **Modify:** `kissbot-security/src/ws_filter.rs` — 重构 filter() 调用 extract_api_key()，添加测试
- **Modify:** `kissbot-security/src/auth_types.rs` — 添加测试
- **Modify:** `kissbot-security/src/validator.rs` — 添加测试
- **Modify:** `kissbot-security/src/axum_middleware.rs` — 添加测试

---

### Task 1: 重构 ws_filter.rs — 调用 extract_api_key()

**Files:**
- Modify: `kissbot-security/src/ws_filter.rs`

- [ ] **Step 1: 重构 filter() 方法**

替换 `ws_filter.rs` 中的 header 提取逻辑为调用 `extract_api_key()`：

```rust
use std::sync::Arc;

use kai_ws::WsHeaderFilter;

use crate::{extract_api_key, ApiKeyValidator, error::Error};

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
```

注意：导入从 `use crate::{ApiKeyValidator, error::Error, HEADER_API_KEY};` 变为 `use crate::{extract_api_key, ApiKeyValidator, error::Error};` — 去掉 `HEADER_API_KEY`。

- [ ] **Step 2: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-security && cargo check 2>&1
```

Expected: 编译通过，无错误无警告。

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-security && git add src/ws_filter.rs && git commit -m "refactor: ApiKeyWsFilter 复用 extract_api_key()

替换 filter() 中手写的 header 提取逻辑为调用 auth_types::extract_api_key()，
消除重复代码。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: auth_types 测试（组 1 — 4 个测试）

**Files:**
- Modify: `kissbot-security/src/auth_types.rs` — 末尾添加 `#[cfg(test)]`

- [ ] **Step 1: 编写 4 个测试函数**

在 `auth_types.rs` 末尾添加：

```rust
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
```

注意：`http::HeaderMap` 和 `http::header::HeaderValue` 的 `parse()` 方法。`HeaderValue` 的 `from_str` 通过 `.parse().unwrap()` 或 `HeaderValue::from_static()` 构造。

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-security && cargo test test_extract_key -- --nocapture
```

Expected: 4 tests passed

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-security && git add src/auth_types.rs && git commit -m "test: auth_types extract_api_key 测试（组1）

添加4个测试：test_extract_key_present / _missing / _empty / _whitespace
覆盖 header 存在、缺失、空值、纯空白字符的场景。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: validator 测试（组 2 — 3 个测试）

**Files:**
- Modify: `kissbot-security/src/validator.rs` — 末尾添加 `#[cfg(test)]`

- [ ] **Step 1: 编写 3 个测试函数**

在 `validator.rs` 末尾添加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_validate_match() {
        let validator = SimpleApiKeyValidator::new(Arc::new("secret".to_string()));
        assert!(validator.validate("secret").is_ok());
    }

    #[test]
    fn test_validate_mismatch() {
        let validator = SimpleApiKeyValidator::new(Arc::new("secret".to_string()));
        let result = validator.validate("wrong-key");
        assert!(matches!(result, Err(crate::Error::InvalidKey)));
    }

    #[test]
    fn test_validate_empty() {
        let validator = SimpleApiKeyValidator::new(Arc::new("secret".to_string()));
        let result = validator.validate("");
        assert!(matches!(result, Err(crate::Error::InvalidKey)));
    }
}
```

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-security && cargo test test_validate -- --nocapture
```

Expected: 3 tests passed

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-security && git add src/validator.rs && git commit -m "test: SimpleApiKeyValidator 测试（组2）

添加3个测试：test_validate_match / _mismatch / _empty
覆盖正确匹配、不匹配、空字符串边界情况。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 4: ws_filter 测试（组 3 — 3 个测试）

**Files:**
- Modify: `kissbot-security/src/ws_filter.rs` — 末尾添加 `#[cfg(test)]`

- [ ] **Step 1: 编写 mock validator 和 3 个测试函数**

在 `ws_filter.rs` 末尾添加：

```rust
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
```

注意：`Error` 需要 `Clone` trait 才能用于 `self.result.clone()`——已在 error.rs 中 `#[derive(Clone)]`。

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-security && cargo test test_filter -- --nocapture
```

Expected: 3 tests passed

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-security && git add src/ws_filter.rs && git commit -m "test: ApiKeyWsFilter 测试（组3）

添加3个测试：test_filter_accept / _missing_key / _invalid_key
使用内联 MockValidator 控制验证结果，验证 filter 的通过/拒绝行为。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 5: axum_middleware 测试（组 4 — 4 个测试）

**Files:**
- Modify: `kissbot-security/src/axum_middleware.rs` — 末尾添加 `#[cfg(test)]`

- [ ] **Step 1: 编写 mock validator 和 4 个测试函数**

在 `axum_middleware.rs` 末尾添加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use axum::{body::Body, body::Bytes};
    use http::Request;

    /// Mock validator that returns a preset result.
    struct MockValidator {
        result: Result<(), Error>,
    }

    impl ApiKeyValidator for MockValidator {
        fn validate(&self, _key: &str) -> Result<(), Error> {
            self.result.clone()
        }
    }

    /// Mock inner service: records whether it was called and returns 200.
    async fn mock_inner(_req: Request<Body>) -> Result<Response, std::convert::Infallible> {
        Ok(http::Response::builder()
            .status(200)
            .body(Body::empty())
            .unwrap())
    }

    fn make_auth_service(validator_result: Result<(), Error>) -> AuthService<
        tower::util::BoxCloneService<Request<Body>, Response, std::convert::Infallible>
    > {
        let validator: Arc<dyn ApiKeyValidator> = Arc::new(MockValidator { result: validator_result });
        let inner = tower::service_fn(mock_inner);
        AuthService {
            inner: inner.boxed_clone(),
            validator,
        }
    }

    #[tokio::test]
    async fn test_auth_service_accept() {
        let mut svc = make_auth_service(Ok(()));
        let request = Request::builder()
            .uri("/api/test")
            .header(crate::HEADER_API_KEY, "valid-key")
            .body(Body::empty())
            .unwrap();
        let response = svc.call(request).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_auth_service_missing_key() {
        let mut svc = make_auth_service(Ok(()));
        let request = Request::builder()
            .uri("/api/test")
            .body(Body::empty())
            .unwrap();
        let response = svc.call(request).await.unwrap();
        assert_eq!(response.status(), 401);
    }

    #[tokio::test]
    async fn test_auth_service_invalid_key() {
        let mut svc = make_auth_service(Err(Error::InvalidKey));
        let request = Request::builder()
            .uri("/api/test")
            .header(crate::HEADER_API_KEY, "wrong-key")
            .body(Body::empty())
            .unwrap();
        let response = svc.call(request).await.unwrap();
        assert_eq!(response.status(), 401);
    }

    #[tokio::test]
    async fn test_auth_service_error_body() {
        let mut svc = make_auth_service(Err(Error::InvalidKey));
        let request = Request::builder()
            .uri("/api/test")
            .header(crate::HEADER_API_KEY, "wrong-key")
            .body(Body::empty())
            .unwrap();
        let response = svc.call(request).await.unwrap();
        assert_eq!(response.status(), 401);
        let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], false);
        assert_eq!(json["error"], "invalid api key");
    }
}
```

注意：
- 需要 `#[tokio::test]` — `tokio` 不是 kissbot-security 的直接 dependency，但 `axum` 依赖 `tokio`，所以它是 transitive dependency。在 edition 2024 中，`cargo test` 可以解析 transitive 依赖的宏。但如果编译报错，需要在 `Cargo.toml` 的 `[dev-dependencies]` 添加 `tokio = { version = "1", features = ["rt", "macros"] }`。
- `Error` 实现了 `Clone` (`#[derive(Clone)]`)，所以 `result.clone()` 可用。
- `tower::BoxCloneService` 需要 `tower` 的 `util` 模块（已在 dependencies 中）。
- `axum::body::to_bytes` 用于读取响应 body。

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/kissbot-security && cargo test test_auth_service -- --nocapture
```

Expected: 4 tests passed

如果报 `tokio` 宏找不到，添加 dev-dependency 到 `Cargo.toml`：
```toml
[dev-dependencies]
tokio = { version = "1", features = ["rt", "macros"] }
```

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-security && git add src/axum_middleware.rs && git commit -m "test: AuthService 中间件测试（组4）

添加4个测试：test_auth_service_accept / _missing_key / _invalid_key / _error_body
使用 tower::service_fn 构造 mock inner service，验证校验通过/拒绝
及错误响应 JSON body 格式。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 6: 全量运行并验证

- [ ] **Step 1: 全量运行所有测试**

```bash
cd /home/admin/project/kissbot/kissbot-security && cargo test 2>&1
```

Expected: 14 tests passed

- [ ] **Step 2: 提交最终确认**

```bash
cd /home/admin/project/kissbot/kissbot-security && git add Cargo.toml src/auth_types.rs src/validator.rs src/ws_filter.rs src/axum_middleware.rs && git commit -m "test: 全量14个测试全部通过

Co-Authored-By: deepseek-v4-flash"
```
