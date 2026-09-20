//! `server/discover`：查询服务端支持的协议版本、能力与身份

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::meta::{Implementation, RequestMeta, ResponseMeta};
use super::ResultType;

/// `server/discover` 请求参数（除标准 `_meta` 外无其他字段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverParams {
    #[serde(rename = "_meta")]
    pub meta: RequestMeta,
}

/// 工具能力声明
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsCapability {
    /// 工具列表变化时是否发送通知（本实现不发送）
    #[serde(rename = "listChanged", default)]
    pub list_changed: bool,
}

/// kissbot 扩展能力声明：服务端在 `server/discover` 中声明它理解 kissbot 扩展键，
/// 客户端据此决定是否使用分组过滤与级联祖先链。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KissbotCapability {
    /// 支持 `tools/list` 的 `group` 过滤与结果 `kissbot/group` 回带
    #[serde(default)]
    pub group: bool,
    /// 支持请求 `_meta["kissbot/ancestors"]` 级联祖先链与自环检测
    #[serde(default)]
    pub ancestors: bool,
}

impl KissbotCapability {
    /// 完整支持 kissbot 的分组与级联扩展
    pub fn full() -> Self {
        Self { group: true, ancestors: true }
    }
}

/// 服务端能力集合：声明 tools 与 kissbot 扩展（不保留未知能力项）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kissbot: Option<KissbotCapability>,
}

/// 发现结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverResult {
    #[serde(rename = "resultType", default)]
    pub result_type: ResultType,
    #[serde(rename = "supportedVersions")]
    pub supported_versions: Vec<String>,
    pub capabilities: ServerCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Arc<String>>,
    #[serde(rename = "_meta", default)]
    pub meta: ResponseMeta,
}

impl DiscoverResult {
    /// 以给定服务端身份构造仅含 tools 能力的发现结果
    pub fn tools_only(server_info: Implementation, instructions: Option<String>) -> Self {
        Self {
            result_type: ResultType::Complete,
            supported_versions: vec![super::PROTOCOL_VERSION.to_string()],
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability { list_changed: false }),
                kissbot: None,
            },
            instructions: instructions.map(Arc::new),
            meta: ResponseMeta {
                server_info: Some(server_info),
                group: None,
            },
        }
    }

    /// 声明完整支持 kissbot 的分组与级联扩展
    pub fn with_kissbot_extensions(mut self) -> Self {
        self.capabilities.kissbot = Some(KissbotCapability::full());
        self
    }

    /// 对端是否声明支持 `tools/list` 分组过滤
    pub fn supports_group(&self) -> bool {
        self.capabilities.kissbot.as_ref().is_some_and(|k| k.group)
    }

    /// 对端是否声明支持级联祖先链
    pub fn supports_ancestors(&self) -> bool {
        self.capabilities.kissbot.as_ref().is_some_and(|k| k.ancestors)
    }

    /// 对端是否声明支持任一项 kissbot 扩展
    pub fn supports_kissbot(&self) -> bool {
        self.capabilities.kissbot.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kissbot_capability_roundtrip() {
        let result = DiscoverResult::tools_only(Implementation::new("s", "1"), None)
            .with_kissbot_extensions();
        let v = serde_json::to_value(&result).unwrap();
        assert_eq!(v["capabilities"]["kissbot"]["group"], true);
        assert_eq!(v["capabilities"]["kissbot"]["ancestors"], true);
        assert_eq!(v["capabilities"]["tools"]["listChanged"], false);
        assert!(result.supports_group() && result.supports_ancestors() && result.supports_kissbot());
    }

    #[test]
    fn plain_server_reports_no_kissbot_support() {
        // 通用 MCP 服务端：能力里没有 kissbot 段
        let raw = json!({
            "resultType": "complete",
            "supportedVersions": ["2026-07-28"],
            "capabilities": { "tools": { "listChanged": false } },
            "_meta": {}
        });
        let result: DiscoverResult = serde_json::from_value(raw).unwrap();
        assert!(!result.supports_group());
        assert!(!result.supports_ancestors());
        assert!(!result.supports_kissbot());
    }

    #[test]
    fn capabilities_with_other_keys_still_parse_and_are_dropped() {
        // 第三方服务端可能声明 resources / prompts / logging 等本实现不建模的能力：
        // 解析不报错，这些键只是被忽略（不保留、不透传）
        let raw = json!({
            "resultType": "complete",
            "supportedVersions": ["2026-07-28"],
            "capabilities": {
                "tools": { "listChanged": true },
                "resources": {},
                "prompts": {},
                "logging": {}
            },
            "_meta": {}
        });
        let result: DiscoverResult = serde_json::from_value(raw).unwrap();
        assert!(result.capabilities.tools.is_some());
        assert!(!result.supports_kissbot());
        let back = serde_json::to_value(&result).unwrap();
        assert!(back["capabilities"].get("resources").is_none(), "未建模能力不序列化");
        assert!(back["capabilities"].get("prompts").is_none());
    }
}
