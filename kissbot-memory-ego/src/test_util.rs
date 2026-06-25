use std::sync::{Once, OnceLock};

static TEST_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
static INIT_CONFIG: Once = Once::new();

/// 初始化 kissbot_memory::Config 使其指向一个临时目录。
/// 多次调用只生效一次。
/// OnceLock<TempDir> 持有临时目录，进程退出时自动清理。
pub fn init_test_config() {
    let dir = TEST_DIR.get_or_init(|| tempfile::tempdir().expect("create tempdir"));
    let config_path = dir.path().join("config.json");
    let root_dir_str = dir.path().display().to_string();
    // SAFETY: 单线程测试环境，无并发 env 访问
    // 用 Once 保护 env set_var 和 config 初始化（仅执行一次）
    INIT_CONFIG.call_once(|| {
        std::fs::write(&config_path, format!(r#"{{"root_dir":"{}"}}"#, root_dir_str)).unwrap();
        unsafe { std::env::set_var("KISSBOT_MEMORY_CONFIG", config_path.to_str().unwrap()); }
        kissbot_memory::Config::get();
    });
}
