use serde::Deserialize;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub root_dir: PathBuf,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("memory")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_get() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        let json_content = format!(r#"{{"memory": {{"root_dir": "{}"}}}}"#, root_dir_str.replace('\\', "/"));
        std::fs::write(&config_path, json_content).unwrap();

        // SAFETY: test environment, single-threaded, no concurrent env access
        unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
        let config = Config::get();
        assert_eq!(config.root_dir, dir.path());
        // 清理 env var（OnceLock 已缓存，不会影响其他测试）
        unsafe { std::env::remove_var("KISSBOT_CONFIG"); }
    }
}
