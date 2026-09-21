//! `server/discover`：查询服务端支持的协议版本、能力与身份

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::with_response_meta;

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

/// kissbot 扩展能力声明：服务端在 `server/discover` 中声明它理解 kissbot 的级联扩展键，
/// 客户端据此判定对端是否理解级联祖先链。分组归因（`Tool._meta["kissbot/group"]`）不需要声明。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KissbotCapability {
    /// 支持请求 `_meta["kissbot/ancestors"]` 级联祖先链与自环检测
    #[serde(default)]
    pub ancestors: bool,
}

impl KissbotCapability {
    /// 支持 kissbot 的级联扩展
    pub fn full() -> Self {
        Self { ancestors: true }
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

    #[serde(
        rename = "_meta",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meta: Option<ResponseMeta>,
}

impl DiscoverResult {
    /// 以给定服务端身份构造发现结果：本实现默认声明 tools 与 kissbot 级联扩展能力。
    /// 读取方向仍兼容不带 kissbot 能力段的第三方服务端（`capabilities.kissbot` 为 `Option`）
    pub fn new(server_info: Implementation, instructions: Option<String>) -> Self {
        Self {
            result_type: ResultType::Complete,
            supported_versions: vec![super::PROTOCOL_VERSION.to_string()],
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability { list_changed: false }),
                kissbot: Some(KissbotCapability::full()),
            },
            instructions: instructions.map(Arc::new),
            meta: Some(ResponseMeta {
                server_info: Some(server_info),
            }),
        }
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

with_response_meta!(DiscoverResult);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kissbot_capability_roundtrip() {
        // 本实现构造的发现结果默认声明 kissbot 级联扩展
        let result = DiscoverResult::new(Implementation::new("s", "1"), None);
        let v = serde_json::to_value(&result).unwrap();
        assert_eq!(v["capabilities"]["kissbot"]["ancestors"], true);
        assert!(v["capabilities"]["kissbot"].get("group").is_none(), "分组归因不需要能力声明");
        assert_eq!(v["capabilities"]["tools"]["listChanged"], false);
        assert!(result.supports_ancestors() && result.supports_kissbot());
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
