# 技术架构

> 本文说明工程实现所用的技术栈、模块形态、依赖与构建方式。各组件的职责与流程见 `docs/design/`。

## 一、技术栈

### 后端（Rust）

| 类别 | 选型 | 用途 |
|------|------|------|
| 语言与构建 | Rust 2024 edition、Cargo | 全部后端组件 |
| 异步运行时 | tokio | 任务、定时器、通道、文件异步 IO |
| HTTP 服务 | axum（含 tower / tower-http） | 各组件的对外接口、SSE、认证中间件与 CORS |
| HTTP 客户端 | reqwest | 模型调用、记忆服务调用、MCP 客户端 |
| WebSocket | tokio-tungstenite（经 kai-ws 封装） | 通道服务端与客户端、记忆通知订阅 |
| 序列化 | serde / serde_json | 全部配置与协议数据 |
| 并发容器 | dashmap、arc-swap、parking_lot | 运行态表、整体替换的路由快照、锁 |
| 字符串 | arcstr | 全项目使用的引用计数字符串 |
| 时间 | chrono | 时间字符串、版本文件名、日期切分 |
| 其他 | thiserror、async-trait、uuid、bytes | 错误类型、异步 trait、标识生成、字节缓冲 |
| 图像 | image | Web 通道的缩略图生成 |
| 本地库 | kai-ws、kai-file、kai-index、kai-date、（kai-codegen） | 见下节 |

### 前端（TypeScript）

| 类别 | 选型 | 用途 |
|------|------|------|
| 语言与构建 | TypeScript 6、Vite 8 | 三个前端应用 |
| UI | React 19 | 页面与组件 |
| 事件推送 | @microsoft/fetch-event-source | Web 通道管理员界面订阅 SSE |
| 代码检查 | eslint 9 | 前端代码检查 |

## 二、本地基础库（kai-rs）

kai-rs 是与本仓库同级目录下的独立仓库，通过路径依赖引用，内容按组件划分：

| 库 | 用途与关键机制 |
|----|----------------|
| kai-ws | WebSocket 通信框架：统一的消息信封（序号、类型、状态码、载荷）、JSON 与二进制双通道、心跳处理器、请求-响应配对、握手阶段的请求头过滤接口 |
| kai-file | 文件工具：正序与倒序行读取（带字节位置）、按 key 分组的批量追加（入队即返回落盘承诺，按条数或超时触发写出）、带时间索引的 jsonl 文件索引（分钟级时间范围 + 每千行锚点，懒更新） |
| kai-index | 倒排索引与全文检索：文档→内容→分词的索引流水线、级联树、子串与模型分词两种实现、名称前缀补全 |
| kai-date | 时间字符串工具：按固定格式做日期/年份/分钟前缀切片、跨日区间切段 |
| kai-codegen | 代码生成工具：把 Rust 结构体定义转为 TypeScript 类型，用于前后端契约同源 |

## 三、模块形态

### 独立进程

| 组件 | 说明 |
|------|------|
| kissbot-agent | 装配 Nexus 与 Station，同时对外提供管理接口与 station 的 MCP 端点 |
| kissbot-memory-store | 记忆写入与查询服务 + 独立的通知端点 |
| kissbot-memory-struct-summary | 摘要记忆服务 |
| kissbot-memory-ego | 自我认知服务 |
| kissbot-channel-web | Web 通道后端（通道服务端 + Web 通信工具） |
| kissbot-channel-client-cli | 命令行通道客户端（调试工具） |

### 库

| 组件 | 被谁引用 |
|------|----------|
| kissbot-api | 全部 Rust 组件 |
| kissbot-config | 全部需要读取公共配置的进程 |
| kissbot-security | 所有对外提供服务的组件与会话客户端 |
| kissbot-llm | kissbot-agent、kissbot-memory-struct-summary |
| kissbot-mcp | kissbot-agent |
| kissbot-channel | kissbot-channel-web |
| kissbot-channel-client | kissbot-agent、kissbot-channel-client-cli |
| kissbot-memory | kissbot-memory-store、kissbot-memory-ego、kissbot-memory-struct-summary、kissbot-agent |

### 前端应用

| 应用 | 访问的后端 |
|------|------------|
| kissbot-channel-web-ui | kissbot-channel-web（管理接口 + 事件推送） |
| kissbot-agent-config | 尚未接入后端 |
| kissbot-memory-manage | 尚未接入后端 |

## 四、构建与运行

- 后端：各组件 `cargo build` / `cargo run`；同一工作区内的组件通过路径依赖互相引用。
- 前端：`npm install` 后 `npm run dev` 启动开发服务器，`npm run build` 产出静态文件（tsc 检查 + vite 构建）。
- 后端不托管前端静态文件，两者的部署方式相互独立；Web 通道的前端默认在开发服务器上运行，并在未指定后端地址时通过开发服务器代理访问本机通道。
- 依赖布局：基础库以 `../kai-rs/<组件>` 形式引用，因此要求 kai-rs 与仓库处于同级目录。

## 五、配置与数据目录

| 文件 | 读取方 | 说明 |
|------|--------|------|
| 公共配置文件（路径由环境变量指定，缺省为当前目录的 config.json） | 全部进程 | 只读，按段提供服务的监听地址、数据根目录、密钥与各服务的访问地址 |
| `<agent 数据目录>/nexus.json` | kissbot-agent | agent 的可改配置：通道、模型 provider、agent/角色配置、pipeline 指派、会话配置 |
| `<agent 数据目录>/station.json` | kissbot-agent | station 标识、本级工具分组、直接子站连接信息 |
| `<agent 数据目录>/context/` | kissbot-agent | 会话上下文缓存与版本文件 |
| 记忆根目录 | 三个记忆服务 | 原始记录、自我认知数据、摘要记忆数据（布局见 `memory-directory.md`） |
| Web 通道仓库文件 | kissbot-channel-web | 用户与群组配置、序号 |
| Web 通道的消息目录与附件目录 | kissbot-channel-web | 消息日文件与附件文件 |

## 六、测试

| 层次 | 方式 |
|------|------|
| Rust 单元与集成测试 | 各组件 `cargo test`；agent 内使用共享测试装配（统一初始化单例与 mock 的 MCP 服务端） |
| 组件级 API 集成测试 | `test/` 下的 Playwright 测试套件，配 `test/workspace` 配置与数据目录 |
| 前端单元测试 | vitest |
| 前端端到端测试 | Playwright（启动前端开发服务器后执行） |
| 手动联调 | `script/` 下的启动脚本、配置模板与命令行通道客户端 |

## 七、部署形态

各组件内部只使用 HTTP 与 WS。对外提供服务时由代理服务器（如 nginx）转换为 HTTPS 与 WSS 并管理证书。组件不自行处理 TLS，也不感知外部域名。
