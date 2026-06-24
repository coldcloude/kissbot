mod error;
#[cfg(test)]
mod test_util;
mod config;
mod agent;
mod ego_md;
mod search;
mod individual_recognition;
mod role_play;
mod api;

use std::sync::Arc;
use tokio::net::TcpListener;
use kissbot_security::{AuthLayer, SimpleApiKeyValidator};

use crate::config::Config;
use crate::api::create_router;

#[tokio::main]
async fn main() {
    let config = Config::get();

    let app = create_router()
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(Arc::new(config.api_key.clone())))));

    let addr = format!("{}:{}", config.listen_addr, config.listen_port);
    let listener = TcpListener::bind(&addr).await.unwrap();

    println!("kissbot-memory-ego listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
