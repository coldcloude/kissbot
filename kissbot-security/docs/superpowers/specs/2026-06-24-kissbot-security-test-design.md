# kissbot-security 单元测试设计

为 `kissbot-security` crate 编写单元测试，覆盖 API key 提取、验证、WS 握手过滤、Axum 中间件。同时重构 `ws_filter.rs` 使其复用 `extract_api_key()`。

## 测试文件位置

遵循项目惯例，测试内联在各模块末尾的 `#[cfg(test)] mod tests` 块中。

## dev-dependencies

无需新增。`tower` 已在 dependencies 中，可在测试中用于构造 mock service。

## 重构

### ws_filter.rs

`ApiKeyWsFilter::filter()` 中的 header 提取逻辑替换为调用 `auth_types::extract_api_key()`。

当前代码手动实现了 header 提取（get → to_str → trim → filter empty），与 `auth_types.rs` 中的 `extract_api_key()` 完全相同。改为：

```rust
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
```

然后清理 `ws_filter.rs` 中不再需要的 `HEADER_API_KEY` 导入。

## 测试分组

### 1. auth_types — extract_api_key（4 个测试）

`#[test]`，同步。直接构造 `http::HeaderMap`。

| 测试 | 说明 |
|------|------|
| `test_extract_key_present` | header 存在且值合法，返回 `Ok(key)` |
| `test_extract_key_missing` | header 不存在，返回 `Err(Error::MissingKey)` |
| `test_extract_key_empty` | header 值为空字符串，返回 `Err(Error::MissingKey)` |
| `test_extract_key_whitespace` | header 值仅为空白字符，返回 `Err(Error::MissingKey)` |

### 2. validator — SimpleApiKeyValidator（3 个测试）

`#[test]`，同步。

| 测试 | 说明 |
|------|------|
| `test_validate_match` | key 与配置的一致，返回 `Ok(())` |
| `test_validate_mismatch` | key 不匹配，返回 `Err(Error::InvalidKey)` |
| `test_validate_empty` | 空字符串不匹配（不是特殊逻辑，只是边界情况） |

### 3. ws_filter — ApiKeyWsFilter（3 个测试）

`#[test]`，同步。通过简单的 mock validator 控制返回结果。

mock validator 用内联 struct 实现 `ApiKeyValidator` trait。

| 测试 | 说明 |
|------|------|
| `test_filter_accept` | header 存在、validator 验证通过，返回 `Ok(())` |
| `test_filter_missing_key` | header 不存在，返回 `Err(401)`，body 含错误描述 |
| `test_filter_invalid_key` | header 存在但 validator 拒绝，返回 `Err(401)` |

### 4. axum_middleware — AuthService（4 个测试）

`#[test]`，同步。使用 `tower::service_fn` 构造 mock inner service。

`AuthLayer` 需要 `Arc<dyn ApiKeyValidator>`，用内联 mock validator。

| 测试 | 说明 |
|------|------|
| `test_auth_service_accept` | 校验通过，请求被转发到 inner service，返回 200 |
| `test_auth_service_missing_key` | header 不存在，返回 401，不调用 inner service |
| `test_auth_service_invalid_key` | key 不匹配，返回 401 |
| `test_auth_service_error_body` | 验证 401 响应的 JSON body 包含 `success: false` 和 `error` 字段 |

## 总数

- 组 1: 4 个测试
- 组 2: 3 个测试
- 组 3: 3 个测试
- 组 4: 4 个测试
- **合计: 14 个测试用例**
