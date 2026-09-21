use std::sync::Arc;

use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};

/// station 可改配置，持久化到 <data_dir>/station.json
/// 全局 Station 每 agent 一个：本地 group 集合 + 直接子 Station 集合
/// （子 Station 通过 MCP 与父通信；group 名构成整树唯一命名空间）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationRepo {
    /// 本站点唯一标识（必填；被其他 station 作为 sub 调用时用于祖先链防环）
    pub station_id: Arc<String>,
    /// 本级加载哪些 group（只声明 group 名；工具元数据与实现由内置注册表按 group 名填充）
    /// serde(default)：旧 station.json 缺省 groups 时反序列化为空列表
    #[serde(default)]
    pub groups: Vec<String>,
    /// 直接子 Station 集合（key = station_id；孙子由子进程自己递归，父不管）
    /// serde(default)：旧 station.json 缺省 sub_stations 时反序列化为空 map
    #[serde(default)]
    pub sub_stations: Arc<ArcSwapHashMap<String, SubStationConfig>>,
}

impl Default for StationRepo {
    fn default() -> Self {
        Self {
            station_id: Arc::new(String::new()),
            groups: Vec::new(),
            sub_stations: Arc::new(ArcSwapHashMap::new()),
        }
    }
}

/// 子 Station 配置（StationRepo.sub_stations 的 value；key = station_id）
/// 只存直接子连接信息；子 Station 内部结构由子进程自己管理，父通过 MCP 查询
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubStationConfig {
    pub station_id: Arc<String>,
    /// 子 Station 基地址（MCP 端点为 base_url + /mcp）
    pub base_url: Arc<String>,
    pub timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<Arc<String>>,
}
