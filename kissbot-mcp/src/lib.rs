//! kissbot-mcp — 极简 MCP 实现（不依赖官方 SDK）
//!
//! 按 MCP 2026-07-28 规范实现无状态 JSON-RPC 2.0 通信，只保留三个方法：
//! `server/discover`、`tools/list`、`tools/call`。在协议之上增加 kissbot 分组扩展：
//! 请求 `params.group`（组白名单过滤），响应 `result._meta["kissbot/group"]`（结果所属组）。

pub mod client;
pub mod http;
pub mod protocol;
pub mod server;

pub use client::{McpClient, McpClientError};
pub use http::{McpHttpConfig, router};
pub use protocol::*;
pub use server::{McpHandler, McpServer};
