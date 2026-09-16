use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ========== 工具调用相关 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolData {
    pub arguments: Value,
    pub result: Value,
    pub error: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Arc<String>,
    pub name: Arc<String>,
    pub data: ToolData,
}

// ========== 模型相关 ==========

#[derive(Debug)]
pub struct ModelResponse {
    /// 返回内容
    pub raw_content: Value,
    pub content: Option<Arc<String>>,
    pub thinking: Option<Arc<String>>, // <think> 标签解析（去标签）
    /// 思考内容：API 字段（DeepSeek reasoning_content / anthropic thinking block）
    pub raw_reasoning_content: Value,
    pub reasoning_content: Option<Arc<String>>,
    /// 工具调用内容
    pub raw_tool_calls: Value,
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 其他信息
    pub finish_reason: Option<Arc<String>>,
    pub total_tokens: u64,
}

/// OpenAI 兼容上下文消息：role 即枚举变体（内部标签序列化，role 与其他字段平级）
/// 字段按编码规范用 Arc<String>（Option 内同样 Arc 包裹）；tool_calls 为 Vec 不包裹
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System { content: Arc<String> },
    User { content: Arc<String> },
    Assistant {
        #[serde(default, skip_serializing_if = "Value::is_null")]
        content: Value,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        reasoning_content: Value,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        tool_calls: Value,
    },
    Tool {
        tool_call_id: Arc<String>,
        /// 调用结果
        content: Arc<String>,
    },
}
