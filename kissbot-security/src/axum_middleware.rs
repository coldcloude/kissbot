use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use http::StatusCode;

use crate::{error::Error, extract_api_key, ApiKeyValidator};
use std::sync::Arc;

/// 认证中间件函数。通过 axum::middleware::from_fn 使用。
///
/// ```ignore
/// use axum::{Router, middleware};
/// use kissbot_security::{auth_middleware, SimpleApiKeyValidator};
/// use std::sync::Arc;
///
/// let validator = Arc::new(SimpleApiKeyValidator::new("my-key".into()));
/// let app = Router::new()
///     .route("/api/...", axum::routing::post(handler))
///     .route_layer(middleware::from_fn(move |req, next| auth_middleware(req, next, validator.clone())));
/// ```
pub async fn auth_middleware(
    request: Request,
    next: Next,
    validator: Arc<dyn ApiKeyValidator>,
) -> Response {
    match extract_api_key(request.headers()).and_then(|key| validator.validate(&key)) {
        Ok(()) => next.run(request).await,
        Err(e) => {
            let status_code = match e {
                Error::MissingKey => StatusCode::UNAUTHORIZED,
                Error::InvalidKey => StatusCode::UNAUTHORIZED,
            };
            (status_code, axum::Json(serde_json::json!({
                "success": false,
                "error": e.to_string(),
            })))
                .into_response()
        }
    }
}
