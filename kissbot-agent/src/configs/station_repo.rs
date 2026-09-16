use std::sync::Arc;

use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;


/// station 可改配置，持久化到 <data_dir>/station.json
/// 全局 Station 每 agent 一个：本地 toolkit 集合 + 直接子 Station 集合
/// （子只能 HTTP 通信，父只存连接信息；toolkit 名全局唯一命名空间，含子 Station 不能重名）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationRepo {
    /// 本站点唯一标识（必填；被其他 station 作为 sub 调用时用于祖先链防环）
    pub station_id: Arc<String>,
    /// 本地 toolkit 集合（key = toolkit 名）
    /// serde(default)：旧 station.json 缺省 toolkits 时反序列化为空 map
    #[serde(default)]
    pub toolkits: Arc<ArcSwapHashMap<String, ToolkitConfig>>,
    /// 直接子 Station 集合（key = station_id；孙子由子进程自己递归，父不管）
    /// serde(default)：旧 station.json 缺省 sub_stations 时反序列化为空 map
    #[serde(default)]
    pub sub_stations: Arc<ArcSwapHashMap<String, SubStationConfig>>,
}

impl Default for StationRepo {
    fn default() -> Self {
        Self {
            station_id: Arc::new(String::new()),
            toolkits: Arc::new(ArcSwapHashMap::new()),
            sub_stations: Arc::new(ArcSwapHashMap::new()),
        }
    }
}

/// Toolkit 配置（StationRepo.toolkits 的 value；key = toolkit 名）
/// Toolkit 中无子 Station；内置 toolkit（如 filesystem）由内置注册表填充元数据与实现，
/// 配置声明的 tools/mcps 作为补充（仅元数据注册，无本地实现时调用返回未实现）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolkitConfig {
    /// 工具元数据（key = 工具名）
    #[serde(default)]
    pub tools: Arc<ArcSwapHashMap<String, ToolConfig>>,
    /// MCP 元数据（key = mcp 名；本轮占位，无实现）
    #[serde(default)]
    pub mcps: Arc<ArcSwapHashMap<String, McpConfig>>,
}

/// MCP 配置（占位：本轮仅建结构，不实现调用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    pub name: Arc<String>,
    pub description: Arc<String>,
}

/// 子 Station 配置（StationRepo.sub_stations 的 value；key = station_id）
/// 只存直接子连接信息；子 Station 内部结构（toolkits/孙子）由子进程自己管理，父通过 HTTP 查询
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubStationConfig {
    pub station_id: Arc<String>,
    pub base_url: Arc<String>,
    pub timeout_secs: u64,
}

/// 工具配置（ToolkitConfig.tools 的 value；name 与 map key 一致）
/// 字段按编码规范用 Arc<String>/Arc<Value>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub name: Arc<String>,
    pub description: Arc<String>,
    pub parameters: Arc<Value>,
}
