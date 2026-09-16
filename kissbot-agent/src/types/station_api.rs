use serde::{Deserialize, Serialize};

use crate::types::ToolCall;

// ========== Station HTTP 协议 DTO ==========

/// 子 Station 工具元数据查询请求（filter：toolkit 白名单；ancestors：根到当前父节点的 station_id 链）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationListToolsRequest {
    #[serde(default)]
    pub filter: Option<Vec<String>>,
    #[serde(default)]
    pub ancestors: Vec<String>,
}

/// 子 Station MCP 元数据查询请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationListMcpsRequest {
    #[serde(default)]
    pub filter: Option<Vec<String>>,
    #[serde(default)]
    pub ancestors: Vec<String>,
}

/// 子 Station 工具调用请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationCallToolRequest {
    pub tool_call: ToolCall,
    pub ancestors: Vec<String>,
}

/// 子 Station 工具调用请求
#[derive(Debug, Serialize)]
pub struct StationCallToolRequestRef<'a> {
    pub tool_call: &'a ToolCall,
    pub ancestors: &'a [String],
}
