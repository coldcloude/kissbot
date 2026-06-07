mod error;
mod config;
mod record;
mod api;

use std::sync::Arc;
use tokio::net::TcpListener;
use axum::middleware;
use kissbot_security::{auth_middleware, SimpleApiKeyValidator, ApiKeyValidator};

use crate::config::Config;
use crate::api::create_router;

#[tokio::main]
async fn main() {
    let config = Config::get();

    let validator: Arc<dyn ApiKeyValidator> = Arc::new(SimpleApiKeyValidator::new(config.api_key.clone()));
    let app = create_router()
        .layer(middleware::from_fn(move |req, next| auth_middleware(req, next, validator.clone())));

    let addr = format!("{}:{}", config.listen_addr, config.listen_port);
    let listener = TcpListener::bind(&addr).await.unwrap();

    println!("kissbot-memory-store listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
