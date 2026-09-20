//! `tools/list` 与 `tools/call` 的消息类型

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::meta::{RequestMeta, ResponseMeta};
use super::ResultType;

/// 工具定义（名称/描述/输入 JSON Schema/工具元数据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: Arc<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Arc<String>>,
    #[serde(default)]
    pub description: Arc<String>,
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
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Arc<Value>>,
}

impl Tool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: Arc::new(name.into()),
            title: None,
            description: Arc::new(description.into()),
            input_schema: Arc::new(input_schema),
            output_schema: None,
            annotations: None,
            meta: None,
        }
    }

    /// 该工具所属的 kissbot 分组（读 `_meta["kissbot/group"]`）
    pub fn group(&self) -> Option<&str> {
        self.meta.as_ref()?.get(super::meta::META_GROUP)?.as_str()
    }

    /// 设置该工具所属的 kissbot 分组（写 `_meta["kissbot/group"]`，保留其余元数据）
    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        let mut object = self
            .meta
            .as_ref()
            .and_then(|m| m.as_object())
            .cloned()
            .unwrap_or_default();
        object.insert(
            super::meta::META_GROUP.to_string(),
            Value::String(group.into()),
        );
        self.meta = Some(Arc::new(Value::Object(object)));
        self
    }
}

/// `tools/list` 请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Arc<String>>,
    /// kissbot 扩展：组白名单（缺省 = 全部，空数组 = 空，非空 = 命中白名单）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Vec<String>>,
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
    pub next_cursor: Option<Arc<String>>,
    #[serde(rename = "_meta", default)]
    pub meta: ResponseMeta,
}

impl ToolsListResult {
    pub fn new(tools: Vec<Arc<Tool>>) -> Self {
        Self {
            result_type: ResultType::Complete,
            tools,
            next_cursor: None,
            meta: ResponseMeta::default(),
        }
    }
}

/// `tools/call` 请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCallParams {
    pub name: Arc<String>,
    #[serde(default)]
    pub arguments: Value,
    #[serde(rename = "_meta")]
    pub meta: RequestMeta,
}

/// 内容块注解
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Annotations {
    /// 目标受众（user / assistant，可多个）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<Role>>,
    /// 重要度（0~1）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(rename = "lastModified", default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<Arc<String>>,
}

/// 注解的受众角色
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// 图标（ResourceLink 可选携带）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Icon {
    pub src: Arc<String>,
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<IconTheme>,
}

/// 图标主题
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconTheme {
    Light,
    Dark,
}

/// 嵌入资源的内容（按有无 `text` / `blob` 区分两种形状，无 type 标签）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceContents {
    Text {
        uri: Arc<String>,
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<Arc<String>>,
        text: Arc<String>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Arc<Value>>,
    },
    Blob {
        uri: Arc<String>,
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<Arc<String>>,
        blob: Arc<String>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Arc<Value>>,
    },
}

/// 工具结果内容块：`type` 即枚举变体（内部标签序列化，`type` 与其他字段平级）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    /// 文本块
    Text {
        text: Arc<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Arc<Value>>,
    },
    /// 图片块（`data` 为 base64）
    Image {
        data: Arc<String>,
        #[serde(rename = "mimeType")]
        mime_type: Arc<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Arc<Value>>,
    },
    /// 音频块（`data` 为 base64）
    Audio {
        data: Arc<String>,
        #[serde(rename = "mimeType")]
        mime_type: Arc<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Arc<Value>>,
    },
    /// 资源链接
    ResourceLink {
        uri: Arc<String>,
        name: Arc<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<Arc<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<Arc<String>>,
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<Arc<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icons: Option<Vec<Arc<Icon>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Arc<Value>>,
    },
    /// 嵌入资源
    Resource {
        resource: Arc<ResourceContents>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Arc<Value>>,
    },
}

impl Content {
    /// 文本块快捷构造
    pub fn text(text: impl Into<String>) -> Self {
        Content::Text {
            text: Arc::new(text.into()),
            annotations: None,
            meta: None,
        }
    }

    /// 文本块返回其文本，非文本块返回 None
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    /// 内容块类型标签（与 wire 上的 `type` 一致）
    pub fn type_name(&self) -> &'static str {
        match self {
            Content::Text { .. } => "text",
            Content::Image { .. } => "image",
            Content::Audio { .. } => "audio",
            Content::ResourceLink { .. } => "resource_link",
            Content::Resource { .. } => "resource",
        }
    }
}

/// kissbot 错误来源标识（`ErrorDetail.source`），kissbot 实现固定填该值
pub const ERROR_SOURCE_KISSBOT: &str = "kissbot";

/// `isError=true` 时 `structuredContent` 的固定错误结构
///
/// `source` 标识错误来源。来源非 kissbot、结构不符或无法解析时，调用方应把结果原样透传，
/// 不做解释；来源为 kissbot 时可按 `code` 处理（见 `CallToolResult::kissbot_error()`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: u32,
    pub message: Arc<String>,
    pub source: Arc<String>,
}

impl ErrorDetail {
    /// kissbot 来源的错误
    pub fn kissbot(code: u32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: Arc::new(message.into()),
            source: Arc::new(ERROR_SOURCE_KISSBOT.to_string()),
        }
    }

    /// 错误是否来自 kissbot
    pub fn is_kissbot(&self) -> bool {
        self.source.as_str() == ERROR_SOURCE_KISSBOT
    }
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
    pub content: Vec<Content>,
    #[serde(
        rename = "structuredContent",
        default,
        skip_serializing_if = "Value::is_null"
    )]
    pub structured_content: Value,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
    #[serde(rename = "_meta", default)]
    pub meta: ResponseMeta,
}

impl CallToolResult {
    /// 成功结果：单个文本块（文本结果的便捷构造）
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::text(text)],
            ..Default::default()
        }
    }

    /// 工具执行失败结果：`isError=true`，固定错误结构落在 `structuredContent`
    pub fn error(code: u32, message: impl Into<String>) -> Self {
        let detail = ErrorDetail::kissbot(code, message);
        Self {
            content: vec![Content::text(detail.message.to_string())],
            structured_content: serde_json::to_value(&detail).unwrap_or(Value::Null),
            is_error: true,
            ..Default::default()
        }
    }

    /// 首个文本块内容
    pub fn text_content(&self) -> Option<String> {
        self.content.iter().find_map(|c| c.as_text().map(|t| t.to_string()))
    }

    /// 解析 `isError` 结果的固定错误结构（不校验来源；结构不符返回 None）
    pub fn error_detail(&self) -> Option<ErrorDetail> {
        if !self.is_error {
            return None;
        }
        serde_json::from_value(self.structured_content.clone()).ok()
    }

    /// 仅当错误来源为 kissbot 时给出固定错误结构；解析失败、结构不符或来源非 kissbot 均为 None，
    /// 此时调用方应把结果原样透传，不按错误码解释。
    pub fn kissbot_error(&self) -> Option<ErrorDetail> {
        self.error_detail().filter(ErrorDetail::is_kissbot)
    }

    /// 失败结果中携带的错误文案（kissbot 错误结构优先，其次回退首个文本块）
    pub fn error_message(&self) -> Option<String> {
        if !self.is_error {
            return None;
        }
        self.error_detail()
            .map(|d| d.message.to_string())
            .or_else(|| self.text_content())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_serializes_input_schema_camel_case() {
        let tool = Tool::new("read", "读取文件", json!({ "type": "object" }));
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
        let tool = Tool::new("read", "读取文件", json!({ "type": "object" })).with_group("filesystem");
        assert_eq!(tool.group(), Some("filesystem"));
        let v = serde_json::to_value(&tool).unwrap();
        assert_eq!(v["_meta"][super::super::meta::META_GROUP], "filesystem");
        let back: Tool = serde_json::from_value(v).unwrap();
        assert_eq!(back.group(), Some("filesystem"));

        // 设置 group 保留其余元数据
        let with_other = Tool {
            meta: Some(Arc::new(json!({ "other/key": 1 }))),
            ..Tool::new("t", "d", json!({}))
        }
        .with_group("g1");
        assert_eq!(with_other.meta.as_ref().unwrap()["other/key"], 1);
        assert_eq!(with_other.group(), Some("g1"));

        // 无元数据时没有分组
        assert!(Tool::new("t", "d", json!({})).group().is_none());
    }

    #[test]
    fn list_params_parse_group_and_meta() {
        let params: ToolsListParams = serde_json::from_value(json!({
            "group": ["filesystem"],
            "cursor": "c1",
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "kissbot/ancestors": ["a"]
            }
        }))
        .unwrap();
        assert_eq!(params.group.as_ref().unwrap(), &vec!["filesystem".to_string()]);
        assert_eq!(params.cursor.as_ref().unwrap().as_str(), "c1");
        assert_eq!(params.meta.ancestors, vec!["a".to_string()]);
    }

    #[test]
    fn producer_builds_content_and_structured_content_explicitly() {
        // 结构化结果由生产方显式决定 content（含兼容用的 JSON 文本块）与 structuredContent，
        // MCP 服务只原样序列化，不做任何合成
        let value = json!({ "path": "a.txt" });
        let r = CallToolResult {
            content: vec![Content::text(value.to_string())],
            structured_content: value.clone(),
            ..Default::default()
        };
        assert!(!r.is_error);
        assert_eq!(r.structured_content["path"], "a.txt");
        assert_eq!(r.text_content().unwrap(), r#"{"path":"a.txt"}"#);
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(back["content"][0]["text"], r#"{"path":"a.txt"}"#);
        assert_eq!(back["structuredContent"]["path"], "a.txt");
    }

    #[test]
    fn text_result_carries_no_structured_content() {
        let r = CallToolResult::text("文件内容");
        assert_eq!(r.text_content().unwrap(), "文件内容");
        assert!(r.structured_content.is_null());
        assert_eq!(serde_json::to_value(&r).unwrap()["content"], json!([{ "type": "text", "text": "文件内容" }]));
    }

    #[test]
    fn call_result_error_uses_fixed_error_struct_with_kissbot_source() {
        let r = CallToolResult::error(1002, "工具不存在: nope");
        assert!(r.is_error);
        // 固定错误结构：code / message / source
        assert_eq!(r.structured_content["code"], 1002);
        assert_eq!(r.structured_content["message"], "工具不存在: nope");
        assert_eq!(r.structured_content["source"], ERROR_SOURCE_KISSBOT);
        assert_eq!(r.error_message().unwrap(), "工具不存在: nope");
        let detail = r.kissbot_error().expect("kissbot 来源应能解析");
        assert_eq!(detail.code, 1002);
        assert!(detail.is_kissbot());
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["resultType"], "complete");
        assert_eq!(v["isError"], true);
    }

    #[test]
    fn non_kissbot_or_malformed_error_is_not_interpreted() {
        // 结构不符 / 来源非 kissbot / 解析失败：kissbot_error() 一律返回 None（调用方透传）
        let cases = [
            // 结构不符：无 source
            json!({ "code": 7, "message": "boom" }),
            // 结构不符：code 类型不对
            json!({ "code": "7", "message": "boom", "source": ERROR_SOURCE_KISSBOT }),
            // 来源非 kissbot
            json!({ "code": 7, "message": "boom", "source": "other-mcp" }),
            // 完全不是错误结构
            json!("plain text error"),
            json!({ "error": "boom" }),
        ];
        for structured in cases {
            let r = CallToolResult {
                content: vec![Content::text("boom")],
                structured_content: structured.clone(),
                is_error: true,
                ..Default::default()
            };
            assert!(
                r.kissbot_error().is_none(),
                "非 kissbot 结构不应被解释: {}",
                structured
            );
            // 文案仍可取（回退文本块），但错误体本身原样保留
            assert_eq!(r.error_message().as_deref(), Some("boom"));
            assert_eq!(serde_json::to_value(&r).unwrap()["structuredContent"], structured);
        }
    }

    #[test]
    fn error_detail_ignores_non_error_results() {
        let ok = CallToolResult::text("正常结果");
        assert!(ok.error_detail().is_none());
        assert!(ok.kissbot_error().is_none());
        assert!(ok.error_message().is_none());
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
        assert_eq!(r.text_content().as_deref(), Some("说明"), "取首个文本块");
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
    fn content_text_serializes_as_text_block() {
        let r = CallToolResult::text("纯文本");
        assert_eq!(
            serde_json::to_value(&r).unwrap()["content"],
            json!([{ "type": "text", "text": "纯文本" }])
        );
    }
}
