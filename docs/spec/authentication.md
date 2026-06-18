# 认证方式

## 认证机制

使用 HTTP header `X-Api-Key` 进行认证。所有 HTTPS 请求和 WSS 握手阶段均携带此 header。

## 设计原则

- **只做认证，不做授权**：仅验证请求方知道 API key，不过问请求方应该做什么
- **单一 key**：使用预配置的统一 API key，不支持多用户和动态 key 管理
- **认证方式统一**：HTTPS 和 WSS 共用同一套校验逻辑

## HTTPS 认证

基于 tower Layer 的 axum 中间件，挂载到 Router 上自动拦截请求：
- 请求到达 → 校验器检查 `X-Api-Key` header
- 缺失 key 或 key 不匹配 → 返回 HTTP 401 + ApiResponse `{ success: false, error: "unauthorized" }`
- 认证通过 → 继续处理请求

## WSS 认证

WSS 握手阶段的 HTTP Upgrade 请求与 HTTPS 请求使用同一套认证机制：
- 在握手阶段检查 Upgrade 请求的 `X-Api-Key` header
- 认证通过后才建立 WebSocket 连接
- 通过 WSS 框架的 filter 回调机制在握手阶段完成校验
