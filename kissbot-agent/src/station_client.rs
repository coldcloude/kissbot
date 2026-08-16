use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

use crate::config_manager::{McpConfig, ToolConfig};
use crate::types::{Error, Result};

/// 子 Station HTTP 通信客户端（子 Station 只能通过 HTTP 通信，不可本地调用）
/// 本轮骨架：请求/响应结构定义好（list_tools / list_mcps / call_tool），调用返回未实现错误；
/// 后续实现 HTTP 协议（查询元数据带 toolkit 白名单过滤 / 工具调用）
pub struct StationClient {
    #[allow(dead_code)] // 骨架期未消费：HTTP 协议落地后由请求发送消费（已知骨架警告）
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

    /// 查询子 Station 平铺后的工具元数据（可选 toolkit 白名单过滤；None = 全部）
    /// 骨架：返回未实现错误，后续由 HTTP 协议实现接入
    pub async fn list_tools(&self, _filter: Option<&HashSet<String>>) -> Result<Vec<ToolConfig>> {
        Err(Error::InternalError("子 Station 工具查询未实现（本轮骨架）".to_string()))
    }

    /// 查询子 Station 平铺后的 MCP 元数据（可选 toolkit 白名单过滤）
    /// 骨架：返回未实现错误（MCP 本轮整体不实现）
    pub async fn list_mcps(&self, _filter: Option<&HashSet<String>>) -> Result<Vec<McpConfig>> {
        Err(Error::InternalError("子 Station MCP 查询未实现（本轮骨架）".to_string()))
    }

    /// 调用子 Station 上的工具
    /// 骨架：返回未实现错误，后续由 HTTP 协议实现接入
    pub async fn call_tool(&self, _name: &str, _params: Value) -> Result<Value> {
        Err(Error::InternalError("子 Station 工具调用未实现（本轮骨架）".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn skeleton_returns_unimplemented() {
        let client = StationClient::new(5);
        let filter: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        assert!(client.list_tools(Some(&filter)).await.unwrap_err().to_string().contains("未实现"));
        assert!(client.list_mcps(None).await.unwrap_err().to_string().contains("未实现"));
        assert!(client.call_tool("read", serde_json::json!({})).await.unwrap_err().to_string().contains("未实现"));
    }
}
