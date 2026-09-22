//! kissbot-llm — LLM 基础组件。
//!
//! 把「与模型服务商通信」和「会话上下文管理」从业务组件中抽出，供 agent 之外的模块
//! （摘要系统、笔记工具等）复用。组件只做两件事：
//!
//! - **provider**：按 provider 配置构造 OpenAI 兼容 / Anthropic 实现并完成一次调用（含重试），
//!   以及拉取模型列表；不持有配置、不做 provider 分派（由调用方按 `provider_type` 选择 `create_*`）。
//! - **session**：按会话标识管理 LLM 上下文（内存消息 + 本地缓存 + 历史归档）。
//!
//! 配置类型（provider/model 参数、LLM 请求参数）与消息/响应类型也在本组件内定义；
//! 配置的存储、落盘与继承链（agent → role → session）留给调用方（kissbot-agent 的 ConfigManager）。

pub mod config;
pub mod error;
pub mod provider;
pub mod session;
pub mod types;

pub use config::*;
pub use error::*;
pub use provider::*;
pub use session::*;
pub use types::*;
