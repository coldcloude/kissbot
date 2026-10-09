//! MCP 客户端：单端点 Streamable HTTP（POST + JSON）请求-响应

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arcstr::ArcStr;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::McpError;
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
    Http { status: u16, message: ArcStr },
    #[error("MCP 远程错误 {0}")]
    Remote(#[from] McpError),
    #[error("MCP 响应解析失败: {0}")]
    Decode(#[from] serde_json::Error),
}

/// MCP 客户端：请求-响应模式，每个请求一个 HTTP POST
pub struct McpClient {
    endpoint: ArcStr,
    api_key: Option<ArcStr>,
    client_info: Implementation,
    next_id: AtomicI64,
    client: reqwest::Client,
}

impl McpClient {
    /// `endpoint` 为 MCP 端点完整 URL；`api_key` 非空时作为 `X-Api-Key` 请求头发送
    pub fn new(endpoint: &str, timeout_secs: u64, api_key: Option<&str>) -> Self {
        Self {
            endpoint: ArcStr::from(endpoint),
            api_key: api_key.map(ArcStr::from),
            client_info: Implementation::new(ArcStr::from("kissbot-mcp-client"), ArcStr::from(env!("CARGO_PKG_VERSION"))),
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
        self.send(METHOD_DISCOVER.clone(), None, params).await.1
    }

    /// `tools/list`：按可选组白名单查询工具
    pub async fn list_tools(
        &self,
        ancestors: Arc<Vec<ArcStr>>,
    ) -> Result<ToolsListResult, McpClientError> {
        let meta = RequestMeta::new(self.client_info.clone()).with_ancestors(ancestors);
        let params = ToolsListParams { cursor: None, meta };
        let params = serde_json::to_value(params)?;
        self.send(METHOD_TOOLS_LIST.clone(), None, params).await.1
    }

    /// `tools/call`：调用远端工具
    pub async fn call_tool(
        &self,
        name: ArcStr,
        arguments: Value,
        ancestors: Arc<Vec<ArcStr>>,
    ) -> (Value, Result<CallToolResult, McpClientError>) {
        let params = ToolsCallParams {
            name: name.clone(),
            arguments,
            meta: RequestMeta::new(self.client_info.clone()).with_ancestors(ancestors),
        };
        let params = match serde_json::to_value(&params) {
            Ok(r) => r,
            Err(e) => return (params.arguments, Err(McpClientError::from(e)))
        };
        let (params_recover, result) = self.send(METHOD_TOOLS_CALL.clone(), Some(name), params).await;
        let arguments_recover = match serde_json::from_value::<ToolsCallParams>(params_recover) {
            Ok(mut r) => r.arguments.take(),
            Err(_) => Value::Null,
        };
        (arguments_recover, result)
    }

    /// 发送 JSON-RPC 请求并解析结果；远程错误映射为 `Remote`
    async fn send<R: DeserializeOwned>(
        &self,
        method: ArcStr,
        name: Option<ArcStr>,
        params: Value,
    ) -> (Value, Result<R, McpClientError>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut builder = self
            .client
            .post(self.endpoint.as_str())
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION.as_str())
            .header("Mcp-Method", method.as_str());
        if let Some(name) = name.as_ref() {
            builder = builder.header("Mcp-Name", name.as_str());
        }
        if let Some(api_key) = &self.api_key {
            builder = builder.header("X-Api-Key", api_key.as_str());
        }
        let mut request = JsonRpcRequest::new(id, method, params);
        let response = builder.json(&request).send().await;
        let params_recover = request.params.take();
        let response = match response {
            Ok(r) => r,
            Err(e) => return (params_recover, Err(McpClientError::from(e)))
        };
        let status = response.status();
        let bytes = match response.bytes().await {
            Ok(r) => r,
            Err(e) => return (params_recover, Err(McpClientError::from(e)))
        };
        if !status.is_success() {
            return (params_recover, Err(McpClientError::Http {
                status: status.as_u16(),
                message: ArcStr::from(String::from_utf8_lossy(&bytes).into_owned()),
            }));
        }
        // HTTP 错误状态同样以 JSON-RPC 错误体返回：优先按其错误码上报
        let rpc = match serde_json::from_slice::<JsonRpcResponse>(&bytes) {
            Ok(r) => r,
            Err(e) => return (params_recover, Err(McpClientError::from(e)))
        };
        let result = match rpc.into_result() {
            Ok(r) => r,
            Err(e) => return (params_recover, Err(McpClientError::from(e)))
        };
        let result = match serde_json::from_value(result) {
            Ok(r) => r,
            Err(e) => return (params_recover, Err(McpClientError::from(e)))
        };
        (params_recover, Ok(result))
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
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    /// 服务端观测到的请求内容（验证请求侧字段确实发出）
    #[derive(Default)]
    struct Observed {
        ancestors: Mutex<Option<Vec<ArcStr>>>,
    }

    struct TestHandler {
        observed: Arc<Observed>,
    }

    #[async_trait]
    impl McpHandler for TestHandler {
        fn server_info(&self) -> std::sync::Arc<Implementation> {
            Arc::new(Implementation::new(ArcStr::from("test-server"), ArcStr::from("0.1.0")))
        }

        async fn discover(&self) -> DiscoverResult {
            DiscoverResult::new(None)
        }

        async fn list_tools(&self, params: ToolsListParams) -> Result<ToolsListResult, McpError> {
            *self.observed.ancestors.lock().unwrap() =
                params.meta.ancestors.as_ref().map(|a| a.as_ref().clone());
            let mut tool = Tool::new(ArcStr::from("read"), ArcStr::from("读"), json!({ "type": "object" }));
            tool.set_group(ArcStr::from("filesystem"));
            Ok(ToolsListResult::new(vec![Arc::new(tool)]))
        }

        async fn call_tool(&self, params: ToolsCallParams) -> Result<CallToolResult, McpError> {
            let value = json!({
                "echo": params.arguments,
                "ancestors": params.meta.ancestors,
            });
            Ok(CallToolResult {
                content: vec![crate::protocol::Content::text(ArcStr::from(value.to_string()))],
                structured_content: value,
                ..Default::default()
            })
        }
    }

    /// 起一个真实 HTTP MCP 服务端，返回（端点 URL, 停止句柄, 观测记录）
    async fn spawn_server() -> (ArcStr, tokio::task::JoinHandle<()>, Arc<Observed>) {
        let observed = Arc::new(Observed::default());
        let app = router(
            McpServer::new(Arc::new(TestHandler { observed: observed.clone() })),
            McpHttpConfig::default(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (ArcStr::from(format!("http://{}/mcp", addr)), handle, observed)
    }

    #[tokio::test]
    async fn client_discover_reads_server_info() {
        let (endpoint, _h, _observed) = spawn_server().await;
        let client = McpClient::new(&endpoint, 5, Some("k"));
        let result = client.discover().await.unwrap();
        assert_eq!(result.result_type, ResultType::Complete);
        assert_eq!(
            result.meta.unwrap().server_info.unwrap().name.as_str(),
            "test-server"
        );
    }

    #[tokio::test]
    async fn client_list_tools_sends_ancestors() {
        let (endpoint, _h, observed) = spawn_server().await;
        let client = McpClient::new(&endpoint, 5, None);
        let ancestors = vec![ArcStr::from("root")];
        let result = client.list_tools(Arc::new(ancestors.clone())).await.unwrap();
        assert_eq!(result.tools[0].name.as_str(), "read");
        assert_eq!(result.tools[0].group(), Some("filesystem"), "group 随工具携带");
        // 请求 `_meta` 的祖先链确实发出
        assert_eq!(observed.ancestors.lock().unwrap().clone(), Some(ancestors));
    }

    #[tokio::test]
    async fn client_call_tool_roundtrip() {
        let (endpoint, _h, _observed) = spawn_server().await;
        let client = McpClient::new(&endpoint, 5, None);
        let result = client
            .call_tool(ArcStr::from("read"), json!({ "path": "a.txt" }), Arc::new(Vec::new()))
            .await.1
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.structured_content["echo"]["path"], "a.txt");
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
