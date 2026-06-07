# kissbot-security 组件内功能实现顺序

## 实现状态：🟡 部分完成（核心模块已完成，测试和文档待完善）

### 第1阶段：项目初始化和基础结构 ✅
- [x] 配置 Cargo.toml，添加依赖（kissbot-api、axum、kai-ws）
- [x] rust 项目骨架（lib.rs、模块声明）

### 第2阶段：auth_types 模块 ✅
- [x] 定义 `Error` 枚举（MissingKey / InvalidKey），独立到 error.rs
- [x] 定义 HTTP header 常量 `pub const HEADER_API_KEY: &str = "X-Api-Key"`
- [x] 实现 `extract_api_key` 辅助函数：从 HTTP header map 中提取 key 值
- [x] 导出所有公开类型

### 第3阶段：validator 模块 ✅
- [x] 定义 `ApiKeyValidator` trait（`fn validate(&self, key: &str) -> Result<(), Error>`）
- [x] 实现 `SimpleApiKeyValidator`（持有预配置 key，字符串比对）

### 第4阶段：axum_middleware 模块 ✅
- [x] 实现 `auth_middleware` 函数（基于 axum::middleware::from_fn）
- [x] 从请求 header 提取 `X-Api-Key`，调用 `ApiKeyValidator` 校验
- [x] 认证失败返回 HTTP 401 + 标准 JSON 格式

### 第5阶段：kai-ws 集成 ✅
- [x] kai-ws 新增 `WsHeaderFilter` trait
- [x] kai-ws 新增 `ws_handle_connection_with_filter` 函数
- [x] kissbot-security 实现 `ApiKeyWsFilter`
- [x] 认证失败返回 HTTP 401 响应并关闭连接

### 第6阶段：各进程接入 ✅
- [x] kissbot-memory-store 接入 HTTPS middleware
- [x] kissbot-memory-ego 接入 HTTPS middleware
- [x] kissbot-channel 接入 WsHeaderFilter

### 第7阶段：完善 🔴
- [ ] Cargo doc 文档注释
- [ ] 集成测试覆盖
- [ ] 配置文件示例（api_key 字段说明）
