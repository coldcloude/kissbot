//! MCP 客户端：单端点 Streamable HTTP（POST + JSON）请求-响应

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::protocol::meta::{Implementation, RequestMeta};
use crate::protocol::{
    CallToolResult, DiscoverResult, JsonRpcRequest, JsonRpcResponse, ToolsCallParams,
    ToolsListParams, ToolsListResult, METHOD_DISCOVER, METHOD_TOOLS_CALL, METHOD_TOOLS_LIST,
    PROTOCOL_VERSION,
};

/// MCP 客户端错误
#[derive(Debug, thiserror::Error)]
pub enum McpClientError {
    #[error("MCP 传输失败: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("MCP 服务端 HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("MCP 远程错误 [{code}]: {message}")]
    Remote { code: i64, message: String },
    #[error("MCP 响应解析失败: {0}")]
    Decode(#[from] serde_json::Error),
}

/// MCP 客户端：请求-响应模式，每个请求一个 HTTP POST
pub struct McpClient {
    endpoint: Arc<String>,
    api_key: Option<Arc<String>>,
    client_info: Implementation,
    next_id: AtomicI64,
    client: reqwest::Client,
}

impl McpClient {
    /// `endpoint` 为 MCP 端点完整 URL；`api_key` 非空时作为 `X-Api-Key` 请求头发送
    pub fn new(endpoint: &str, timeout_secs: u64, api_key: Option<&str>) -> Self {
        Self {
            endpoint: Arc::new(endpoint.to_string()),
            api_key: api_key.map(|k| Arc::new(k.to_string())),
            client_info: Implementation::new("kissbot-mcp-client", env!("CARGO_PKG_VERSION")),
            next_id: AtomicI64::new(1),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .unwrap_or_default(),
        }
    }

    /// 覆盖客户端自报身份
    pub fn with_client_info(mut self, client_info: Implementation) -> Self {
        self.client_info = client_info;
        self
    }

    /// `server/discover`：查询服务端支持的协议版本、能力与身份
    pub async fn discover(&self) -> Result<DiscoverResult, McpClientError> {
        let params = serde_json::json!({ "_meta": RequestMeta::new(self.client_info.clone()) });
        self.send(METHOD_DISCOVER, None, params).await
    }

    /// `tools/list`：按可选组白名单查询工具
    pub async fn list_tools(
        &self,
        group: Option<&[String]>,
        ancestors: &[String],
    ) -> Result<ToolsListResult, McpClientError> {
        let params = ToolsListParams {
            cursor: None,
            group: group.map(|g| g.to_vec()),
            meta: RequestMeta::new(self.client_info.clone()).with_ancestors(ancestors),
        };
        let params = serde_json::to_value(params)?;
        self.send(METHOD_TOOLS_LIST, None, params).await
    }

    /// `tools/call`：调用远端工具
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        ancestors: &[String],
    ) -> Result<CallToolResult, McpClientError> {
        let params = ToolsCallParams {
            name: Arc::new(name.to_string()),
            arguments,
            meta: RequestMeta::new(self.client_info.clone()).with_ancestors(ancestors),
        };
        let params = serde_json::to_value(params)?;
        self.send(METHOD_TOOLS_CALL, Some(name), params).await
    }

    /// 发送 JSON-RPC 请求并解析结果；远程错误映射为 `Remote`
    async fn send<R: DeserializeOwned>(
        &self,
        method: &str,
        name: Option<&str>,
        params: Value,
    ) -> Result<R, McpClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);
        let mut builder = self
            .client
            .post(self.endpoint.as_str())
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .header("Mcp-Method", method);
        if let Some(name) = name {
            builder = builder.header("Mcp-Name", name);
        }
        if let Some(api_key) = &self.api_key {
            builder = builder.header("X-Api-Key", api_key.as_str());
        }
        let response = builder.json(&request).send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        // HTTP 错误状态同样以 JSON-RPC 错误体返回：优先按其错误码上报
        match serde_json::from_slice::<JsonRpcResponse>(&bytes) {
            Ok(rpc) => match rpc.into_result() {
                Ok(result) => Ok(serde_json::from_value(result)?),
                Err(e) => Err(McpClientError::Remote {
                    code: e.code,
                    message: e.message,
                }),
            },
            Err(_) => Err(McpClientError::Http {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&bytes).to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{router, McpHttpConfig};
    use crate::protocol::{McpError, ResultType, Tool, ToolsListResult};
    use crate::server::{McpHandler, McpServer};
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::net::TcpListener;

    struct TestHandler;

    #[async_trait]
    impl McpHandler for TestHandler {
        async fn discover(&self) -> DiscoverResult {
            DiscoverResult::tools_only(Implementation::new("test-server", "0.1.0"), None)
        }

        async fn list_tools(&self, params: ToolsListParams) -> Result<ToolsListResult, McpError> {
            let mut tool = Tool::new("read", "读", json!({ "type": "object" }));
            if let Some(group) = params.group.as_ref().and_then(|g| g.first()) {
                tool = tool.with_group(group.clone());
            }
            // 把收到的祖先链回填进工具元数据，验证请求 `_meta` 往返
            let mut object = tool
                .meta
                .as_ref()
                .and_then(|m| m.as_object())
                .cloned()
                .unwrap_or_default();
            object.insert("test/ancestors".to_string(), json!(params.meta.ancestors));
            tool.meta = Some(Arc::new(Value::Object(object)));
            Ok(ToolsListResult::new(vec![Arc::new(tool)]))
        }

        async fn call_tool(&self, params: ToolsCallParams) -> Result<CallToolResult, McpError> {
            if params.name.as_str() == "boom" {
                return Err(McpError::station_cycle("station-a"));
            }
            let value = json!({
                "echo": params.arguments,
                "ancestors": params.meta.ancestors,
            });
            Ok(CallToolResult {
                content: vec![crate::protocol::Content::text(value.to_string())],
                structured_content: value,
                ..Default::default()
            })
        }
    }

    /// 起一个真实 HTTP MCP 服务端，返回（端点 URL, 停止句柄）
    async fn spawn_server() -> (String, tokio::task::JoinHandle<()>) {
        let app = router(
            McpServer::new(Arc::new(TestHandler), Implementation::new("test-server", "0.1.0")),
            McpHttpConfig::default(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{}/mcp", addr), handle)
    }

    #[tokio::test]
    async fn client_discover_reads_server_info() {
        let (endpoint, _h) = spawn_server().await;
        let client = McpClient::new(&endpoint, 5, Some("k"));
        let result = client.discover().await.unwrap();
        assert_eq!(result.result_type, ResultType::Complete);
        assert_eq!(
            result.meta.server_info.as_ref().unwrap().name.as_str(),
            "test-server"
        );
    }

    #[tokio::test]
    async fn client_list_tools_sends_group_and_ancestors() {
        let (endpoint, _h) = spawn_server().await;
        let client = McpClient::new(&endpoint, 5, None);
        let group = vec!["filesystem".to_string()];
        let ancestors = vec!["root".to_string()];
        let result = client.list_tools(Some(&group), &ancestors).await.unwrap();
        assert_eq!(result.tools[0].name.as_str(), "read");
        assert_eq!(result.tools[0].group(), Some("filesystem"), "group 随工具携带");
        // 服务端把请求祖先链回填进工具元数据，验证 `_meta` 往返
        let meta = result.tools[0].meta.as_ref().unwrap();
        assert_eq!(meta["test/ancestors"], json!(ancestors));
    }

    #[tokio::test]
    async fn client_call_tool_roundtrip() {
        let (endpoint, _h) = spawn_server().await;
        let client = McpClient::new(&endpoint, 5, None);
        let result = client
            .call_tool("read", json!({ "path": "a.txt" }), &[])
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.structured_content["echo"]["path"], "a.txt");
    }

    #[tokio::test]
    async fn client_maps_remote_error() {
        let (endpoint, _h) = spawn_server().await;
        let client = McpClient::new(&endpoint, 5, None);
        let err = client.call_tool("boom", Value::Null, &[]).await.unwrap_err();
        match err {
            McpClientError::Remote { code, message } => {
                assert_eq!(code, crate::protocol::STATION_CYCLE);
                assert!(message.contains("station-a"));
            }
            other => panic!("期望 Remote 错误，实际: {:?}", other),
        }
    }

    #[tokio::test]
    async fn client_reports_unreachable_transport() {
        // 未监听端口：传输层错误
        let client = McpClient::new("http://127.0.0.1:1/mcp", 2, None);
        assert!(matches!(
            client.discover().await.unwrap_err(),
            McpClientError::Transport(_)
        ));
    }
}
