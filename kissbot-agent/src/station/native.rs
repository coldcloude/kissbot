//! 原生工具：本级自己实现的工具，含实现接口、工具条目、分组与参数 schema，以及内置注册表

use std::collections::BTreeMap;
use std::sync::Arc;

use arcstr::ArcStr;
use async_trait::async_trait;
use kissbot_mcp::{CallToolResult, Tool};
use serde::Serialize;
use serde_json::Value;

use crate::station::filesystem::BUILTIN_GROUP_FILESYSTEM;

/// Station 原生工具实现：本级 Station 自己实现的工具（区别于代理到子 Station 的 MCP 工具）
#[async_trait]
pub trait ToolImplement: Send + Sync {
    async fn call(&self, arguments: Value) -> CallToolResult;
}

// ========== 运行态结构 ==========

/// Station 原生工具：元数据 + 实现
pub struct NativeTool {
    pub config: Arc<Tool>,
    pub implement: Arc<dyn ToolImplement>,
}

/// 参数中单个属性的 JSON Schema
#[derive(Debug, Serialize)]
pub struct PropertySchema {
    pub r#type: ArcStr,
    pub description: ArcStr,
}

/// 内置工具的参数 JSON Schema（固定结构，不做手拼 JSON）
#[derive(Debug, Serialize)]
pub struct ObjectSchema {
    pub r#type: ArcStr,
    pub properties: BTreeMap<ArcStr, PropertySchema>,
    pub required: Vec<ArcStr>,
}

impl ObjectSchema {
    pub fn value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// 分组管理工具类
pub struct Group {
    pub name: ArcStr,
    pub tools: Vec<NativeTool>,
}

impl Group {
    pub fn new(name: ArcStr) -> Self {
        Self {
            name,
            tools: Vec::new(),
        }
    }
    
    pub fn add_tool(&mut self, mut config: Tool, implement: Arc<dyn ToolImplement>) {
        config.set_group(self.name.clone());
        self.tools.push(NativeTool {
            config: Arc::new(config),
            implement,
        });
    }
}

/// 内置注册表：按 group 名给出该 group 的内置工具（元数据 + 原生实现）
/// 配置显式声明对应 group 名时才注册；未声明则不注册（内置工具也须显式配置 group 才可用）
pub fn native_group(name: &str) -> Option<Group> {
    match name {
        n if n == BUILTIN_GROUP_FILESYSTEM => Some(super::filesystem::group()),
        _ => None,
    }
}
