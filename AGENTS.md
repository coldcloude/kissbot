# 项目基本信息

## 项目名称
Keep It Simple Stupid BOT - kissbot

## 开发环境
- 开发操作系统为Windows系统，调试后台产物应为exe文件
- 开发命令行环境为PowerShell，执行命令时使用PowerShell的语法

## 开发框架
- 后台rust+cargo
- 前台typescript+react+vite

## 项目决策文档
- 项目需求和开发者决策在 开发者决策.txt 中，不要修改这个文件
- 本文件（AGENTS.md）记录了项目整体的开发计划，其中 项目开发计划 章节的内容根据 开发者决策.txt 中的决策进行记录并扩充设计，随着开发过程不断变更，体现项目架构的最新变化
- 项目每个目录代表一个模块，每个模块目录下都使用一个独立的 AGENTS.md 记录模块的架构设计和实现决策，随着开发过程不断变更，体现模块的最新变化

# 项目开发计划

## 项目概述
实现一个分布式的智能机器人框架，采用多进程架构，模块间通过WSS、HTTPS或文件进行通信。

## 模块架构

### 顶层模块
| 模块名称 | 类型 | 说明 |
|---------|------|------|
| kissbot-agent | Rust | 智能体核心，处理LLM调用和agentic loop |
| kissbot-channel | Rust | 消息通道框架 |
| kissbot-memory-store | Rust | 记忆存储模块 |
| kissbot-memory-struct | Rust | 记忆结构框架 |

### 子模块/实现模块
| 模块名称 | 类型 | 说明 |
|---------|------|------|
| kissbot-channel-web | Rust | Web消息通道后台（WSS服务器） |
| kissbot-channel-web-ui | React + Vite | Web消息通道前台（用户界面） |
| kissbot-memory-struct-abstract | Rust | 记忆结构实现（摘要搜索，纯后台） |
| kissbot-agent-config | React + Vite | 智能体配置UI |
| kissbot-memory-manage | React + Vite | 记忆管理UI |

## 通信方式
- agent ↔ channel: WSS (agent作为客户端)
- agent ↔ memory-store: HTTPS (agent作为客户端)
- memory-store ↔ memory-struct: 文件系统 + HTTPS通知
- agent ↔ memory-struct: HTTPS (通过tool)
- UI ↔ 后端: HTTPS

## 实现步骤

### 第1阶段：核心模块初始化
- [x] 初始化所有Rust项目
- [x] 初始化所有React+Vite项目
- [x] 建立项目目录结构
- [ ] 各模块架构设计

### 第2阶段：agent实现
- [ ] LLM API集成
- [ ] agentic loop
- [ ] tool调用机制
- [ ] WSS客户端

### 第3阶段：channel实现
- [ ] 框架trait定义
- [ ] channel-web后台实现（WSS服务器）
- [ ] channel-web-ui前台实现（React+Vite）

### 第4阶段：memory-store实现
- [ ] 文件系统存储
- [ ]  HTTPS API接口
- [ ] 订阅/通知机制

### 第5阶段：memory-struct实现
- [ ] 框架trait定义
- [ ] memory-struct-abstract实现（摘要搜索）
- [ ] HTTPS tool接口

### 第6阶段：UI实现
- [ ] agent-config（配置agent）
- [ ] memory-manage（管理记忆）

## 技术栈
- 后端: Rust + Cargo
- 前端: TypeScript + React + Vite
- 配置文件：JSON格式
- 数据库：SQLite

## 安全要求
- 所有后台服务（agent、channel、memory-store、memory-struct及其实现）都应具备配置可信证书文件的机制
- 支持使用自签名证书密钥进行WSS、HTTPS通信

## 依赖版本

### 前端依赖（所有UI项目通用）
| 依赖包 | 版本 |
|--------|------|
| react | 19.2.0 |
| react-dom | 19.2.0 |
| @types/react | 19.2.0 |
| @types/react-dom | 19.2.0 |
| typescript | 6.0.2 |
| vite | 8.0.8 |
| @vitejs/plugin-react | 6.0.1 |
| eslint | 9.39.4 |
| @typescript-eslint/eslint-plugin | 8.58.2 |
| @typescript-eslint/parser | 8.58.2 |
| eslint-plugin-react-hooks | 7.0.1 |
| eslint-plugin-react-refresh | 0.5.2 |

### 后端配置（所有后端项目通用）
| 配置项 | 版本 |
|--------|------|
| Rust edition | 2024 |

### 后端依赖（所有后端项目通用）
| 依赖包 | 版本 | 用途 |
|--------|------|------|
| tokio | 1.51 | 异步运行时 |
| tokio-tungstenite | 0.29 | WSS客户端 |
| reqwest | 0.13 | HTTPS客户端 |
| axum | 0.8 | HTTPS服务器 |
| serde | 1.0 | JSON序列化 |
| serde_json | 1.0 | JSON序列化 |
