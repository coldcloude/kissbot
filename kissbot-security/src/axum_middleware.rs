use std::sync::Arc;

use axum::{
    extract::Request,
    response::{IntoResponse, Response},
};
use http::StatusCode;
use tower::{Layer, Service};

use crate::{ApiKeyValidator, error::Error, extract_api_key};

/// 认证中间件，从请求头提取 X-Api-Key 并调用 validator 校验。
/// 认证失败返回 HTTP 401 + JSON 错误响应。
#[derive(Clone)]
pub struct AuthMiddleware {
    validator: Arc<dyn ApiKeyValidator>,
}

impl AuthMiddleware {
    pub fn new(validator: Arc<dyn ApiKeyValidator>) -> Self {
        Self { validator }
    }
}

impl<S> Layer<S> for AuthMiddleware {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            validator: self.validator.clone(),
        }
    }
}

/// 认证服务，包装内部 service，在转发请求前校验 API key。
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
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        // 检查 API key
        let auth_result = extract_api_key(request.headers())
            .and_then(|key| self.validator.validate(&key));

        if let Err(e) = auth_result {
            let status_code = match &e {
                Error::MissingKey => StatusCode::UNAUTHORIZED,
                Error::InvalidKey => StatusCode::UNAUTHORIZED,
            };
            let response = (
                status_code,
                axum::Json(serde_json::json!({
                    "success": false,
                    "error": e.to_string(),
                })),
            )
                .into_response();
            return Box::pin(async move { Ok(response) });
        }

        Box::pin(self.inner.call(request))
    }
}
