use axum::{
    extract::Request,
    response::{IntoResponse, Response},
};
use http::StatusCode;
use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::{Layer, Service};

use crate::{error::Error, extract_api_key, ApiKeyValidator};

/// 认证中间件的 Layer。
/// 通过 `AuthLayer::new(validator)` 创建，然后通过 `.layer()` 应用到 Router。
///
/// ```ignore
/// use axum::Router;
/// use kissbot_security::{AuthLayer, SimpleApiKeyValidator};
/// use std::sync::Arc;
///
/// let validator = Arc::new(SimpleApiKeyValidator::new("my-key".into()));
/// let app = Router::new()
///     .route("/api/...", axum::routing::post(handler))
///     .layer(AuthLayer::new(validator));
/// ```
#[derive(Clone)]
pub struct AuthLayer {
    validator: Arc<dyn ApiKeyValidator>,
}

impl AuthLayer {
    pub fn new(validator: Arc<dyn ApiKeyValidator>) -> Self {
        Self { validator }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            validator: self.validator.clone(),
        }
    }
}

/// 认证服务的内部实现。
#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    validator: Arc<dyn ApiKeyValidator>,
}

impl<S> Service<Request> for AuthService<S>
where
    S: Service<Request, Response = Response> + Send + 'static,
    S::Future: Send,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        match extract_api_key(request.headers()).and_then(|key| self.validator.validate(&key)) {
            Ok(()) => Box::pin(self.inner.call(request)),
            Err(e) => {
                let status_code = match e {
                    Error::MissingKey => StatusCode::UNAUTHORIZED,
                    Error::InvalidKey => StatusCode::UNAUTHORIZED,
                };
                let response = (
                    status_code,
                    axum::Json(serde_json::json!({
                        "success": false,
                        "error": e.to_string(),
                    })),
                ).into_response();
                Box::pin(async move { Ok(response) })
            }
        }
    }
}
