# 整体技术架构文档

## 一、技术栈总览

### 后端
| 技术 | 用途 |
|------|------|
| Rust (2024 edition) | 全部后端模块开发语言 |
| Cargo | 包管理和构建 |
| tokio 1.x | 异步运行时 |
| axum 0.7 | HTTPS 服务器 |
| reqwest 0.12 | HTTPS 客户端 |
| tokio-tungstenite 0.26 | WSS 客户端/服务器 |
| serde / serde_json 1.0 | JSON 序列化 |
| futures 0.3 | 异步任务组合 |
| tower 0.5 | 中间件抽象层（用于认证 Layer） |
| dashmap 6.1 | 并发安全哈希表 |
| chrono 0.4 | 日期时间处理 |
| thiserror 2.0 | 错误类型定义 |
| uuid 1.0 | UUID 生成 |
| config 0.15 | 配置文件读取 |

### 前端
| 技术 | 版本 | 用途 |
|------|------|------|
| TypeScript | 6.0.x | 前端开发语言 |
| React | 19.2.x | UI 框架 |
| Vite | 8.0.x | 构建工具 |
| react-dom | 19.2.x | React DOM 渲染 |
| @vitejs/plugin-react | 6.x | Vite React 插件 |
| eslint | 9.x | 代码检查 |
| @microsoft/fetch-event-source | 8.x | SSE 连接库 |

### 本地库
| 库 | 用途 |
|----|------|
| kai-index（Rust） | 倒排索引模块，用于全文搜索 |
| kai-ws（Rust） | WebSocket 通信框架，支持 JSON/二进制消息、心跳、请求头过滤 |
| kai-file（Rust） | 文件 I/O 工具库（反向行读取器等） |
| kai-codegen（Rust） | Rust struct 转 TypeScript 类型定义代码生成工具 |

## 二、通信协议

使用 HTTPS、WSS 和文件系统共享三种通信方式，详细协议说明见 [communication.md](communication.md)。

### WSS（WebSocket Secure）
用于实时双向通信场景，共有两组 WSS 连接：

**nexus ↔ channel**：nexus 作为 WSS 客户端连接 channel 的 WSS 服务器。每个 nexus 对应唯一连接。

**memory-store ↔ memory-struct**：memory-struct 作为 WSS 客户端连接 memory-store。memory-store 有新数据时广播通知。

### HTTPS
用于请求-响应模式的通信：
- nexus → memory-store：推送记忆记录
- channel → memory-store：推送消息记录
- nexus → memory-ego：读取自我认知设定
- nexus ↔ station：tool call 请求/响应（同进程时通过内部调用）
- nexus → memory-struct：内置记忆查询 tool 调用（不记入记忆）
- 前端 UI → 后端：配置管理、记忆查看管理（HTTP + SSE）

### 文件系统共享
- memory-store 和 memory-struct-* 共同读写同一文件系统目录

## 三、通信通用规范

### API 设计原则
- 路径无参数：HTTP API 路径仅用于路由到具体处理函数，不含动态参数
- 参数全 JSON：所有输入参数在请求体中传递
- 统一响应格式：所有 API 响应使用 ApiResponse 结构（success + data + error）
- 认证方式：所有请求（HTTPS）必须携带认证 header，WSS 握手阶段同样通过 header 认证
- 详细认证机制见 [authentication.md](authentication.md)

### 数据结构一致性
通过泛型 trait 确保并发类型和 API 类型一致：
```
XxxGeneric（泛型定义）→ XxxKind（trait 约束）
→ SyncXxx（内部类型约束，用 Arc/DashMap）
→ LocalXxx（API 类型约束，用 String/HashMap）
```

### 时间格式
- 时间格式：`yyyy-MM-dd HH:mm:ss`（24 小时制）
- 日期格式：`yyyy-MM-dd`
- 年格式：`yyyy`

## 四、数据存储

### 记忆文件
- 使用 JSON Lines 格式，便于追加和流式读取
- 按 agent ID、年份和记忆模式组织目录结构
- 四种记录类型分文件存储：通道文本、思考内容、工具调用、工具结果
- 详细文件结构见 [memory-storage.md](memory-storage.md)

### 元数据和配置
- 使用 JSON 格式存储
- 配置文件、元数据文件、角色设定文件等均使用 JSON

### 附件
- 由各 channel 实现模块自行管理（文件系统等）
- 基础模块仅要求提供 get_attachment_metadata / get_attachment_data 接口

## 五、模块类型划分

### 独立进程（Rust app）
| 模块 | 启动方式 | 说明 |
|------|----------|------|
| kissbot-agent | cargo run | Agent 智能体，启动时选择开启 nexus、station 或同时开启两者 |
| kissbot-memory-store | cargo run | 记忆存储模块 |
| kissbot-memory-ego | cargo run | 自我认知模块 |
| kissbot-channel-web | cargo run | Web 消息通道 |
| kissbot-memory-struct-abstract | cargo run | 记忆结构（摘要索引，骨架阶段） |

### 库模块（Rust lib）
| 模块 | 被谁使用 |
|------|----------|
| kissbot-api | kissbot-channel, kissbot-memory, kissbot-security, kissbot-memory-ego, kissbot-memory-store, 及所有其他模块 |
| kissbot-security | 计划接入：kissbot-memory-store、kissbot-memory-ego、kissbot-agent、kissbot-channel-web 等 |
| kissbot-channel | kissbot-channel-web 等实现模块，kissbot-agent |
| kissbot-memory | kissbot-memory-store, kissbot-memory-ego, kissbot-memory-struct-*, kissbot-memory-struct |
| kissbot-memory-struct | kissbot-memory-struct-abstract 等记忆结构实现模块 |
| kai-ws | kissbot-api, kissbot-channel, kissbot-security |
| kai-file | kissbot-memory, kissbot-memory-store |
| kai-index | kissbot-memory-ego |

### 前端（TypeScript + React + Vite）
| 模块 | 访问的后端 |
|------|------------|
| kissbot-agent-config | kissbot-agent（nexus/station 配置） |
| kissbot-memory-manage | memory-store, memory-struct, memory-ego |
| kissbot-channel-web-ui | channel-web |
