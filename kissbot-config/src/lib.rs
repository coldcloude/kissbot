use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Deserialize)]
pub struct Config {
    raw: serde_json::Value,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    /// 从环境变量 KISSBOT_CONFIG 指定路径加载 JSON 文件
    /// 未设置时默认读取 ./config.json
    /// 加载失败时 panic（fail-fast）
    fn load() -> Self {
        let config_path = std::env::var("KISSBOT_CONFIG")
            .map(|p| PathBuf::from(p))
            .unwrap_or_else(|_| PathBuf::from("config.json"));

        let content = std::fs::read_to_string(config_path)
            .expect("kissbot-config: failed to read config file");
        let raw: serde_json::Value = serde_json::from_str(&content)
            .expect("kissbot-config: failed to parse config JSON");
        Self { raw }
    }

    /// 获取全局单例，首次调用时自动加载
    /// 加载失败时 panic（配置错误的 fail-fast）
    pub fn get() -> &'static Self {
        CONFIG.get_or_init(|| Config::load())
    }

    /// 从配置的 JSON 结构中导航到指定路径，反序列化为 T
    ///
    /// path 使用点号分隔，如 "memory.store"
    /// 从 raw 中逐层导航：raw["memory"]["store"]
    /// 路径不存在或类型不匹配时 panic
    pub fn get_section<T: DeserializeOwned>(&self, path: &str) -> T {
        let mut cursor = &self.raw;
        for key in path.split('.') {
            cursor = cursor.get(key)
                .unwrap_or_else(|| panic!("kissbot-config: section '{path}' not found"));
        }
        serde_json::from_value(cursor.clone())
            .unwrap_or_else(|e| panic!("kissbot-config: section '{path}' type mismatch: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    #[should_panic(expected = "section 'nonexistent' not found")]
    fn test_get_section_panics_on_missing_path() {
        let raw: serde_json::Value = serde_json::from_str(r#"{"key": "value"}"#).unwrap();
        let cfg = Config { raw };
        let _val: String = cfg.get_section("nonexistent");
    }

    #[test]
    fn test_get_section_simple() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let content = r#"{"memory": {"root_dir": "data"}}"#;
        std::fs::write(&config_path, content).unwrap();

        // Config::get() 只初始化一次，用子进程测试就要重置 OnceLock
        // 这里用 load() 不可行了，所以直接构造一个 Config 测试
        let raw: serde_json::Value = serde_json::from_str(content).unwrap();
        let cfg = Config { raw };

        #[derive(Deserialize)]
        struct MemCfg {
            root_dir: String,
        }
        let mem: MemCfg = cfg.get_section("memory");
        assert_eq!(mem.root_dir, "data");
    }

    #[test]
    fn test_get_section_nested() {
        let content = r#"{"memory": {"store": {"port": 8082, "host": "127.0.0.1"}}}"#;
        let raw: serde_json::Value = serde_json::from_str(content).unwrap();
        let cfg = Config { raw };

        #[derive(Deserialize)]
        struct StoreCfg {
            port: u16,
            host: String,
        }
        let store: StoreCfg = cfg.get_section("memory.store");
        assert_eq!(store.port, 8082);
        assert_eq!(store.host, "127.0.0.1");
    }
}
