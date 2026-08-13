# ConfigManager 全局单例化 + 单例访问函数统一命名 get()

日期：2026-08-14
状态：设计已确认

## 1. 目标

`ConfigManager` 改为进程级全局单例：任何模块读配置直接 `ConfigManager::get()` 获取，
不再通过字段持有 / 参数传递引用（AgentCoordinator、ChannelManager、ModelClient、HttpServer、command_router 全部去除）。

同时把 `AgentCoordinator::instance()` 改名为 `get()`，与 `ConfigManager::get()`、`ApiConfig::get()`、`SecurityConfig::get()` 统一。

改造范围仅 `kissbot-agent`。

## 2. 核心决策

| # | 决策 | 说明 |
|---|------|------|
| 1 | 单例模式：std `OnceLock` + 显式 `init()` | `pub async fn init() -> Result<()>`（内部 OnceLock::set）；`pub fn get() -> &'static Self`（未初始化 panic）。与 `AgentCoordinator` 同一模式（async 构建无法惰性 `get_or_init`，显式 init 让失败在启动早期明确上报） |
| 2 | `ConfigManager::new()` 私有化 | 仅供 `init()` 内部调用；测试同样走 `init()` |
| 3 | 访问函数统一改名 `get()` | `AgentCoordinator::instance()` → `AgentCoordinator::get()`；新增 `ConfigManager::get()`；与 `ApiConfig::get()` / `SecurityConfig::get()` 命名一致 |
| 4 | 各结构去除 config 引用 | Coordinator / ChannelManager / ModelClient / HttpServer 删字段，command_router 删参数，一律 `ConfigManager::get()` |
| 5 | 装配顺序：init → coordinator → http_server | `main.rs` 第一步 `ConfigManager::init().await.expect(...)`，随后 `AgentCoordinator::new()`（无参）、`HttpServer::new()`（无参）均从单例取 |
| 6 | 测试走单例 | `http_server::tests::test_manager` 改为 `ConfigManager::init()`（KISSBOT_CONFIG 指向 tempdir）；进程内 ConfigManager 全局唯一 |
| 7 | 内部状态与写路径不变 | `RwLock<NexusRepo>`、update/add/remove 回写、落盘、listeners 机制全部不动 |

## 3. 改动清单

### 3.1 config_manager.rs

- 新增 `static INSTANCE: OnceLock<ConfigManager>`
- `pub async fn new()` → 私有 `async fn new()`；新增 `pub async fn init() -> Result<()>`、`pub fn get() -> &'static ConfigManager`
- 注释更新：全局单例说明（与 AgentCoordinator 同模式）

### 3.2 coordinator.rs

- `AgentCoordinator::instance()` 改名 `get()`（定义、注释、spawn 消费者内调用）
- 删字段 `pub config: Arc<ConfigManager>`；`new()` 去 config 参数（无参）
- ~20 处 `self.config.xxx()` → `ConfigManager::get().xxx()`

### 3.3 main.rs

- `ConfigManager::new()` → `ConfigManager::init().await.expect(...)`
- `AgentCoordinator::new(config.clone())` → `AgentCoordinator::new()`
- `HttpServer::new(mgr_config)` → `HttpServer::new()`
- `AgentCoordinator::instance().run()` → `AgentCoordinator::get().run()`

### 3.4 channel_manager.rs

- 删字段 `config: Arc<ConfigManager>`、`new()` 去参数
- 重连循环内实时读绑定：`ConfigManager::get().channel(...)`（每次调用取，行为不变）
- 回调 `AgentCoordinator::instance()` → `AgentCoordinator::get()`

### 3.5 model_client.rs

- 删字段 `config_manager: Arc<ConfigManager>`、`new()` 去参数
- `call` / `list_models` 内改 `ConfigManager::get()`

### 3.6 http_server.rs

- 删字段 `config: Arc<ConfigManager>`、`new()` 去参数
- 各 endpoint 内改 `ConfigManager::get()`
- 测试 `test_manager` 改为 `ConfigManager::init()`（tempdir data_dir）；注释同步（单例 init 一次）

### 3.7 command_router.rs

- `check_admin` / `execute` 删 `config: &ConfigManager` 参数，改 `ConfigManager::get()`
- `channel_current_key` 内 `AgentCoordinator::instance()` → `AgentCoordinator::get()`

### 3.8 session_manager.rs

- `AgentCoordinator::instance()` → `AgentCoordinator::get()`

## 4. 数据流 / 错误处理 / 测试

- **读**：任意模块 `ConfigManager::get().channels()` 等，行为不变（配置永远最新）
- **写**：管理命令回写 → 内部 RwLock → 落盘，不变；listeners 机制保留（现无注册点）
- **失败**：`init()` 失败在 main 里 `expect` 退出；未初始化调 `get()` panic（与 coordinator 一致）
- **测试**：http_server 测试 init 一次（tempdir）；channel_manager / command_router 测试不依赖 ConfigManager，不受影响；coordinator 测试（verify_agent_exists 保留分支）不触配置，不受影响

## 5. 已知取舍

测试进程内 ConfigManager 全局唯一（data_dir 固定一次）。未来若出现需要不同 data_dir 的多组测试，需引入 test reset 机制——按 YAGNI 暂不做。

## 6. 验证

- `cargo check` 通过
- `cargo test` 全绿（现有 116 个测试）
