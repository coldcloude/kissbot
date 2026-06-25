mod error;
mod config;
mod record;
mod api;

use std::sync::Arc;
use tokio::net::TcpListener;
use kissbot_security::{AuthLayer, SecurityConfig, SimpleApiKeyValidator};

use crate::config::Config;
use crate::api::create_router;

#[tokio::main]
async fn main() {
    let config = Config::get();
    let security = SecurityConfig::get();

    let app = create_router()
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(security.api_key.clone()))));

    let addr = format!("{}:{}", config.listen_addr, config.listen_port);
    let listener = TcpListener::bind(&addr).await.unwrap();

    println!("kissbot-memory-store listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
