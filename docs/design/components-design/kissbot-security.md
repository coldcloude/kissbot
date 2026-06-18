# kissbot-security 组件设计

## 概述
安全认证模块，为所有模块的 HTTP 通信提供统一的 API key 认证能力。作为独立的 Rust lib crate，定义了认证相关的数据契约和具体的校验接入工具。

## 设计原则
- **只做认证，不做授权**：仅验证请求方知道 API key，不过问请求方应该做什么
- **单一 key**：使用预配置的统一 API key，不支持多用户和动态 key 管理
- **契约与实现分离**：接口定义与具体校验逻辑分离
- **认证方式统一**：HTTPS 和 WSS 共用同一套校验逻辑，请求携带 `X-Api-Key` HTTP header

## 内部模块

### 1. 认证类型定义
认证相关的数据类型：
- 认证失败错误类型：区分缺失 key 和 key 不匹配
- HTTP header 名称常量 `X-Api-Key`
- 从 header map 中提取 key 值的工具函数

### 2. 校验器
认证校验的实现：
- 校验接口定义：接收 API key 字符串，返回校验结果
- 简单字符串比对实现：持有预配置 key，与请求携带的 key 比对
- 使用者也可自定义校验逻辑

### 3. WSS 接入
WSS 握手阶段的 filter 回调工具：
- 实现 kai-ws 的 `WsHeaderFilter` 接口，在 WebSocket 握手阶段校验
- 从 Upgrade 请求的 header 中提取 `X-Api-Key`，调用校验器验证
- 认证失败关闭连接

### 4. HTTPS 接入
HTTPS 请求认证的 axum 中间件：
- 基于 tower Layer 的标准 axum 中间件，挂载到 Router 上自动拦截请求
- 调用校验器验证 `X-Api-Key` header
- 认证失败返回 HTTP 401 + 标准 JSON 错误响应，认证通过继续处理请求

## 依赖关系

```
kissbot-security
  ├── kissbot-api（使用 ApiResponse 等现有类型）
  ├── axum + tower（提供中间件机制）
  └── kai-ws（使用 WsHeaderFilter 接口）
```

`kissbot-api` 不依赖 `kissbot-security`，依赖方向保持单向。

## 使用方式

各独立进程在启动时创建校验器实例，挂载到 HTTPS 服务器或 WSS 服务器即可：
- **HTTPS 服务器**：创建 `SimpleApiKeyValidator`，通过 `AuthLayer` 挂载到 axum Router
- **WSS 服务器**：创建 `SimpleApiKeyValidator`，通过 `ApiKeyWsFilter` 传入 `ws_handle_connection_with_filter`

## 对外接口
以库形式提供数据类型、校验接口、HTTPS 中间件和 WSS filter 工具，供所有需要认证能力的模块依赖。
