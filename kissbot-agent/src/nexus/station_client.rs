use serde_json::Value;
use std::time::Duration;

use crate::nexus::types::{Result, Error};

/// Station 通信客户端
pub struct StationClient {
    client: reqwest::Client,
    _default_timeout: Duration,
}

impl StationClient {
    pub fn new(default_timeout_secs: u64) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(default_timeout_secs))
                .build()
                .unwrap_or_default(),
            _default_timeout: Duration::from_secs(default_timeout_secs),
        }
    }

    /// 调用 Station 上的工具
    /// 本期为骨架，仅返回未实现错误。后续由 ToolCallDispatcher 接入。
    pub async fn call_tool(
        &self,
        _station_url: &str,
        _tool_name: &str,
        _params: Value,
    ) -> Result<Value> {
        Err(Error::InternalError("工具调用未实现（本期骨架）".to_string()))
    }
}
