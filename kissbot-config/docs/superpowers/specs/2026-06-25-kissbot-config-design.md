# kissbot-config 公共配置组件设计

## 概述

统一 kissbot 项目各组件（memory、memory-store、memory-ego、channel-web 等）的配置管理方式。避免各组件各自独立加载配置文件的现状，改为一个公共配置组件负责加载，各组件只定义自己需要的结构并从公共配置中提取。

## 设计原则

- **单一文件**：一份 JSON 配置文件，通过环境变量 `KISSBOT_CONFIG` 指定路径，默认 `./config.json`
- **层级结构**：JSON 按组件层级组织，对应 Rust 嵌套 struct
- **只读**：公共 Config 只加载一次（`OnceLock`），运行时不可变
- **解耦**：`kissbot-config` crate 不依赖任何业务组件，各组件各自定义自己的 Config struct
- **可读写配置不属于 Config**：channel-web 的 messenger 可读写配置改名为 Repo，使用独立文件

## 新增组件：kissbot-config

### 位置

`/home/admin/project/kissbot/kissbot-config/`

### 依赖

- serde (derive)
- serde_json

不依赖 config crate、tokio、或其他业务组件。

### API

```rust
// kissbot-config/src/lib.rs

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
pub struct Config {
    raw: serde_json::Value,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    /// 从环境变量 KISSBOT_CONFIG 指定路径加载 JSON 文件
    /// 未设置时默认读取 ./config.json
    pub fn load() -> Result<Self, ConfigError>;

    /// 获取全局单例，首次调用时自动加载
    /// 加载失败时 panic（配置错误的fail-fast）
    pub fn get() -> &'static Self;

    /// 从配置的 JSON 结构中导航到指定路径，反序列化为 T
    ///
    /// path 使用点号分隔，如 "memory.store"
    /// 从 raw 中逐层导航：raw["memory"]["store"]
    /// 路径不存在或类型不匹配时 panic
    pub fn get_section<T: DeserializeOwned>(&self, path: &str) -> T;
}
```

`get_section` 实现：

```rust
pub fn get_section<T: DeserializeOwned>(&self, path: &str) -> T {
    let mut cursor = &self.raw;
    for key in path.split('.') {
        cursor = cursor.get(key)
            .unwrap_or_else(|| panic!("kissbot-config: section '{path}' not found"));
    }
    serde_json::from_value(cursor.clone())
        .unwrap_or_else(|e| panic!("kissbot-config: section '{path}' type mismatch: {e}"))
}
```

### 错误类型

```rust
#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Json(serde_json::Error),
}
```

## JSON 结构

```json
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

各组件不感知所有层级——`memory` 只取自己的段，`memory.store` 只取子段，`memory.ego` 只取自己的子段。组件之间通过路径字符串解耦。

## 各组件改造

### kissbot-memory

```rust
// kissbot-memory/src/config.rs

#[derive(Deserialize)]
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

移除：`config` crate 依赖、`Config::load()`、`KISSBOT_MEMORY_CONFIG` env var。

### kissbot-memory-store

```rust
// kissbot-memory-store/src/config.rs

#[derive(Deserialize)]
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

移除：`config` crate 依赖、`Config::load()`、`KISSBOT_MEMORY_STORE_CONFIG` env var。

### kissbot-memory-ego

```rust
// kissbot-memory-ego/src/config.rs

#[derive(Deserialize)]
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

移除：`config` crate 依赖、`Config::load()`、`KISSBOT_MEMORY_EGO_CONFIG` env var。

### kissbot-channel-web

#### 只读部分（App Config）

放入公共 Config`channel-web` 段，使用 `get_section("channel-web")`。

```rust
#[derive(Deserialize)]
pub struct Config {
    pub messenger_repo: String,
    pub attachment_dir: String,
    pub memory_store_url: String,
    pub ws_listen_addr: String,
    pub http_listen_addr: String,
}
```

#### 可读写部分（Repo）

原 `MessengerConfig` 改名为 `WebMessengerRepo`，保持可读写（`DashMap` + `Arc<RwLock<>>`）。使用独立文件，路径由公共 Config 的 `channel-web.messenger_repo` 指定。Repo 文件不参与公共 Config 加载。

移除：`config` crate 依赖（channel-web 原有 Config 改用 get_section 后不再需要）。

## 迁移计划

1. 创建 `kissbot-config` crate
2. 在各组件 Cargo.toml 中添加 `kissbot-config` 依赖，移除 `config` 依赖
3. 逐步改造各组件 config.rs（可逐个组件进行，互不影响）
4. 创建根级 `config.json`，汇总所有组件配置
5. 删除各组件独立 config.json（数据已迁移）
6. channel-web 可读写部分改名 Repo

## 不覆盖的范围

- `kissbot-agent` 的配置（使用独立 `ConfigManager` 管理系统，不参与本次改造）
- `kai-rs` 系列 crate（无配置需求）
- 配置热更新（公共 Config 本次设计为只读、启动时加载一次）
