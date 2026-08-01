# kissbot-config 组件设计

## 概述
公共配置组件，集中管理所有组件的配置信息。作为库模块，各组件通过依赖引入并读取配置。采用单一 JSON 配置文件，按组件层级组织，只读加载。

**配置来源**：环境变量 `KISSBOT_CONFIG` 指定 JSON 文件路径，未设置时默认读取 `./config.json`。

## 核心功能

### Epic 1：配置加载与访问
- **目标**：加载 JSON 配置文件，提供层级化配置访问接口
- **依赖**：无
- **用户故事**：
  - 作为一个外部系统（kissbot 服务进程），我要在启动时从指定路径加载 JSON 配置文件，以初始化运行时配置
  - 作为一个外部系统（配置使用组件），我要通过点号路径导航到配置的指定层级，以获取自己需要的配置段

### Epic 2：API 网络地址配置
- **目标**：统一管理各组件访问其他服务的 URL 地址
- **依赖**：kissbot-config（配置加载）
- **用户故事**：
  - 作为一个外部系统（nexus），我要从公共配置中读取 memory-store 和 memory-ego 的 URL，以初始化记忆读写和认知组件通信

### Epic 3：安全配置
- **目标**：统一管理各组件的 API key 认证密钥
- **依赖**：kissbot-config（配置加载）
- **用户故事**：
  - 作为一个外部系统（HTTP 服务组件），我要从公共配置中读取 api_key，以验证请求来源的认证信息
  - 作为一个外部系统（WS 服务组件），我要从公共配置中读取 api_key，以验证 WebSocket 握手阶段的认证信息
  - 作为一个外部系统（channel-web），我要从公共配置中读取 admin_api_key，以区分管理员和普通用户的访问权限
  - 作为一个外部系统（HTTP 客户端组件），我要从公共配置中读取 api_key，以在请求中附加认证头

## 内部模块

### 1. 公共配置入口（kissbot-config）
- 读取 JSON 文件，持有 `serde_json::Value`
- 提供 `get_section::<T>(path: &str) -> T` 方法，按点号路径从 Value 中导航并反序列化
- 使用 `OnceLock` 全局单例，仅加载一次

### 2. 网络地址配置（kissbot-api::config）
- 定义 `ApiConfig`：memory_store_url、memory_ego_url
- 封装 `get_section("api")`，提供静态 `get()` 方法
- 各组件通过 `kissbot_api::ApiConfig::get()` 获取

### 3. 安全配置（kissbot-security::config）
- 定义 `SecurityConfig`：api_key、admin_api_key
- 封装 `get_section("security")`，提供静态 `get()` 方法
- 各组件通过 `kissbot_security::SecurityConfig::get()` 获取
- 使用 `Arc<String>` 类型，方便跨异步任务传递

## 功能流程

### Epic 1 配置加载流程
服务进程启动 → 首次调用 `Config::get()` → 读取环境变量 `KISSBOT_CONFIG` 或使用默认路径 → 读取 JSON 文件 → 解析为 `serde_json::Value` → 存入 `OnceLock` 全局单例 → 各组件通过 `get_section` 按需提取。

### Epic 2 API 地址配置流程
服务进程启动 → 各组件调用 `ApiConfig::get()` → 触发 `Config::get()` 加载公共配置 → 从 JSON 的 `api` 段提取 `memory_store_url` 和 `memory_ego_url` → 存入 `OnceLock` → 组件通过 `api_config.memory_store_url` 获取。

### Epic 3 安全配置流程
服务进程启动 → 各组件调用 `SecurityConfig::get()` → 触发 `Config::get()` 加载公共配置 → 从 JSON 的 `security` 段提取 `api_key` 和 `admin_api_key` → 存入 `OnceLock` → 认证中间件和服务通过 `security_config.api_key` 获取验证密钥。

## 关键设计

### 配置来源与只读加载
- 配置来源：环境变量 `KISSBOT_CONFIG` 指定 JSON 文件路径，未设置时默认读取 `./config.json`
- 配置按组件层级组织，只读加载，全局单例仅加载一次

### 启动阶段快速失败
- 配置加载失败、配置段不存在或类型不匹配时立即 panic，在启动阶段暴露配置错误
