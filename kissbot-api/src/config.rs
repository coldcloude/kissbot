use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    pub memory_store_url: String,
    pub memory_ego_url: String,
    /// 摘要记忆（memory-struct-abstract）服务地址；缺省为空 = 未部署（输入处理器降级为普通合批）
    #[serde(default)]
    pub memory_struct_abstract_url: String,
}

impl ApiConfig {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<ApiConfig> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("api")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abstract_url_defaults_to_empty_when_absent() {
        // 旧配置没有该字段 → 缺省为空串（表示摘要记忆未部署）
        let cfg: ApiConfig = serde_json::from_value(serde_json::json!({
            "memory_store_url": "http://127.0.0.1:8082",
            "memory_ego_url": "http://127.0.0.1:3001"
        }))
        .unwrap();
        assert_eq!(cfg.memory_store_url, "http://127.0.0.1:8082");
        assert_eq!(cfg.memory_struct_abstract_url, "");
    }

    #[test]
    fn abstract_url_is_read_when_present() {
        let cfg: ApiConfig = serde_json::from_value(serde_json::json!({
            "memory_store_url": "",
            "memory_ego_url": "",
            "memory_struct_abstract_url": "http://127.0.0.1:8083"
        }))
        .unwrap();
        assert_eq!(cfg.memory_struct_abstract_url, "http://127.0.0.1:8083");
    }
}
