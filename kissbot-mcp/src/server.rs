//! MCP 服务端：方法分发与 `_meta` 校验

use std::sync::Arc;

use arcstr::ArcStr;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::protocol::discover::DiscoverParams;
use crate::protocol::meta::Implementation;
use crate::protocol::{
    CallToolResult, DiscoverResult, JsonRpcRequest, JsonRpcResponse, McpError, ResultType,
    ToolsCallParams, ToolsListParams, ToolsListResult, METHOD_DISCOVER, METHOD_TOOLS_CALL,
    METHOD_TOOLS_LIST, PROTOCOL_VERSION,
};

/// MCP 服务端业务处理器：由使用方实现（如 Station）
#[async_trait]
pub trait McpHandler: Send + Sync + 'static {
    /// 服务端能力与身份（协议版本与服务端身份由 `McpServer` 统一填充）
    async fn discover(&self) -> DiscoverResult;

    /// 工具列表（返回的工具定义可携带 kissbot 分组）
    async fn list_tools(&self, params: ToolsListParams) -> Result<ToolsListResult, McpError>;

    /// 工具调用
    async fn call_tool(&self, params: ToolsCallParams) -> Result<CallToolResult, McpError>;
}

/// MCP 服务端：把 JSON-RPC 请求分发到 `McpHandler`，并统一填充 `resultType` 与服务端身份
pub struct McpServer<H: McpHandler> {
    handler: Arc<H>,
    server_info: Implementation,
    instructions: Option<ArcStr>,
    supported_versions: Vec<ArcStr>,
}

impl<H: McpHandler> McpServer<H> {
    pub fn new(handler: Arc<H>, server_info: Implementation) -> Self {
        Self {
            handler,
            server_info,
            instructions: None,
            supported_versions: vec![ArcStr::from(PROTOCOL_VERSION)],
        }
    }

    /// 设置 `server/discover` 返回的自然语言说明
    pub fn with_instructions(mut self, instructions: impl Into<ArcStr>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn server_info(&self) -> &Implementation {
        &self.server_info
    }

    pub fn supported_versions(&self) -> &[ArcStr] {
        &self.supported_versions
    }

    /// 处理一条 JSON-RPC 请求
    pub async fn handle(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id.clone();
        match self.dispatch(request).await {
            Ok(result) => JsonRpcResponse::success(id, result),
            Err(e) => JsonRpcResponse::error(Some(id), e),
        }
    }

    async fn dispatch(&self, request: JsonRpcRequest) -> Result<Value, McpError> {
        request.validate()?;
        match request.method.as_str() {
            METHOD_DISCOVER => {
                let params: DiscoverParams = parse_params(&request)?;
                self.check_version(&params.meta)?;
                let mut result = self.handler.discover().await;
                result.result_type = ResultType::Complete;
                result.supported_versions = self.supported_versions.clone();
                // 服务端显式配置了说明才覆盖处理器给出的说明
                if self.instructions.is_some() {
                    result.instructions = self.instructions.clone();
                }
                result = result.with_server_info(self.server_info.clone());
                to_value(result)
            }
            METHOD_TOOLS_LIST => {
                let params: ToolsListParams = parse_params(&request)?;
                self.check_version(&params.meta)?;
                let mut result = self.handler.list_tools(params).await?;
                result.result_type = ResultType::Complete;
                result = result.with_server_info(self.server_info.clone());
                to_value(result)
            }
            METHOD_TOOLS_CALL => {
                let params: ToolsCallParams = parse_params(&request)?;
                self.check_version(&params.meta)?;
                let mut result = self.handler.call_tool(params).await?;
                result.result_type = ResultType::Complete;
                result = result.with_server_info(self.server_info.clone());
                to_value(result)
            }
            other => Err(McpError::method_not_found(other)),
        }
    }

    /// 协议版本协商：请求版本须在服务端支持列表内
    fn check_version(&self, meta: &crate::protocol::RequestMeta) -> Result<(), McpError> {
        if self
            .supported_versions
            .iter()
            .any(|v| v.as_str() == meta.protocol_version.as_str())
        {
            return Ok(());
        }
        Err(McpError::unsupported_protocol_version(
            meta.protocol_version.as_str(),
            &self.supported_versions,
        ))
    }
}

fn parse_params<T: DeserializeOwned>(request: &JsonRpcRequest) -> Result<T, McpError> {
    serde_json::from_value(request.params.clone()).map_err(|e| {
        McpError::invalid_params(format!("{} 参数非法: {}", request.method.as_str(), e))
    })
}

fn to_value<T: Serialize>(value: T) -> Result<Value, McpError> {
    serde_json::to_value(value).map_err(|e| McpError::internal(format!("结果序列化失败: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::meta::ResponseMeta;
    use crate::protocol::Tool;
    use serde_json::json;

    struct EchoHandler;

    fn meta_value() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
            "io.modelcontextprotocol/clientCapabilities": {}
        })
    }

    #[async_trait]
    impl McpHandler for EchoHandler {
        async fn discover(&self) -> DiscoverResult {
            DiscoverResult {
                result_type: ResultType::Complete,
                supported_versions: vec![],
                capabilities: Default::default(),
                instructions: None,
                meta: Some(ResponseMeta::default()),
            }
        }

        async fn list_tools(&self, _params: ToolsListParams) -> Result<ToolsListResult, McpError> {
            let mut tool = Tool::new("echo", "回声", json!({ "type": "object" }));
            tool.set_group("g1");
            Ok(ToolsListResult::new(vec![Arc::new(tool)]))
        }

        async fn call_tool(&self, params: ToolsCallParams) -> Result<CallToolResult, McpError> {
            if params.name.as_str() == "cycle" {
                return Err(McpError::station_cycle("station-a"));
            }
            Ok(CallToolResult::text(params.arguments.to_string()))
        }
    }

    #[tokio::test]
    async fn handle_discover_fills_server_identity_and_versions() {
        let server = McpServer::new(
            Arc::new(EchoHandler),
            Implementation::new("kissbot-test", "0.1.0"),
        )
        .with_instructions("测试服务端");
        let req = JsonRpcRequest::new(1, METHOD_DISCOVER, json!({ "_meta": meta_value() }));
        let resp = server.handle(req).await;
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["result"]["resultType"], "complete");
        assert_eq!(v["result"]["supportedVersions"][0], PROTOCOL_VERSION);
        assert_eq!(v["result"]["instructions"], "测试服务端");
        assert_eq!(
            v["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "kissbot-test"
        );
    }

    #[tokio::test]
    async fn handle_tools_list_keeps_group_extension() {
        let server = McpServer::new(Arc::new(EchoHandler), Implementation::new("s", "1"));
        let req = JsonRpcRequest::new(1, METHOD_TOOLS_LIST, json!({ "_meta": meta_value() }));
        let resp = server.handle(req).await;
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["result"]["tools"][0]["name"], "echo");
        assert_eq!(v["result"]["tools"][0]["_meta"]["kissbot/group"], "g1");
    }

    #[tokio::test]
    async fn handle_tools_call_maps_handler_error_to_jsonrpc_error() {
        let server = McpServer::new(Arc::new(EchoHandler), Implementation::new("s", "1"));
        let req = JsonRpcRequest::new(
            2,
            METHOD_TOOLS_CALL,
            json!({ "name": "cycle", "arguments": {}, "_meta": meta_value() }),
        );
        let v = serde_json::to_value(&server.handle(req).await).unwrap();
        assert_eq!(v["error"]["code"], crate::protocol::STATION_CYCLE);
        assert!(v.get("result").is_none());
    }

    #[tokio::test]
    async fn handle_unknown_method_returns_method_not_found() {
        let server = McpServer::new(Arc::new(EchoHandler), Implementation::new("s", "1"));
        let req = JsonRpcRequest::new(1, "tools/unknown", json!({ "_meta": meta_value() }));
        let v = serde_json::to_value(&server.handle(req).await).unwrap();
        assert_eq!(v["error"]["code"], crate::protocol::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn handle_missing_meta_returns_invalid_params() {
        let server = McpServer::new(Arc::new(EchoHandler), Implementation::new("s", "1"));
        let req = JsonRpcRequest::new(1, METHOD_TOOLS_LIST, json!({}));
        let v = serde_json::to_value(&server.handle(req).await).unwrap();
        assert_eq!(v["error"]["code"], crate::protocol::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn handle_unsupported_version_returns_negotiation_error() {
        let server = McpServer::new(Arc::new(EchoHandler), Implementation::new("s", "1"));
        let req = JsonRpcRequest::new(
            1,
            METHOD_DISCOVER,
            json!({ "_meta": {
                "io.modelcontextprotocol/protocolVersion": "1999-01-01",
                "io.modelcontextprotocol/clientCapabilities": {}
            }}),
        );
        let v = serde_json::to_value(&server.handle(req).await).unwrap();
        assert_eq!(
            v["error"]["code"],
            crate::protocol::UNSUPPORTED_PROTOCOL_VERSION
        );
        assert_eq!(
            v["error"]["data"]["supportedVersions"][0],
            PROTOCOL_VERSION
        );
    }

    /// 结果生产方给什么 content，服务端就原样回传：不解释、不合成、不改写
    #[tokio::test]
    async fn handle_tools_call_relays_content_verbatim() {
        struct RelayHandler;

        #[async_trait]
        impl McpHandler for RelayHandler {
            async fn discover(&self) -> DiscoverResult {
                DiscoverResult::new(Implementation::new("relay", "1"), None)
            }

            async fn list_tools(&self, _: ToolsListParams) -> Result<ToolsListResult, McpError> {
                Ok(ToolsListResult::new(vec![]))
            }

            async fn call_tool(&self, _: ToolsCallParams) -> Result<CallToolResult, McpError> {
                let content = json!([
                    { "type": "text", "text": "说明" },
                    { "type": "image", "data": "eHl6", "mimeType": "image/png" },
                    { "type": "resource", "resource": { "uri": "file:///a.txt", "text": "内嵌" } }
                ]);
                Ok(CallToolResult {
                    content: serde_json::from_value(content).unwrap(),
                    structured_content: json!({ "temp": 25 }),
                    ..Default::default()
                })
            }
        }

        let server = McpServer::new(Arc::new(RelayHandler), Implementation::new("s", "1"));
        let req = JsonRpcRequest::new(
            1,
            METHOD_TOOLS_CALL,
            json!({ "name": "any", "arguments": {}, "_meta": meta_value() }),
        );
        let v = serde_json::to_value(&server.handle(req).await).unwrap();
        assert_eq!(
            v["result"]["content"],
            json!([
                { "type": "text", "text": "说明" },
                { "type": "image", "data": "eHl6", "mimeType": "image/png" },
                { "type": "resource", "resource": { "uri": "file:///a.txt", "text": "内嵌" } }
            ]),
            "content 数组逐字回传"
        );
        assert_eq!(v["result"]["structuredContent"]["temp"], 25, "structuredContent 原样保留");
    }
}
