use async_trait::async_trait;
use kissbot_mcp::CallToolResult;
use serde_json::Value;

use crate::{
    pipeline::AgentToolCaller,
    station::{Station, TERR_TOOL_INTERNAL_ERROR, tool_error},
    types::{ToolCall, ToolData},
};

/// Nexus 侧工具调用器：把 ToolCall 适配到 Station 的 MCP 工具接口，并把结果还原为 ToolData。
/// ToolCall/ToolData 只属于 Nexus 交互层，Station 内部不使用它们。
pub struct StationToolCaller;

/// MCP 工具结果 → Nexus 侧 ToolData
///
/// 只承载 MCP `content` 字段转成的 `Value`（内容块数组，完整保留 text / image / audio /
/// resource_link / resource 各类型）：成功进 `result`、失败进 `error`。
/// `structuredContent` 不进入 Nexus，也就不回传 LLM。
fn call_result_to_tool_data(arguments: Value, result: CallToolResult) -> ToolData {
    let content = serde_json::to_value(&result.content).unwrap_or(Value::Null);
    if result.is_error {
        return ToolData { arguments, result: Value::Null, error: content };
    }
    ToolData { arguments, result: content, error: Value::Null }
}

#[async_trait]
impl AgentToolCaller for StationToolCaller {
    async fn call_tool(&self, mut tool_call: ToolCall) -> ToolCall {
        let arguments = tool_call.data.arguments.take();
        let called = Station::get()
            .call_tool(tool_call.name.as_str(), arguments.clone(), &[])
            .await;
        tool_call.data = match called {
            Ok(result) => call_result_to_tool_data(arguments, result),
            Err(e) => ToolData {
                arguments,
                result: Value::Null,
                error: tool_error(TERR_TOOL_INTERNAL_ERROR, e.message),
            },
        };
        tool_call
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::TERR_TOOL_NOT_FOUND;
    use serde_json::json;

    #[test]
    fn success_result_uses_mcp_content_as_value() {
        // 文本结果：文本块原样承载
        let data = call_result_to_tool_data(
            json!({ "path": "a.txt" }),
            CallToolResult::text("内容"),
        );
        assert_eq!(data.result, json!([{ "type": "text", "text": "内容" }]));
        assert!(data.error.is_null());
        assert_eq!(data.arguments["path"], "a.txt", "原始参数保留");

        // 结构化结果：文本块承载 JSON 序列化，同时结构保留在 structuredContent
        let structured = call_result_to_tool_data(
            Value::Null,
            CallToolResult {
                content: vec![kissbot_mcp::Content::text("{\"temp\":25}".to_string())],
                structured_content: json!({ "temp": 25 }),
                ..Default::default()
            },
        );
        assert_eq!(
            structured.result,
            json!([{ "type": "text", "text": "{\"temp\":25}" }])
        );
    }

    #[test]
    fn kissbot_error_uses_content_as_error_and_drops_structured_content() {
        let data = call_result_to_tool_data(
            Value::Null,
            CallToolResult::error(TERR_TOOL_NOT_FOUND, "工具不存在: x"),
        );
        assert!(data.result.is_null(), "失败时 result 留空");
        // error 取 content（文本块），不回传 structuredContent
        assert_eq!(data.error, json!([{ "type": "text", "text": "工具不存在: x" }]));
    }

    #[test]
    fn foreign_error_uses_content_and_drops_structured_content() {
        let result = CallToolResult {
            content: vec![kissbot_mcp::Content::text("boom")],
            structured_content: json!({ "error": "boom", "detail": 42 }),
            is_error: true,
            ..Default::default()
        };
        let data = call_result_to_tool_data(Value::Null, result);
        assert!(data.result.is_null());
        assert_eq!(data.error, json!([{ "type": "text", "text": "boom" }]));
        assert!(!data.error.to_string().contains("detail"), "structuredContent 不回传");
    }

    #[test]
    fn error_with_content_blocks_keeps_all_blocks() {
        let result = CallToolResult {
            content: vec![
                kissbot_mcp::Content::text("失败说明"),
                kissbot_mcp::Content::Image {
                    data: std::sync::Arc::new("eHl6".to_string()),
                    mime_type: std::sync::Arc::new("image/png".to_string()),
                    annotations: None,
                    meta: None,
                },
            ],
            structured_content: json!({ "code": 7, "source": "kissbot" }),
            is_error: true,
            ..Default::default()
        };
        let data = call_result_to_tool_data(Value::Null, result);
        assert_eq!(data.error.as_array().unwrap().len(), 2, "失败结果的 content 块原样带出");
        assert_eq!(data.error[1]["type"], "image");
    }

    #[test]
    fn text_only_success_becomes_content_array() {
        let data = call_result_to_tool_data(Value::Null, CallToolResult::text("纯文本"));
        assert_eq!(
            data.result,
            json!([{ "type": "text", "text": "纯文本" }])
        );
    }

    #[test]
    fn all_content_block_types_are_preserved_in_result() {
        let result: CallToolResult = serde_json::from_value(json!({
            "resultType": "complete",
            "content": [
                { "type": "text", "text": "说明" },
                { "type": "image", "data": "eHl6", "mimeType": "image/png" },
                { "type": "audio", "data": "eA==", "mimeType": "audio/wav" },
                { "type": "resource_link", "uri": "file:///a.txt", "name": "a.txt" },
                { "type": "resource", "resource": { "uri": "file:///b.txt", "text": "内嵌" } }
            ]
        }))
        .unwrap();
        let data = call_result_to_tool_data(Value::Null, result);
        assert_eq!(data.result.as_array().unwrap().len(), 5);
        assert_eq!(data.result[1]["type"], "image");
        assert_eq!(data.result[1]["data"], "eHl6");
        assert_eq!(data.result[4]["resource"]["text"], "内嵌");
    }
}
