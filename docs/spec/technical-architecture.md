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

## 二、模块类型划分

### 独立进程（Rust app）
| 模块 |
|------|
| kissbot-agent |
| kissbot-memory-store |
| kissbot-memory-ego |
| kissbot-channel-web |
| kissbot-memory-struct-abstract |

### 库模块（Rust lib）
| 模块 |
|------|
| kissbot-api |
| kissbot-security |
| kissbot-channel |
| kissbot-memory |
| kissbot-memory-struct |
| kai-ws |
| kai-file |
| kai-index |

### 前端（TypeScript + React + Vite）
| 模块 |
|------|
| kissbot-agent-config |
| kissbot-memory-manage |
| kissbot-channel-web-ui |
