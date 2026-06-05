# kissbot-security 组件设计

## 概述
安全认证模块，为所有模块的 HTTP 通信提供统一的 API key 认证能力。作为独立的 Rust lib crate，定义了认证相关的数据契约（trait + 类型）和具体的校验工具函数。

## 设计原则
- **只做认证，不做授权**：仅验证请求方知道 API key，不过问请求方应该做什么
- **单一 key**：使用预配置的统一 API key，不支持多用户和动态 key 管理
- **契约与实现分离**：trait 定义校验接口，具体校验逻辑可灵活替换
- **协议统一**：HTTPS 和 WSS 共用同一套认证机制，WSS 在握手阶段通过 `kai-ws` 的 filter 回调检查 header

## 内部模块

### 1. auth_types 模块
认证核心数据类型定义，与 `kissbot-api` 的 `common` 模块同级：

- `AuthError` 枚举：
  - `MissingKey` — 请求未携带 API key header
  - `InvalidKey` — API key 不匹配
- HTTP header 常量 `X-Api-Key`
- `extract_api_key` 辅助函数：从 HTTP header map 中提取 key 值

### 2. validator 模块
认证校验逻辑：

- `ApiKeyValidator` trait：`fn validate(&self, key: &str) -> Result<(), AuthError>`
- `SimpleApiKeyValidator` 具体实现：持有预配置 key，与请求 header 中的 key 做字符串比对
- 可通过配置文件或环境变量初始化

### 3. wss_filter 模块
WSS 握手阶段的 filter 回调工具：

- `WssUpgradeFilter` trait：供 `kai-ws` 在 WSS Upgrade 握手后、WebSocket 流建立前调用的回调接口
- 回调接收 HTTP Request headers，调用 `ApiKeyValidator` 校验
- 认证失败则关闭连接并返回 HTTP 401 响应

### 4. axum_middleware 模块
HTTPS 请求认证的 axum middleware 工具：

- 提供 axum 的 `Layer`（中间件），自动拦截所有 HTTP 请求
- 从请求 header 中提取 `X-Api-Key`，调用 `ApiKeyValidator` 校验
- 认证失败返回 HTTP 401 + 标准 `ApiResponse` 格式错误响应
- 认证通过则继续处理请求
- 开放白名单路径配置（如健康检查接口可免认证）

## 依赖关系

```
kissbot-security
  ├── kissbot-api（使用 ApiResponse 等现有类型）
  ├── axum（提供 middleware layer）
  └── kai-ws（使用 WssUpgradeFilter trait）
```

`kissbot-api` 不依赖 `kissbot-security`，依赖方向保持单向。

## 使用方式

各独立进程（memory-store、memory-ego、agent、channel-web 等）：

### HTTPS 服务器接入
```rust
// 1. 创建 validator 实例
let validator = SimpleApiKeyValidator::new(config.api_key);

// 2. 构建 axum router 时注入 middleware
let app = Router::new()
    .route("/api/...", post(handler))
    .layer(AuthLayer::new(validator));
```

### WSS 服务器接入（通过 kai-ws）
```rust
// 1. 创建 validator 实例
let validator = SimpleApiKeyValidator::new(config.api_key);

// 2. ws_handle_connection 传入 filter 回调
ws_handle_connection(stream, queue_capacity, processor_context, &initializer, 
    |request| validator.validate(extract_api_key(request)))
    .await?;
```

## 对外接口
以库形式提供数据类型、trait、middleware 和工具函数，供所有需要认证能力的模块依赖。
