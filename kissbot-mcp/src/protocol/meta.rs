//! MCP `_meta` 协议字段与 kissbot 扩展键
//!
//! 规范保留键（`io.modelcontextprotocol/*`）承载协议版本、客户端/服务端身份与能力；
//! kissbot 自身扩展键统一使用 `kissbot/` 前缀（非保留前缀，符合规范命名规则）。

use std::sync::Arc;

use arcstr::ArcStr;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 协议版本（每请求必填）
pub const META_PROTOCOL_VERSION: ArcStr = arcstr::literal!("io.modelcontextprotocol/protocolVersion");
/// 客户端身份
pub const META_CLIENT_INFO: ArcStr = arcstr::literal!("io.modelcontextprotocol/clientInfo");
/// 客户端能力（每请求必填）
pub const META_CLIENT_CAPABILITIES: ArcStr = arcstr::literal!("io.modelcontextprotocol/clientCapabilities");
/// 服务端身份
pub const META_SERVER_INFO: ArcStr = arcstr::literal!("io.modelcontextprotocol/serverInfo");
/// kissbot 扩展：工具自身所属的组名（`Tool._meta`）
pub const META_GROUP: ArcStr = arcstr::literal!("kissbot/group");
/// kissbot 扩展：station 级联祖先链（根 → 当前父节点）
pub const META_ANCESTORS: ArcStr = arcstr::literal!("kissbot/ancestors");

/// 实现身份（客户端/服务端自报，协议不校验）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    pub name: ArcStr,
    #[serde(default)]
    pub version: ArcStr,
}

impl Implementation {
    pub fn new(name: ArcStr, version: ArcStr) -> Self {
        Self {
            name,
            version,
        }
    }
}

/// 请求 `_meta`：协议版本与客户端能力必填，其余可选
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMeta {

    #[serde(rename = "io.modelcontextprotocol/protocolVersion")]
    pub protocol_version: ArcStr,

    #[serde(
        rename = "io.modelcontextprotocol/clientInfo",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub client_info: Option<Implementation>,

    #[serde(rename = "io.modelcontextprotocol/clientCapabilities")]
    pub client_capabilities: Value,

    /// kissbot 扩展：MCP 级联
    #[serde(
        rename = "kissbot/ancestors",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ancestors: Option<Arc<Vec<ArcStr>>>,
}

impl RequestMeta {
    /// 构造客户端请求 `_meta`（协议版本固定为本实现版本，能力为空对象）
    pub fn new(client_info: Implementation) -> Self {
        Self {
            protocol_version: super::PROTOCOL_VERSION.clone(),
            client_info: Some(client_info),
            client_capabilities: serde_json::json!({}),
            ancestors: None,
        }
    }

    /// 设置级联祖先链
    pub fn with_ancestors(mut self, ancestors: Arc<Vec<ArcStr>>) -> Self {
        self.ancestors = Some(ancestors);
        self
    }
}

/// 响应 `_meta`：服务端身份（分组扩展随工具定义携带，见 `Tool._meta`）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponseMeta {
    #[serde(
        rename = "io.modelcontextprotocol/serverInfo",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub server_info: Option<Implementation>,
}

#[macro_export]
macro_rules! with_response_meta {
    ($ty:ty) => {
        impl $ty {
            /// 添加meta
            pub fn with_server_info(mut self, server_info: Implementation) -> Self {
                let mut meta = self.meta.take().unwrap_or_default();
                meta.server_info = Some(server_info);
                self.meta = Some(meta);
                self
            }
        }
    };
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolResponseMeta {
    #[serde(
        rename = "kissbot/group",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub group: Option<ArcStr>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_meta_requires_protocol_version_and_capabilities() {
        let ok: RequestMeta = serde_json::from_value(json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "kissbot/ancestors": ["root"]
        }))
        .unwrap();
        assert_eq!(ok.protocol_version.as_str(), "2026-07-28");
        assert_eq!(ok.ancestors.unwrap().as_slice(), ["root".to_string()]);

        // 缺 protocolVersion：解析失败（上层映射为 -32602）
        assert!(
            serde_json::from_value::<RequestMeta>(json!({
                "io.modelcontextprotocol/clientCapabilities": {}
            }))
            .is_err()
        );
        // 缺 clientCapabilities：解析失败
        assert!(
            serde_json::from_value::<RequestMeta>(json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28"
            }))
            .is_err()
        );
    }

    #[test]
    fn request_meta_omits_empty_ancestors() {
        let meta = RequestMeta::new(Implementation::new(ArcStr::from("c"), ArcStr::from("1")));
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v.get(META_ANCESTORS.as_str()).is_none(), "空祖先链不序列化");
        assert_eq!(v[META_PROTOCOL_VERSION.as_str()], "2026-07-28");
    }

    #[test]
    fn response_meta_serializes_only_server_info() {
        // 结果侧 `_meta` 以 Option 表达可选：默认（无身份）序列化为 {}
        let empty = ResponseMeta::default();
        assert_eq!(serde_json::to_value(&empty).unwrap(), json!({}));
        let filled = ResponseMeta {
            server_info: Some(Implementation::new(ArcStr::from("s"), ArcStr::from("1"))),
        };
        assert_eq!(serde_json::to_value(&filled).unwrap()[META_SERVER_INFO.as_str()]["name"], "s");
    }

    #[test]
    fn response_meta_carries_server_info_only() {
        // 分组随工具定义携带（Tool._meta），不再放在响应 meta 上
        let meta = ResponseMeta {
            server_info: Some(Implementation::new(ArcStr::from("kissbot-station"), ArcStr::from("0.1.0"))),
        };
        let v = serde_json::to_value(&meta).unwrap();
        assert_eq!(v[META_SERVER_INFO.as_str()]["name"], "kissbot-station");
        assert!(v.get(META_GROUP.as_str()).is_none(), "响应 meta 不带 group");
    }
}
