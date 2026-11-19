use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use arcstr::ArcStr;
use serde::{Deserialize, Serialize};

/// station 可改配置，持久化到 <data_dir>/station.json
/// 全局 Station 每 agent 一个：本地 group 集合 + 直接子 Station 集合
/// （子 Station 通过 MCP 与父通信；group 名构成整树唯一命名空间）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationRepo {
    /// 本站点唯一标识（必填；被其他 station 作为 sub 调用时用于祖先链防环）
    pub station_id: ArcStr,
    /// 本级加载哪些 group（只声明 group 名；工具元数据与实现由内置注册表按 group 名填充）
    /// 运行期可经管理 API 增删（Arc::make_mut），BTreeSet 保证落盘顺序稳定
    /// serde(default)：旧 station.json 缺省 groups 时反序列化为空集合
    #[serde(default)]
    pub groups: Arc<BTreeSet<ArcStr>>,
    /// 直接子 Station 集合（key = station_id；孙子由子进程自己递归，父不管）
    /// serde(default)：旧 station.json 缺省 sub_stations 时反序列化为空 map
    #[serde(default)]
    pub sub_stations: Arc<HashMap<ArcStr, Arc<SubStationConfig>>>,
}

impl Default for StationRepo {
    fn default() -> Self {
        Self {
            station_id: ArcStr::new(),
            groups: Arc::new(BTreeSet::new()),
            sub_stations: Arc::new(HashMap::new()),
        }
    }
}

/// 子 Station 配置（StationRepo.sub_stations 的 value；key = station_id）
/// 只存直接子连接信息；子 Station 内部结构由子进程自己管理，父通过 MCP 查询
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubStationConfig {
    pub station_id: ArcStr,
    /// 子 Station 基地址（MCP 端点为 base_url + /mcp）
    pub base_url: ArcStr,
    pub timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<ArcStr>,
    /// 是否建立运行态：false 时不建 MCP 客户端，其工具不参与列表与路由（运行期可经管理 API 启停）
    /// serde(default = true)：旧 station.json 缺省 enabled 时按启用处理
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl SubStationConfig {
    /// 生效的默认 group（根据 station_id 构造）
    pub fn default_group(&self) -> ArcStr {
        ArcStr::from(format!("[{}]", self.station_id))
    }

    /// 工具名在本级的全局名（加 prefix）
    pub fn global_name(&self, name: ArcStr) -> ArcStr {
        if let Some(prefix) = self.prefix.as_ref() {
            ArcStr::from(format!("{}{}", prefix.as_str(), name.as_str()))
        } else {
            name
        }
    }

    /// 本级全局名还原为子站的本地名（去 prefix）
    pub fn local_name(&self, name: ArcStr) -> ArcStr {
        if let Some(prefix) = self.prefix.as_ref() {
            if let Some(n) = name.strip_prefix(prefix.as_str()) {
                return ArcStr::from(n);
            }
        }
        name
    }
}
