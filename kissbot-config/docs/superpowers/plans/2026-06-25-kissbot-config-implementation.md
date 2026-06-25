# kissbot-config 公共配置组件实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 创建 `kissbot-config` crate，改造 4 个组件的 config.rs 使其从公共配置中提取配置。

**Architecture:** 新增 `kissbot-config` crate（纯 serde_json + OnceLock），定义 `get_section<T>(path)` API。各组件删除独立 config 文件加载逻辑，改为调用 `kissbot_config::Config::get().get_section()`，使用自身 OnceLock 做缓存。channel-web 的可读写配置改名 Repo。

**Tech Stack:** Rust, serde, serde_json

## 全局约束

- `kissbot-config` crate 不依赖 config crate、tokio、或其他业务组件
- 只依赖 serde (derive) + serde_json
- `get_section()` 路径不存在或类型不匹配时 panic（必填配置 fail-fast）
- 各组件 Config struct 使用自身 `static INSTANCE: OnceLock<T>` 做缓存，不直接暴露 kissbot-config 内部类型
- channel-web 可读写部分改名为 Repo，独立文件

---

## 文件结构

| 文件 | 操作 | 说明 |
|------|------|------|
| `kissbot-config/Cargo.toml` | 创建 | 新 crate，依赖 serde + serde_json |
| `kissbot-config/src/lib.rs` | 创建 | Config struct、load、get、get_section |
| `kissbot-config/src/error.rs` | 创建 | ConfigError 类型 |
| `kissbot-memory/Cargo.toml` | 修改 | 加 kissbot-config 依赖，移 config 依赖 |
| `kissbot-memory/src/config.rs` | 修改 | 改用 get_section("memory") |
| `kissbot-memory-store/Cargo.toml` | 修改 | 加 kissbot-config 依赖，移 config 依赖 |
| `kissbot-memory-store/src/config.rs` | 修改 | 改用 get_section("memory.store") |
| `kissbot-memory-ego/Cargo.toml` | 修改 | 加 kissbot-config 依赖，移 config 依赖 |
| `kissbot-memory-ego/src/config.rs` | 修改 | 改用 get_section("memory.ego") |
| `kissbot-channel-web/Cargo.toml` | 修改 | 加 kissbot-config 依赖，移 config 依赖 |
| `kissbot-channel-web/src/config.rs` | 修改 | 改用 get_section("channel-web") |
| `kissbot-channel-web/src/messenger.rs` | 修改 | MessengerConfig → WebMessengerRepo 改名 |
| `kissbot-channel-web/src/main.rs` | 修改 | 适配 Repo 的构造方式 |
| `config.json` | 创建 | 根级公共配置文件 |

---

### Task 1: 创建 kissbot-config crate

**Files:**
- Create: `kissbot-config/Cargo.toml`
- Create: `kissbot-config/src/lib.rs`
- Create: `kissbot-config/src/error.rs`

**Interfaces:**
- Produces: `kissbot_config::Config { fn load() -> Result, fn get() -> &'static Self, fn get_section<T: DeserializeOwned>(path) -> T }`
- Produces: `kissbot_config::ConfigError { Io(io::Error), Json(serde_json::Error) }`

- [ ] **Step 1: 创建 Cargo.toml**

```toml
[package]
name = "kissbot-config"
version = "0.1.0"
edition = "2024"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

- [ ] **Step 2: 创建 error.rs**

```rust
use std::fmt;

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "IO error: {e}"),
            ConfigError::Json(e) => write!(f, "JSON error: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self { ConfigError::Io(e) }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self { ConfigError::Json(e) }
}
```

- [ ] **Step 3: 创建 lib.rs**

```rust
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::OnceLock;

mod error;
pub use error::ConfigError;

#[derive(Deserialize)]
pub struct Config {
    raw: serde_json::Value,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    /// 从环境变量 KISSBOT_CONFIG 指定路径加载 JSON 文件
    /// 未设置时默认读取 ./config.json
    pub fn load() -> Result<Self, ConfigError> {
        let config_path = std::env::var("KISSBOT_CONFIG")
            .map(|p| PathBuf::from(p))
            .unwrap_or_else(|_| PathBuf::from("config.json"));

        let content = std::fs::read_to_string(config_path)?;
        let raw: serde_json::Value = serde_json::from_str(&content)?;
        Ok(Self { raw })
    }

    /// 获取全局单例，首次调用时自动加载
    /// 加载失败时 panic（配置错误的 fail-fast）
    pub fn get() -> &'static Self {
        CONFIG.get_or_init(|| {
            Config::load().expect("kissbot-config: failed to load config")
        })
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

    #[test]
    fn test_load_nonexistent() {
        unsafe { std::env::set_var("KISSBOT_CONFIG", "/tmp/nonexistent-config-test.json"); }
        let result = Config::load();
        assert!(result.is_err());
        unsafe { std::env::remove_var("KISSBOT_CONFIG"); }
    }

    #[test]
    fn test_get_section_simple() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let content = r#"{"memory": {"root_dir": "data"}}"#;
        std::fs::write(&config_path, content).unwrap();

        unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
        let cfg = Config::load().unwrap();

        #[derive(Deserialize)]
        struct MemCfg {
            root_dir: String,
        }
        let mem: MemCfg = cfg.get_section("memory");
        assert_eq!(mem.root_dir, "data");
        unsafe { std::env::remove_var("KISSBOT_CONFIG"); }
    }

    #[test]
    fn test_get_section_nested() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let content = r#"{"memory": {"store": {"port": 8082, "host": "127.0.0.1"}}}"#;
        std::fs::write(&config_path, content).unwrap();

        unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
        let cfg = Config::load().unwrap();

        #[derive(Deserialize)]
        struct StoreCfg {
            port: u16,
            host: String,
        }
        let store: StoreCfg = cfg.get_section("memory.store");
        assert_eq!(store.port, 8082);
        assert_eq!(store.host, "127.0.0.1");
        unsafe { std::env::remove_var("KISSBOT_CONFIG"); }
    }
}
```

- [ ] **Step 4: 添加 tempfile dev-dependency**

在 `kissbot-config/Cargo.toml` 末尾添加：

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 5: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-config && cargo test 2>&1
```

Expected: 3 tests pass, no warnings.

- [ ] **Step 6: 创建根级 config.json**

```json
/home/admin/project/kissbot/config.json

{
  "memory": {
    "root_dir": "data",
    "store": {
      "listen_addr": "127.0.0.1",
      "listen_port": 8082,
      "api_key": "memory-store-key"
    },
    "ego": {
      "listen_addr": "127.0.0.1",
      "listen_port": 3001,
      "api_key": "memory-ego-key"
    }
  },
  "channel-web": {
    "messenger_repo": "channel-web-repo.json",
    "attachment_dir": "attachments",
    "memory_store_url": "http://127.0.0.1:8102",
    "ws_listen_addr": "127.0.0.1:8201",
    "http_listen_addr": "127.0.0.1:8301"
  }
}
```

- [ ] **Step 7: Commit**

```bash
git add kissbot-config/
git commit -m "feat: 创建 kissbot-config 公共配置组件"
```

---

### Task 2: 改造 kissbot-memory config.rs

**Files:**
- Modify: `kissbot-memory/Cargo.toml`
- Modify: `kissbot-memory/src/config.rs`

**Interfaces:**
- Consumes: `kissbot_config::Config::get().get_section::<T>(path)` from Task 1
- Produces: `kissbot_memory::config::Config { pub root_dir: PathBuf }` with `Config::get() -> &'static Config`

- [ ] **Step 1: 修改 Cargo.toml**

在 `kissbot-memory/Cargo.toml` 中：
- 删除 `config = "0.15"` 行
- 添加 `kissbot-config = { path = "../kissbot-config" }`

- [ ] **Step 2: 重写 config.rs**

```rust
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
```

- [ ] **Step 3: 检查引用 `Config::load()` 的地方**

`kissbot-memory` 中是否有其他地方直接调用 `Config::load()`？搜索 `Config::load` 和 `KISSBOT_MEMORY_CONFIG`。

Also check `MemoryConfig` alias — in `DirectoryManager::get()` it references `crate::Config::get()` which is fine (just the singleton accessor).

- [ ] **Step 4: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test 2>&1 | grep -E "test result|error"
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add kissbot-memory/Cargo.toml kissbot-memory/src/config.rs
git commit -m "refactor: kissbot-memory config 改用公共 Config"
```

---

### Task 3: 改造 kissbot-memory-store config.rs

**Files:**
- Modify: `kissbot-memory-store/Cargo.toml`
- Modify: `kissbot-memory-store/src/config.rs`

**Interfaces:**
- Consumes: `kissbot_config::Config::get().get_section::<T>(path)` from Task 1
- Produces: `kissbot_memory_store::config::Config { listen_addr, listen_port, api_key }` with `Config::get() -> &'static Config`

- [ ] **Step 1: 修改 Cargo.toml**

在 `kissbot-memory-store/Cargo.toml` 中：
- 删除 `config = "0.15"` 行
- 添加 `kissbot-config = { path = "../kissbot-config" }`

- [ ] **Step 2: 重写 config.rs**

```rust
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub listen_addr: String,
    pub listen_port: u16,
    pub api_key: String,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("memory.store")
        })
    }
}
```

- [ ] **Step 3: 移除 `load_existing_file_state` 不再使用的 `use crate::error::Result`**

检查 `config.rs` 现在是否还用到 `Result`——不需要了（`load()` 方法已删除）。

- [ ] **Step 4: 编译+测试验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-store && cargo test 2>&1
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add kissbot-memory-store/Cargo.toml kissbot-memory-store/src/config.rs
git commit -m "refactor: kissbot-memory-store config 改用公共 Config"
```

---

### Task 4: 改造 kissbot-memory-ego config.rs

**Files:**
- Modify: `kissbot-memory-ego/Cargo.toml`
- Modify: `kissbot-memory-ego/src/config.rs`

**Interfaces:**
- Consumes: `kissbot_config::Config::get().get_section::<T>(path)` from Task 1
- Produces: `kissbot_memory_ego::config::Config { listen_addr, listen_port, api_key }` with `Config::get() -> &'static Config`

- [ ] **Step 1: 修改 Cargo.toml**

在 `kissbot-memory-ego/Cargo.toml` 中：
- 删除 `config = "0.15"` 行
- 添加 `kissbot-config = { path = "../kissbot-config" }`

- [ ] **Step 2: 重写 config.rs**

```rust
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub listen_addr: String,
    pub listen_port: u16,
    pub api_key: String,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("memory.ego")
        })
    }
}
```

- [ ] **Step 3: 编译+测试验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo test 2>&1
```

Expected: tests pass.

- [ ] **Step 4: Commit**

```bash
git add kissbot-memory-ego/Cargo.toml kissbot-memory-ego/src/config.rs
git commit -m "refactor: kissbot-memory-ego config 改用公共 Config"
```

---

### Task 5: 改造 kissbot-channel-web + Repo 改名

**Files:**
- Modify: `kissbot-channel-web/Cargo.toml`
- Modify: `kissbot-channel-web/src/config.rs`
- Modify: `kissbot-channel-web/src/messenger.rs`
- Modify: `kissbot-channel-web/src/main.rs`

**Note:** This task is larger than the others — consider splitting into subtasks if the messenger.rs changes are extensive.

- [ ] **Step 1: 修改 Cargo.toml**

在 `kissbot-channel-web/Cargo.toml` 中：
- 删除 `config = "0.15"` 行
- 添加 `kissbot-config = { path = "../kissbot-config" }`

- [ ] **Step 2: 重写 config.rs 从公共 Config 加载**

```rust
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub messenger_repo: String,
    pub attachment_dir: String,
    pub memory_store_url: String,
    pub ws_listen_addr: String,
    pub http_listen_addr: String,
}

impl Config {
    pub fn get() -> &'static Self {
        static INSTANCE: OnceLock<Config> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            kissbot_config::Config::get().get_section("channel-web")
        })
    }
}
```

- [ ] **Step 3: MessengerConfig → WebMessengerRepo 改名**

在 `kissbot-channel-web/src/messenger.rs` 中：
- `struct MessengerConfig` → `struct WebMessengerRepo`
- `WebMessengerCreator` 中的字段名和构造函数参数名同步更新
- 文件名本身如果叫 `messenger.rs` 可以不改，但要确认 `WebMessengerRepo` 的 public 接口

检查 `WebMessengerCreator::new()` — 它当前接收 `messenger_config: &str` 路径，用 `serde_json::from_str` 加载。保持这个模式不变，只改名。

```rust
// 改名后的结构
pub struct WebMessengerRepo {
    pub messenger_id: String,
    pub admin_key: String,
    pub user_key: String,
    // ... 其他字段不变
}
```

- [ ] **Step 4: 更新 main.rs 中的引用**

在 `kissbot-channel-web/src/main.rs` 中，找到引用 `MessengerConfig` 的地方改为 `WebMessengerRepo`，`Config::load()` → `kissbot_config::Config::get()` 获取公共 Config。

- [ ] **Step 5: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-channel-web && cargo check 2>&1
```

Expected: compiles without errors.

- [ ] **Step 6: Commit**

```bash
git add kissbot-channel-web/ && git commit -m "refactor: kissbot-channel-web config 改用公共 Config，MessengerConfig 改名为 WebMessengerRepo"
```

---

### Task 6: 清理旧配置文件

**Files:**
- Delete: `kissbot-memory-store/config.json`（数据已迁移到根级 config.json）
- Delete: `kissbot-memory-ego/config.json`（数据已迁移）
- Keep: `kissbot-channel-web/kissbot-channel-web-config.json`（这是 Repo 文件，保持）
- Keep: `kissbot-channel-web/config.json` → 检查是否已被新 config.json 替代

**注意：** `kissbot-memory` 原来没有独立 config.json（它的 config.json 在测试中由 init_test_config 动态创建），所以不需要删除。

- [ ] **Step 1: 检查各目录下旧的独立 config.json**

```bash
ls -la /home/admin/project/kissbot/kissbot-memory-store/config.json
ls -la /home/admin/project/kissbot/kissbot-memory-ego/config.json
ls -la /home/admin/project/kissbot/kissbot-memory/config.json
ls -la /home/admin/project/kissbot/kissbot-channel-web/config.json
```

- [ ] **Step 2: 删除不再需要的独立配置文件**

```bash
rm /home/admin/project/kissbot/kissbot-memory-store/config.json
rm /home/admin/project/kissbot/kissbot-memory-ego/config.json
```

- [ ] **Step 3: 验证每个 crate 编译+测试通过**

```bash
cd /home/admin/project/kissbot/kissbot-memory && cargo test 2>&1 | grep "test result"
cd /home/admin/project/kissbot/kissbot-memory-store && cargo test 2>&1 | grep "test result"
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo test 2>&1 | grep "test result"
cd /home/admin/project/kissbot/kissbot-channel-web && cargo check 2>&1 | grep -E "error|warning"
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "chore: 清理旧独立配置文件，数据已迁移至根级 config.json"
```

---

## Self-Review 检查清单

1. **Spec coverage:**
   - Task 1: kissbot-config crate 创建 — ✅ 完全覆盖 spec 的 API 设计和错误类型
   - Task 2: kissbot-memory 改造 — ✅ get_section("memory") 与 spec 一致
   - Task 3: kissbot-memory-store 改造 — ✅ get_section("memory.store") 与 spec 一致
   - Task 4: kissbot-memory-ego 改造 — ✅ get_section("memory.ego") 与 spec 一致
   - Task 5: channel-web 改造 + Repo 改名 — ✅ 拆分公共 Config 和 Repo
   - Task 6: 清理旧配置文件 — ✅ spec 迁移计划第 5 条
   - 根级 config.json 创建 — ✅ Task 1 Step 6

2. **Placeholder scan:** 无 TBD/TODO/占位符。

3. **Type consistency:** 所有接口签名在任务间一致：`kissbot_config::Config::get() -> &'static Self` + `get_section::<T>(path) -> T`，各组件 `Config::get() -> &'static Self`。
