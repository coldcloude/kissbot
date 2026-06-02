# 整体技术架构文档

## 一、技术栈总览

### 后端
| 技术 | 用途 |
|------|------|
| Rust (2024 edition) | 全部后端模块开发语言 |
| Cargo | 包管理和构建 |
| tokio 1.x | 异步运行时 |
| axum 0.8 | HTTPS 服务器 |
| reqwest 0.13 | HTTPS 客户端 |
| tokio-tungstenite 0.29 | WSS 客户端/服务器 |
| serde / serde_json 1.0 | JSON 序列化 |
| futures 0.3 | 异步任务组合 |
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

### 本地库
| 库 | 用途 |
|----|------|
| kai-index（Rust） | 倒排索引模块，用于全文搜索 |

## 二、通信协议

### WSS（WebSocket Secure）
用于实时双向通信场景：
- **agent ↔ channel**：agent 作为 WSS 客户端连接 channel 的 WSS 服务器。每个 agent 对应唯一连接，消息体包含 MSG 类型和 JSON data。
  - 消息类型：bind / bind_ack、outgoing_message（agent → channel）、incoming_message（channel → agent）、get_channels / channels、group_change、attachment_download / attachment_data、ping / pong
- **memory-store ↔ memory-struct**：memory-struct 作为 WSS 客户端连接 memory-store。memory-store 有新数据时通过 WSS 广播通知。

### HTTPS
用于请求-响应模式的通信：
- **agent → memory-store**：推送记忆记录
- **channel → memory-store**：推送消息记录
- **agent → memory-ego**：读取自我认知设定
- **agent → memory-struct**：查询记忆（agentic loop 内的 tool call）
- **前端 UI → 后端**：配置管理、记忆查看管理
- **所有 API 路径仅用于路由，参数放在 JSON 请求体中**

### 文件系统共享
- memory-store 和 memory-struct-* 共同读写同一文件系统目录
- memory-store 写入记忆文件
- memory-struct 读取记忆文件构建索引

## 三、通信通用规范

### API 设计原则
- 路径无参数：HTTP API 路径仅用于路由到具体处理函数，不含动态参数
- 参数全 JSON：所有输入参数在请求体中传递
- 统一响应格式：所有 API 响应使用 ApiResponse 结构（success + data + error）

### 数据结构一致性
通过泛型 trait 确保并发类型和 API 类型一致：
```
XxxGeneric（泛型定义）→ XxxKind（trait 约束）
→ SyncXxx（内部类型约束，用 Arc/DashMap）
→ LocalXxx（API 类型约束，用 String/HashMap）
```

### 安全要求
- 所有通信使用 HTTPS 或 WSS 协议
- 所有客户端支持自定义可信证书文件配置
- 支持自签名证书

### 时间格式
- 时间格式：`yyyy-MM-dd HH:mm:ss`（24 小时制）
- 日期格式：`yyyy-MM-dd`
- 年格式：`yyyy`

## 四、数据存储

### 记忆文件
- 使用 JSON Lines 格式，便于追加和流式读取
- 按 agent ID → 年 → 角色 → 日期组织文件目录结构
- 三种记录类型分文件存储：channel 文本、思考内容、工具调用

### 元数据和配置
- 使用 JSON 格式存储
- metadata.json：agent 元数据
- user-recognition.json：用户识别信息
- role-play-{role-id}.json：角色设定

### 附件
- 由各 channel 实现模块自行管理（文件系统等）
- 基础模块仅要求提供 get_attachment_metadata / get_attachment_data 接口

## 五、模块类型划分

### 独立进程（Rust app）
| 模块 | 启动方式 | 依赖的基础库 |
|------|----------|--------------|
| kissbot-agent | cargo run | kissbot-api, kissbot-channel |
| kissbot-memory-store | cargo run | kissbot-memory, kissbot-api |
| kissbot-memory-ego | cargo run | kissbot-memory, kissbot-api, kai-index |
| kissbot-channel-web | cargo run | kissbot-channel, kissbot-api |
| kissbot-memory-struct-abstract | cargo run | kissbot-memory-struct, kissbot-api |

### 库模块（Rust lib）
| 模块 | 被谁使用 |
|------|----------|
| kissbot-api | 所有其他模块 |
| kissbot-channel | kissbot-channel-web 等实现模块 |
| kissbot-memory | memory-store, memory-ego, memory-struct-* |
| kissbot-memory-struct | memory-struct-abstract 等实现 |
| kissbot-project | agent（工程模式） |

### 前端（TypeScript + React + Vite）
| 模块 | 访问的后端 |
|------|------------|
| kissbot-agent-config | agent |
| kissbot-memory-manage | memory-store, memory-struct, memory-ego |
| kissbot-channel-web-ui | channel-web |
