//! kissbot-mcp — 极简 MCP 实现（不依赖官方 SDK）
//!
//! 按 MCP 2026-07-28 规范实现无状态 JSON-RPC 2.0 通信，只保留三个方法：
//! `server/discover`、`tools/list`、`tools/call`。在协议之上增加 kissbot 扩展：
//! 工具定义用 `_meta["kissbot/group"]` 携带所属分组，级联请求用 `_meta["kissbot/ancestors"]`
//! 携带祖先链（防环），服务端在 `server/discover` 的能力里声明是否理解这些扩展键。

pub mod client;
pub mod http;
pub mod protocol;
pub mod server;

pub use client::{McpClient, McpClientError};
pub use http::{McpHttpConfig, router};
pub use protocol::*;
pub use server::{McpHandler, McpServer};
