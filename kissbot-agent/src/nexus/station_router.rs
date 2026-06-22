use dashmap::DashMap;

use crate::nexus::config_manager::StationConfig;

/// Station 路由表，维护已配置的 Station 地址映射
#[allow(dead_code)]
pub struct StationRouter {
    // station_id → StationConfig
    stations: DashMap<String, StationConfig>,
}

impl StationRouter {
    #[allow(dead_code)]
    pub fn new(stations: Vec<StationConfig>) -> Self {
        let map = DashMap::new();
        for s in stations {
            map.insert(s.station_id.clone(), s);
        }
        Self { stations: map }
    }

    /// 更新 Station 列表（配置变更时调用）
    #[allow(dead_code)]
    pub fn update(&self, stations: Vec<StationConfig>) {
        self.stations.clear();
        for s in stations {
            self.stations.insert(s.station_id.clone(), s);
        }
    }

    /// 按 Station ID 查询地址
    #[allow(dead_code)]
    pub fn get_url(&self, station_id: &str) -> Option<String> {
        self.stations.get(station_id).map(|s| s.base_url.clone())
    }

    /// 获取所有 Station ID
    #[allow(dead_code)]
    pub fn list_ids(&self) -> Vec<String> {
        self.stations.iter().map(|e| e.key().clone()).collect()
    }
}
