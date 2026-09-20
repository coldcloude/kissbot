use std::sync::Arc;

use kissbot_api::ArcSwapHashMap;
use kissbot_mcp::Tool;
use serde::{Deserialize, Serialize};

/// station 可改配置，持久化到 <data_dir>/station.json
/// 全局 Station 每 agent 一个：本地 group 集合 + 直接子 Station 集合
/// （子 Station 通过 MCP 与父通信；group 名构成整树唯一命名空间）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationRepo {
    /// 本站点唯一标识（必填；被其他 station 作为 sub 调用时用于祖先链防环）
    pub station_id: Arc<String>,
    /// 本地 group 集合（key = group 名；每个 group 是一组工具）
    /// serde(default)：旧 station.json 缺省 groups 时反序列化为空 map
    #[serde(default)]
    pub groups: Arc<ArcSwapHashMap<String, GroupConfig>>,
    /// 直接子 Station 集合（key = station_id；孙子由子进程自己递归，父不管）
    /// serde(default)：旧 station.json 缺省 sub_stations 时反序列化为空 map
    #[serde(default)]
    pub sub_stations: Arc<ArcSwapHashMap<String, SubStationConfig>>,
}

impl Default for StationRepo {
    fn default() -> Self {
        Self {
            station_id: Arc::new(String::new()),
            groups: Arc::new(ArcSwapHashMap::new()),
            sub_stations: Arc::new(ArcSwapHashMap::new()),
        }
    }
}

/// group 配置（StationRepo.groups 的 value；key = group 名）
/// tools 为工具元数据（MCP Tool：名称/描述/输入 JSON Schema）；
/// 内置 group（如 filesystem）由内置注册表填充元数据与实现，配置声明的 tools 作元数据补充
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupConfig {
    /// 工具元数据（key = 工具名）
    #[serde(default)]
    pub tools: Arc<ArcSwapHashMap<String, Tool>>,
}

/// 子 Station 配置（StationRepo.sub_stations 的 value；key = station_id）
/// 只存直接子连接信息；子 Station 内部结构由子进程自己管理，父通过 MCP 查询
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubStationConfig {
    pub station_id: Arc<String>,
    /// 子 Station 基地址（MCP 端点为 base_url + /mcp）
    pub base_url: Arc<String>,
    pub timeout_secs: u64,
    /// 子 Station 的默认 group：对端不支持 kissbot 分组扩展、或工具列表响应不带
    /// `kissbot/group` 时，其工具整体归入该 group；缺省（空串）时回落为 station_id
    #[serde(default)]
    pub default_group: Arc<String>,
}

impl SubStationConfig {
    /// 生效的默认 group（未配置时回落为 station_id）
    pub fn effective_group(&self) -> &str {
        if self.default_group.is_empty() {
            self.station_id.as_str()
        } else {
            self.default_group.as_str()
        }
    }
}
