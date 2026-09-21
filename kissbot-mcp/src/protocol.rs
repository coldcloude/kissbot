//! MCP 协议类型与常量（2026-07-28 规范 + kissbot 扩展）

use serde::{Deserialize, Serialize};

pub mod discover;
pub mod jsonrpc;
pub mod meta;
pub mod tools;

pub use discover::*;
pub use jsonrpc::*;
pub use meta::*;
pub use tools::*;

/// 本实现所实现的 MCP 协议版本
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// JSON-RPC 版本（消息固定携带该值）
pub const JSONRPC_VERSION: &str = "2.0";

/// `server/discover` 方法名
pub const METHOD_DISCOVER: &str = "server/discover";
/// `tools/list` 方法名
pub const METHOD_TOOLS_LIST: &str = "tools/list";
/// `tools/call` 方法名
pub const METHOD_TOOLS_CALL: &str = "tools/call";

/// 结果类型：`complete` 表示请求已完成
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ResultType {
    #[default]
    Complete,
    InputRequired,
    /// 客户端不认识的 resultType（规范要求视为非法，此处保留原文以便上层判错）
    Unknown(String),
}

impl ResultType {
    pub fn as_str(&self) -> &str {
        match self {
            ResultType::Complete => "complete",
            ResultType::InputRequired => "input_required",
            ResultType::Unknown(other) => other,
        }
    }
}

impl Serialize for ResultType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ResultType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "complete" => ResultType::Complete,
            "input_required" => ResultType::InputRequired,
            _ => ResultType::Unknown(raw),
        })
    }
}
