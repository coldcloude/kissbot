# kissbot-security 组件内功能实现顺序

## 实现状态：🔴 未开始

### 第1阶段：项目初始化和基础结构 🔴
- [ ] 配置 Cargo.toml，添加依赖（kissbot-api、axum、kai-ws）
- [ ] rust 项目骨架（lib.rs、模块声明）
- [ ] Cargo workspace 注册（如果 workspace 已存在则注册子包）

### 第2阶段：auth_types 模块 🔴
- [ ] 定义 `AuthError` 枚举（MissingKey / InvalidKey）
- [ ] 定义 HTTP header 常量 `pub const HEADER_API_KEY: &str = "X-Api-Ke圞"`
- [ ] 实现 `extract_api_key` 辅助函数：从 HTTP header map 中提取 key 值
- [ ] 导出所有公开类型

### 第3阶段：validator 模块 🔴
- [ ] 定义 `ApiKeyValidator` trait（`fn validate(&self, key: &str) -> Result<(), AuthError>`）
- [ ] 实现 `SimpleApiKeyValidator`（持有预配置 key，字符串比对）
- [ ] 单元测试

### 第4阶段：axum_middleware 模块 🔴
- [ ] 实现 `AuthLayer`（axum tower Layer），拦截所有 HTTP 请求
- [ ] 从请求 header 提取 `X-Api-Key`，调用 `ApiKeyValidator` 校验
- [ ] 认证失败返回 HTTP 401 + 标准 `ApiResponse` 格式
- [ ] 支持白名单路径配置
- [ ] 集成测试

### 第5阶段：kai-ws 集成 🔴
- [ ] 在 `kai-ws` 中增加 `WssUpgradeFilter` trait
- [ ] 修改 `kai-ws` 的 `ws_handle_connection` 函数签名，增加可选的 filter 回调参数
- [ ] filter 回调在 `accept_async(stream)` 之前调用，获取 HTTP Request headers 进行检查
- [ ] 认证失败返回 HTTP 401 响应并关闭连接
- [ ] kissbot-security 提供 `create_api_key_wss_filter` 工厂函数
- [ ] 集成测试

### 第6阶段：各进程接入 ✅
- [ ] kissbot-memory-store 接入 HTTPS middleware
- [ ] kissbot-memory-ego 接入 HTTPS middleware
- [ ] 其他进程（channel-web、agent 等）在其实装时同时接入

### 第7阶段：完善 🔴
- [ ] Cargo doc 文档注释
- [ ] 集成测试覆盖
- [ ] 配置文件示例（api_key 字段说明）
