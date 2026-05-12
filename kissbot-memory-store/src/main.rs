mod error;
mod config;
mod record;
mod api;

use tokio::net::TcpListener;

use crate::config::Config;
use crate::api::create_router;

#[tokio::main]
async fn main() {
    let config = Config::get();
    
    let app = create_router();
    
    let addr = format!("{}:{}", config.listen_addr, config.listen_port);
    let listener = TcpListener::bind(&addr).await.unwrap();
    
    println!("kissbot-memory-store listening on {}", addr);
    
    axum::serve(listener, app).await.unwrap();
}
