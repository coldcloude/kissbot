# 整体技术架构文档

## 一、技术栈总览

### 后端
| 技术 | 用途 |
|------|------|
| Rust (2024 edition) | 全部后端模块开发语言 |
| Cargo | 包管理和构建 |
| tokio 1.x | 异步运行时 |
| axum 0.7 | HTTP 服务器（对外 HTTPS 由代理层承担） |
| reqwest 0.12 | HTTP 客户端 |
| tokio-tungstenite 0.26 | WS 客户端/服务器（kai-ws 实现，对外 WSS 由代理层承担） |
| serde / serde_json 1.0 | JSON 序列化（公共配置亦经 serde_json 读取） |
| futures / futures-util 0.3 | 异步任务组合 |
| tower 0.5 | 中间件抽象层（用于认证 Layer） |
| dashmap 6.1 | 并发安全哈希表 |
| chrono 0.4 | 日期时间处理 |
| thiserror 2.0 | 错误类型定义 |
| uuid 1.0 | UUID 生成 |

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

## 二、模块类型划分

### 独立进程（Rust app）
| 模块 | 启动方式 |
|------|----------|
| kissbot-agent | cargo run |
| kissbot-memory-store | cargo run |
| kissbot-memory-ego | cargo run |
| kissbot-channel-web | cargo run |
| kissbot-channel-client-cli | cargo run（命令行通道客户端） |
| kissbot-memory-struct-abstract | cargo run |

### 库模块（Rust lib）
| 模块 | 被谁使用 |
|------|----------|
| kissbot-api | 所有其他模块 |
| kissbot-config | kissbot-agent、kissbot-api、kissbot-memory-*、kissbot-channel-web、kissbot-security（公共配置读取） |
| kissbot-security | kissbot-agent、kissbot-memory-store、kissbot-memory-ego、kissbot-channel-*、kissbot-memory-struct-* |
| kissbot-mcp | kissbot-agent（station 的 MCP 服务端与客户端） |
| kissbot-channel | kissbot-channel-* |
| kissbot-channel-client | kissbot-agent、kissbot-channel-client-cli（通道客户端与 Terminal 回调抽象） |
| kissbot-memory | kissbot-memory-store、kissbot-memory-ego、kissbot-memory-struct-* |
| kissbot-memory-struct | kissbot-memory-struct-* |
| kai-ws | kissbot-api、kissbot-channel、kissbot-security、kissbot-agent、kissbot-channel-web、kissbot-channel-client |
| kai-file | kissbot-memory、kissbot-memory-store、kissbot-agent |
| kai-index | kissbot-memory-ego |
| kai-date | kai-file、kissbot-memory、kissbot-channel-web |
| kai-codegen | 构建期（Rust struct → TypeScript 类型） |

### 前端（TypeScript + React + Vite）
| 模块 | 访问的后端 |
|------|------------|
| kissbot-agent-config | kissbot-agent |
| kissbot-memory-manage | kissbot-memory-store、kissbot-memory-struct-*、kissbot-memory-ego |
| kissbot-channel-web-ui | kissbot-channel-web |
