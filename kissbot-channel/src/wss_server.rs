use crate::error::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use kai_ws::{WsContext, WsHeartbeatHandler, WsProcessorInitializer, ws_handle_connection};
use tracing::{Level, info, span};
use std::{sync::{Arc, atomic::{AtomicU32, Ordering}}};
use tokio::{net::TcpListener, time::{Duration}};

const MSG_QUEUE_SIZE: usize = 100;

static INTERVAL: Duration = Duration::from_secs(10);

pub struct ConnectContext {
    pub connect_id: u32,
    pub ws_context: Arc<WsContext>,
    pub heartbeat_handler: Arc<WsHeartbeatHandler>,
}

pub struct WssServer {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
}

#[async_trait]
impl WsProcessorInitializer<Arc<ConnectContext>> for WssServer {
    async fn init(&mut self, ws_context: Arc<WsContext>) -> std::result::Result<Arc<ConnectContext>, kai_ws::Error> {
        let connect_id = self.global_connect_id.fetch_add(1, Ordering::Relaxed);
        let heartbeat_handler = Arc::new(WsHeartbeatHandler::new(INTERVAL, ws_context.clone()));
        let connect_context = Arc::new(ConnectContext {
            connect_id,
            ws_context,
            heartbeat_handler: heartbeat_handler.clone(),
        });
        heartbeat_handler.start();
        Ok(connect_context)
    }
}

impl WssServer {
    pub fn new() -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
        }
    }
    
    pub async fn start(&mut self, addr: &str) -> Result<()> {
        let span = span!(Level::INFO, "wss serverstart");
        let _enter = span.enter();
        let listener = TcpListener::bind(addr).await?;
        info!("WSS Server listening on: {}", addr);
        
        while let Ok((stream, _)) = listener.accept().await {
            let connect_id = self.global_connect_id.fetch_add(1, Ordering::Relaxed);
            let ws_context = ws_handle_connection(stream, self, MSG_QUEUE_SIZE).await?;
            self.connect_map.insert(connect_id, ws_context);
        }
        
        Ok(())
    }
}
