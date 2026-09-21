//! JSON-RPC 2.0 消息类型与错误码
//!
//! 与基础 JSON-RPC 的差异（MCP 规范要求）：请求 id 不得为 null；结果响应必须携带
//! `resultType` 字段。响应构造由 `JsonRpcResponse` 承担。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::JSONRPC_VERSION;

// ========== JSON-RPC 标准错误码 ==========

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

// ========== MCP 规范错误码（规范保留区间 -32020..-32099） ==========

/// HTTP 请求头与请求体不一致
pub const HEADER_MISMATCH: i64 = -32020;
/// 缺少服务端要求的客户端能力
pub const MISSING_REQUIRED_CLIENT_CAPABILITY: i64 = -32021;
/// 服务端不支持请求的协议版本
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

// ========== kissbot 应用自定义错误码（JSON-RPC 保留区间之外） ==========

/// station 级联自环（自身 station_id 出现在祖先链中）
pub const STATION_CYCLE: i64 = 1001;

/// JSON-RPC 请求 ID（字符串或整数；协议禁止 null）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::Number(n) => write!(f, "{}", n),
            RequestId::String(s) => write!(f, "{}", s),
        }
    }
}

impl From<i64> for RequestId {
    fn from(value: i64) -> Self {
        RequestId::Number(value)
    }
}

impl From<String> for RequestId {
    fn from(value: String) -> Self {
        RequestId::String(value)
    }
}

impl From<&str> for RequestId {
    fn from(value: &str) -> Self {
        RequestId::String(value.to_string())
    }
}

/// JSON-RPC 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: Arc<String>,
    pub id: RequestId,
    pub method: Arc<String>,
    #[serde(default)]
    pub params: Value,
}

impl JsonRpcRequest {
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: Arc::new(JSONRPC_VERSION.to_string()),
            id: id.into(),
            method: Arc::new(method.into()),
            params,
        }
    }

    /// 校验 JSON-RPC 版本（`JSONRPC_VERSION`）
    pub fn validate(&self) -> Result<(), McpError> {
        if self.jsonrpc.as_str() != JSONRPC_VERSION {
            return Err(McpError::invalid_request(format!(
                "不支持的 jsonrpc 版本: {}",
                self.jsonrpc
            )));
        }
        Ok(())
    }
}

/// JSON-RPC 错误对象
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: Arc<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl JsonRpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: Arc::new(message.into()),
            data: Value::Null,
        }
    }

    pub fn with_data(code: i64, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: Arc::new(message.into()),
            data,
        }
    }
}

/// JSON-RPC 响应（result / error 二者之一）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: Arc<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// 成功响应：result 必须携带 resultType（由上层结果类型保证）
    pub fn success(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: Arc::new(JSONRPC_VERSION.to_string()),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    /// 错误响应；id 未知（如解析失败）时为 None
    pub fn error(id: Option<RequestId>, error: McpError) -> Self {
        Self {
            jsonrpc: Arc::new(JSONRPC_VERSION.to_string()),
            id,
            result: None,
            error: Some(JsonRpcError {
                code: error.code,
                message: Arc::new(error.message),
                data: error.data,
            }),
        }
    }

    /// 取出 result，error 时转为 `McpError`
    pub fn into_result(self) -> Result<Value, McpError> {
        match (self.result, self.error) {
            (Some(result), _) => Ok(result),
            (None, Some(error)) => Err(McpError {
                code: error.code,
                message: error.message.to_string(),
                data: error.data,
            }),
            (None, None) => Err(McpError::internal("响应既无 result 也无 error")),
        }
    }
}

/// MCP 错误：协议级失败，映射为 JSON-RPC 错误响应
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct McpError {
    pub code: i64,
    pub message: String,
    pub data: Value,
}

impl McpError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: Value::Null,
        }
    }

    pub fn with_data(code: i64, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }

    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(PARSE_ERROR, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(INVALID_REQUEST, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("方法不存在: {}", method))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, message)
    }

    pub fn header_mismatch(message: impl Into<String>) -> Self {
        Self::new(HEADER_MISMATCH, message)
    }

    pub fn unsupported_protocol_version(requested: &str, supported: &[String]) -> Self {
        Self::with_data(
            UNSUPPORTED_PROTOCOL_VERSION,
            format!("不支持的协议版本: {}", requested),
            serde_json::json!({ "supportedVersions": supported }),
        )
    }

    /// station 级联自环
    pub fn station_cycle(station_id: &str) -> Self {
        Self::new(STATION_CYCLE, format!("station cycle detected: {}", station_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_roundtrip_keeps_numeric_id() {
        let raw = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/list",
            "params": { "group": ["fs"] }
        });
        let req: JsonRpcRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.id, RequestId::Number(7));
        assert_eq!(req.method.as_str(), "tools/list");
        assert_eq!(req.params["group"][0], "fs");
        req.validate().unwrap();
    }

    #[test]
    fn request_rejects_null_id() {
        // 协议禁止 null id：反序列化应失败
        let raw = json!({ "jsonrpc": "2.0", "id": null, "method": "tools/list" });
        assert!(serde_json::from_value::<JsonRpcRequest>(raw).is_err());
    }

    #[test]
    fn request_validate_rejects_wrong_jsonrpc() {
        let req = JsonRpcRequest {
            jsonrpc: Arc::new("1.0".to_string()),
            id: RequestId::Number(1),
            method: Arc::new("tools/list".to_string()),
            params: Value::Null,
        };
        assert_eq!(req.validate().unwrap_err().code, INVALID_REQUEST);
    }

    #[test]
    fn response_success_omits_error() {
        let resp = JsonRpcResponse::success(RequestId::Number(1), json!({"ok": true}));
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert!(v.get("error").is_none());
        assert_eq!(resp.into_result().unwrap()["ok"], true);
    }

    #[test]
    fn response_error_maps_to_mcp_error() {
        let resp = JsonRpcResponse::error(
            Some(RequestId::Number(2)),
            McpError::station_cycle("station-a"),
        );
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["error"]["code"], STATION_CYCLE);
        assert!(v.get("result").is_none());
        let err = resp.into_result().unwrap_err();
        assert_eq!(err.code, STATION_CYCLE);
        assert!(err.message.contains("station-a"));
    }

    #[test]
    fn response_without_id_serializes_without_id() {
        let resp = JsonRpcResponse::error(None, McpError::parse_error("bad json"));
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.get("id").is_none(), "无法读取 id 时省略 id 字段");
    }
}
