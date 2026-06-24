use std::sync::Once;

/// 初始化 kissbot_memory::Config 使其指向一个临时目录。
/// 多次调用只生效一次。
/// TempDir 在闭包结束时 drop，但 Config::get() 已将 root_dir 读入 OnceLock 内存。
pub fn init_test_config() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let dir = tempfile::tempdir().expect("create tempdir");
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        std::fs::write(&config_path, format!(r#"{{"root_dir":"{}"}}"#, root_dir_str))
            .expect("write config");
        // SAFETY: 单线程测试环境，无并发 env 访问
        unsafe { std::env::set_var("KISSBOT_MEMORY_CONFIG", config_path.to_str().unwrap()); }
        kissbot_memory::Config::get();
    });
}
