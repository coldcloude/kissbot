//! kissbot-memory-struct-abstract：摘要记忆实现组件（独立服务）。
//!
//! 订阅 memory-store 的新数据通知，把记忆切分为 case、生成 case 摘要，并按新消息定位 case 提供上下文。
//! 组件设计与职责边界见 docs/design/components-design/kissbot-memory-struct-abstract.md，
//! 对外接口与存储格式见 docs/spec/memory-struct-abstract.md。

mod api;
mod archive;
mod config;
mod error;
mod locate;
mod model;
mod notify;
mod records;
mod agent_role;
mod service;
mod stub_model;
mod tree;

#[cfg(test)]
mod stub;

use std::sync::Arc;

use kissbot_security::{AuthLayer, SecurityConfig, SimpleApiKeyValidator};
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::config::Config;
use crate::model::{DecisionModel, LlmModel};
use crate::agent_role::AgentRoleManager;
use crate::service::AbstractService;
use crate::stub_model::StubModel;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::get();
    let security = SecurityConfig::get();

    let root = config.storage_root();
    let manager = Arc::new(AgentRoleManager::new(root.clone()));
    // provider_type = "stub" 时使用确定性桩模型（集成测试与手动验证用，不调用外部模型）
    let model: Arc<dyn DecisionModel> = if config.provider_config.provider_type.as_str() == "stub" {
        info!("[abstract] provider_type=stub，使用桩模型（仅供测试/手动验证）");
        Arc::new(StubModel::new())
    } else {
        match LlmModel::new(config) {
            Ok(model) => Arc::new(model),
            Err(e) => {
                eprintln!("kissbot-memory-struct-abstract: 模型接入初始化失败: {}", e);
                std::process::exit(1);
            }
        }
    };
    let service = Arc::new(AbstractService::new(manager, model, config.max_batch_records));

    info!(
        "[abstract] memory root: {}, max batch records: {}, notify: {:?}",
        root.display(),
        config.max_batch_records,
        config.notify_url
    );

    // 启动恢复：加载磁盘上已有角色（未归档的原始记录留待首次查询时统一处理）
    match service.restore_roles().await {
        Ok(handles) => info!("[abstract] restored {} agent roles", handles.len()),
        Err(e) => error!("[abstract] restore agent roles failed: {}", e),
    }

    // 通知订阅：连接 memory-store 的通知服务
    let notify_service = service.clone();
    let notify_url = config.notify_url.clone();
    let reconnect_interval_secs = config.ws_reconnect_interval_secs;
    tokio::spawn(async move {
        if let Err(e) = notify::subscribe(notify_service, notify_url, reconnect_interval_secs).await {
            error!("[abstract] notification subscription stopped: {}", e);
        }
    });

    let app = api::create_router(service)
        .layer(AuthLayer::new(Arc::new(SimpleApiKeyValidator::new(security.api_key.clone()))));
    let addr = config.listen_addr();
    let listener = TcpListener::bind(addr.as_str()).await.unwrap();
    info!("kissbot-memory-struct-abstract listening on {}", addr);
    axum::serve(listener, app).await.unwrap();
}
