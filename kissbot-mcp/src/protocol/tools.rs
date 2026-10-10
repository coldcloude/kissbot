//! `tools/list` 与 `tools/call` 的消息类型

use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Implementation, ToolResponseMeta, with_response_meta};

use super::meta::{RequestMeta, ResponseMeta};
use super::ResultType;

/// 工具定义（名称/描述/输入 JSON Schema/工具元数据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: ArcStr,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<ArcStr>,

    #[serde(default)]
    pub description: ArcStr,

    #[serde(rename = "inputSchema")]
    pub input_schema: Arc<Value>,

    #[serde(
        rename = "outputSchema",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub output_schema: Option<Arc<Value>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Arc<Value>>,

    /// 工具元数据（规范 `Tool._meta`）；kissbot 分组等扩展随工具携带
    #[serde(
        rename = "_meta",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meta: Option<ToolResponseMeta>,
}

impl Tool {
    pub fn new(
        name: ArcStr,
        description: ArcStr,
        input_schema: Value,
    ) -> Self {
        Self {
            name,
            title: None,
            description,
            input_schema: Arc::new(input_schema),
            output_schema: None,
            annotations: None,
            meta: None,
        }
    }

    /// 该工具所属的 kissbot 分组（读 `_meta["kissbot/group"]`）
    pub fn group(&self) -> Option<&str> {
        self.meta.as_ref()?.group.as_deref()
    }

    /// 设置该工具所属的 kissbot 分组（写 `_meta["kissbot/group"]`，保留其余元数据）
    pub fn set_group(&mut self, group: ArcStr) {
        let mut meta = self.meta.take().unwrap_or_default();
        meta.group = Some(group);
        self.meta = Some(meta);
    }
}

/// `tools/list` 请求参数（params 顶层保持规范形状：cursor + _meta）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ArcStr>,
    #[serde(rename = "_meta")]
    pub meta: RequestMeta,
}

/// `tools/list` 结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListResult {
    #[serde(rename = "resultType", default)]
    pub result_type: ResultType,

    pub tools: Vec<Arc<Tool>>,

    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<ArcStr>,

    #[serde(
        rename = "_meta",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meta: Option<ResponseMeta>,
}

impl ToolsListResult {
    pub fn new(tools: Vec<Arc<Tool>>) -> Self {
        Self {
            result_type: ResultType::Complete,
            tools,
            next_cursor: None,
            meta: None,
        }
    }
}

with_response_meta!(ToolsListResult);

/// `tools/call` 请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCallParams {
    pub name: ArcStr,
    #[serde(default)]
    pub arguments: Value,
    #[serde(rename = "_meta")]
    pub meta: RequestMeta,
}

/// `tools/call` 结果
///
/// `content` 由工具（结果生产方）决定并原样回传，MCP 服务不解释、不合成、不改写它；
/// `structuredContent` 是生产方可选提供的机器可读形态。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallToolResult {
    #[serde(rename = "resultType", default)]
    pub result_type: ResultType,

    #[serde(default)]
    pub content: Vec<ContentBlock>,

    #[serde(
        rename = "structuredContent",
        default,
        skip_serializing_if = "Value::is_null"
    )]
    pub structured_content: Value,

    #[serde(rename = "isError", default)]
    pub is_error: bool,

    #[serde(
        rename = "_meta",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meta: Option<ResponseMeta>,
}

impl CallToolResult {
    /// 成功结果：单个文本块
    pub fn text(text: ArcStr) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            ..Default::default()
        }
    }
    /// 失败结果：单个文本块
    pub fn error(msg: ArcStr) -> Self {
        Self {
            content: vec![ContentBlock::text(msg)],
            is_error: true,
            ..Default::default()
        }
    }
}

with_response_meta!(CallToolResult);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 首个文本块内容（成功结果读文本用）
    fn first_text(result: &CallToolResult) -> Option<&str> {
        result.content.iter().find_map(|c| c.as_text())
    }

    #[test]
    fn tool_serializes_input_schema_camel_case() {
        let tool = Tool::new(ArcStr::from("read"), ArcStr::from("读取文件"), json!({ "type": "object" }));
        let v = serde_json::to_value(&tool).unwrap();
        assert_eq!(v["name"], "read");
        assert_eq!(v["inputSchema"]["type"], "object");
        assert!(v.get("outputSchema").is_none());
        assert!(v.get("title").is_none());
        assert!(v.get("_meta").is_none(), "无元数据时不序列化 _meta");
    }

    #[test]
    fn tool_group_travels_in_meta() {
        // group 是工具的固有属性，随工具定义的 `_meta` 携带
        let mut tool = Tool::new(ArcStr::from("read"), ArcStr::from("读取文件"), json!({ "type": "object" }));
        // 无元数据时没有分组
        assert!(tool.group().is_none());
        tool.set_group(ArcStr::from("filesystem"));
        assert_eq!(tool.group(), Some("filesystem"));
        let v = serde_json::to_value(&tool).unwrap();
        assert_eq!(v["_meta"][super::super::meta::META_GROUP.as_str()], "filesystem");
        let back: Tool = serde_json::from_value(v).unwrap();
        assert_eq!(back.group(), Some("filesystem"));

        // 重复设置覆盖为新值
        let mut plain = Tool::new(ArcStr::from("t"), ArcStr::from("d"), json!({}));
        plain.set_group(ArcStr::from("g1"));
        assert_eq!(plain.group(), Some("g1"));
        assert_eq!(
            serde_json::to_value(&plain).unwrap()["_meta"][super::super::meta::META_GROUP.as_str()],
            "g1"
        );
    }

    #[test]
    fn list_params_parse_cursor_and_ancestors() {
        // params 顶层保持规范形状（cursor + _meta）；级联祖先链在请求 `_meta`
        let params: ToolsListParams = serde_json::from_value(json!({
            "cursor": "c1",
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "kissbot/ancestors": ["a"]
            }
        }))
        .unwrap();
        assert_eq!(params.cursor.as_ref().unwrap().as_str(), "c1");
        assert_eq!(params.meta.ancestors.unwrap().as_slice(), ["a".to_string()]);

        // 不带祖先链：缺省为空（None）
        let none: ToolsListParams = serde_json::from_value(json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }))
        .unwrap();
        assert!(none.meta.ancestors.is_none());

        // 请求 `_meta` 必填：缺失即解析失败
        assert!(serde_json::from_value::<ToolsListParams>(json!({ "cursor": "c1" })).is_err());
    }

    #[test]
    fn producer_builds_content_and_structured_content_explicitly() {
        // 结构化结果由生产方显式决定 content（含兼容用的 JSON 文本块）与 structuredContent，
        // MCP 服务只原样序列化，不做任何合成
        let value = json!({ "path": "a.txt" });
        let r = CallToolResult {
            content: vec![ContentBlock::text(ArcStr::from(value.to_string()))],
            structured_content: value.clone(),
            ..Default::default()
        };
        assert!(!r.is_error);
        assert_eq!(r.structured_content["path"], "a.txt");
        assert_eq!(first_text(&r).unwrap(), r#"{"path":"a.txt"}"#);
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(back["content"][0]["text"], r#"{"path":"a.txt"}"#);
        assert_eq!(back["structuredContent"]["path"], "a.txt");
    }

    #[test]
    fn text_result_carries_no_structured_content() {
        let r = CallToolResult::text(ArcStr::from("文件内容"));
        assert_eq!(first_text(&r).unwrap(), "文件内容");
        assert!(r.structured_content.is_null());
        assert_eq!(serde_json::to_value(&r).unwrap()["content"], json!([{ "type": "text", "text": "文件内容" }]));
    }

    #[test]
    fn call_result_covers_all_content_block_types() {
        // 规范的全部 ContentBlock 变体：text / image / audio / resource_link / resource
        let content = json!([
            { "type": "text", "text": "说明" },
            { "type": "image", "data": "eHl6", "mimeType": "image/png" },
            { "type": "audio", "data": "eA==", "mimeType": "audio/wav" },
            { "type": "resource_link", "uri": "file:///a.txt", "name": "a.txt", "mimeType": "text/plain" },
            { "type": "resource", "resource": { "uri": "file:///b.txt", "mimeType": "text/plain", "text": "内嵌文本" } },
            { "type": "resource", "resource": { "uri": "file:///c.png", "mimeType": "image/png", "blob": "eHl6" } }
        ]);
        let r: CallToolResult =
            serde_json::from_value(json!({ "resultType": "complete", "content": content })).unwrap();
        assert_eq!(r.content.len(), 6);
        assert_eq!(first_text(&r), Some("说明"), "取首个文本块");
        assert_eq!(
            r.content.iter().map(|c| c.type_name()).collect::<Vec<_>>(),
            vec!["text", "image", "audio", "resource_link", "resource", "resource"]
        );
        // 各变体逐字段往返一致
        assert_eq!(serde_json::to_value(&r).unwrap()["content"], content);
    }

    #[test]
    fn content_annotations_and_meta_roundtrip() {
        let block = json!({
            "type": "image",
            "data": "eHl6",
            "mimeType": "image/png",
            "annotations": { "audience": ["user", "assistant"], "priority": 0.9 },
            "_meta": { "kissbot/note": "自定义" }
        });
        let r: CallToolResult =
            serde_json::from_value(json!({ "resultType": "complete", "content": [block] })).unwrap();
        assert_eq!(serde_json::to_value(&r).unwrap()["content"][0], block);
    }

    #[test]
    fn result_meta_omitted_when_empty() {
        // 响应 `_meta` 可选：为空时不序列化（不再输出 `"_meta": {}`）
        let call = CallToolResult::text(ArcStr::from("ok"));
        assert!(serde_json::to_value(&call).unwrap().get("_meta").is_none());
        let list = ToolsListResult::new(vec![]);
        assert!(serde_json::to_value(&list).unwrap().get("_meta").is_none());

        // 有内容时正常输出
        let with_info = CallToolResult::text(ArcStr::from("ok"))
            .with_server_info(Arc::new(Implementation::new(ArcStr::from("s"), ArcStr::from("1"))));
        assert_eq!(
            serde_json::to_value(&with_info).unwrap()["_meta"]
                ["io.modelcontextprotocol/serverInfo"]["name"],
            "s"
        );
    }

    #[test]
    fn content_text_serializes_as_text_block() {
        let r = CallToolResult::text(ArcStr::from("纯文本"));
        assert_eq!(
            serde_json::to_value(&r).unwrap()["content"],
            json!([{ "type": "text", "text": "纯文本" }])
        );
    }
}
