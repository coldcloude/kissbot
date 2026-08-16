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
    /// 骨架：返回空集合（子 Station 无工具可提供），HTTP 协议实现后返回真实列表
    pub async fn list_tools(&self, _filter: Option<&HashSet<String>>) -> Result<Vec<ToolConfig>> {
        Ok(Vec::new())
    }

    /// 查询子 Station 平铺后的 MCP 元数据（可选 toolkit 白名单过滤）
    /// 骨架：返回空集合（MCP 本轮整体不实现）
    pub async fn list_mcps(&self, _filter: Option<&HashSet<String>>) -> Result<Vec<McpConfig>> {
        Ok(Vec::new())
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
    async fn skeleton_list_empty_call_unimplemented() {
        let client = StationClient::new(5);
        let filter: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        assert!(client.list_tools(Some(&filter)).await.unwrap().is_empty(), "骨架查询返回空集合");
        assert!(client.list_mcps(None).await.unwrap().is_empty(), "骨架 MCP 查询返回空集合");
        assert!(client.call_tool("read", serde_json::json!({})).await.unwrap_err().to_string().contains("未实现"), "骨架调用保持未实现");
    }
}
