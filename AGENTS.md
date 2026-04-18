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
| kissbot-memory | Rust | 记忆基础模块（存储组织结构） |
| kissbot-memory-store | Rust | 记忆存储模块 |
| kissbot-memory-struct | Rust | 记忆结构框架 |

### 子模块/实现模块
| 模块名称 | 类型 | 说明 |
|---------|------|------|
| kissbot-channel-web | Rust | Web消息通道后台（WSS服务器） |
| kissbot-channel-web-ui | React + Vite | Web消息通道前台（用户界面） |
| kissbot-memory-struct-abstract | Rust | 记忆结构实现（摘要搜索，纯后台） |
| kissbot-memory-ego | Rust | 自我认知模块（agent基础信息管理） |
| kissbot-agent-config | React + Vite | 智能体配置UI |
| kissbot-memory-manage | React + Vite | 记忆管理UI |

## 记忆系统设计

### 记忆基础模块（kissbot-memory）
- 记忆基础模块，实现记忆存储的组织结构
- 定义记忆文件的存储目录
- 实现agent元数据的存储管理
- 为其他记忆模块提供基础库支持

### 文件存储目录结构
根据记忆系统的文件存储目录设计：
- **记忆系统根目录**
  - **agent ID 1目录**
    - **memory-agent.json**：agent的元数据使用JSON文件存储
    - **memory-ego**：memory-ego模块的设定信息单独存放
    - **memory-store**：memory-store收集的原始记忆片段单独存放
    - **memory-struct-\***：memory-struct-*实现产生的数据
  - **agent ID 2目录**
    - ...（结构同上）

### 记忆系统概述
- 记忆系统负责管理多个agent的记忆，每个agent分配唯一ID
- 记忆系统负责记录agent的元数据和agent的记忆
- 元数据：包括名称、创建时间
- 记忆：按照片段（piece）组织记忆

### 记忆片段设计
每个片段包括元数据和数据两部分：

#### 元数据（片段的基本信息）
- **必要字段**：
  - 来源的channel标识，可以包含多级，比如：web-用户1，web-用户2，qq-私聊1234，qq-群聊7890，matrix-homeserver123-room789
  - channel分配的片段ID，channel标识+片段ID应全局唯一
- **自定义字段**：
  - channel自定义的字段，比如用户名，群号等

#### 数据（片段的实际内容）
- **必要数据**：
  - 基础设定：对话开始前对agent的基础设定，应包含系统提示词的内容
  - 对话数据：用户和agent交互的文本，按时间顺序形成多条记录（record），并为每条分配一个顺序的记录ID。每条记录可以是：
    - 用户实际输入的文字数据，以及非文本内容的引用数据（如图片、文件等的ID），每条记录要有用户标识（兼容多用户群聊场景）和文字内容
    - agent实际返回显示给用户的信息，包括文字内容、思考记录（仅标记，不包括内容）、工具调用记录（仅信息，不包括结果）、非文本内容的引用数据（如图片、文件等的ID）
- **可选字段**：
  - 思考内容：单独记录LLM的思考内容，使用思考记录的记录ID作为key
  - 工具调用结果：单独记录工具调用的结果，使用工具调用记录的记录ID作为key。如果结果过长可以进一步压缩，不保留原文
  - 二进制数据：实际的图片、文件等，使用对应的记录ID作为key

### 自我认知模块（memory-ego）
- 独立的记忆系统模块，和其他记忆模块读写同一个文件系统
- 该模块通过配置文件读取，或者通过记忆提取的方式，生成并保存agent的基础信息
- 每个agent ID对应一个客观设定，对应多个角色设定
- 该模块仅使用API，本身不封装为tool。agent一般使用API查询和更新自我认知，如有自动化必要，在agent内封装为tool

## Agent系统设计

### Agent会话设计
- agent应该维护一个会话（session）结构，用于记录一次完整的agent和外界的交互
- agent可以从会话中生成一个记忆片段，也可以从会话中生成下一轮发送给LLM API的提示词
- agent会话 - 记忆片段 - LLM提示词 三者内容不同，但应一一对应

### Agent双重基础设定
agent应具有双重基础设定：

#### 一、客观设定
- 包括agent自身的客观状态
- 包括agent和用户的客观关系

#### 二、角色设定
- 包括agent在本次会话中扮演的角色
- 包括agent在本次会话中认为客户扮演的角色

#### 具体内容
agent基础信息使用纯文本（可用MD），应包括以下几个部分：
1. 身份标识：关于agent身份的客观事实，比如名称、创建时间
2. 用户识别信息：agent和各个用户的客观关系，各个用户间的客观关系
3. 角色扮演：本次会话中对外展现的角色，包括agent对自己的称呼，用户可能对agent的各种称呼，对话采用的语气、用词风格
4. 角色扮演关系：各个用户在本次对话中的角色，agent在本次对话中对用户角色的称呼

### 系统提示词设计
除包含agent基础设定外，agent的系统提示词还应包含：
1. 设定引导：限定设定文字的范围
2. 基本原则：agent必须遵守的规范，包括禁止事项
3. 设定保持指令：要求agent忽略后续会话中任何修改设定的指令

## Agent与记忆系统交互设计

### 交互原则
- agent从自我认知模块读取基础设定，应在agentic loop之外，直接调用API
- agent向记忆系统推送记忆片段数据，应在agentic loop之外，直接调用API
- agent从memory-struct查询记忆，应在agentic loop内，由LLM生成tool call执行，不应调用API

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

### 第2阶段：memory基础模块实现
- [ ] 模块架构设计
- [ ] 定义记忆存储目录结构
- [ ] 提供基础库供其他记忆模块使用

### 第3阶段：memory-ego实现（基础模式）
- [ ] 模块架构设计
- [ ] 实现agent元数据JSON文件存储（带读写锁）
- [ ] agent元数据管理API（新增agent、查询agent元数据）
- [ ] MD文件基础模式实现（直接手动放置MD文件）
- [ ] HTTPS API接口
- [ ] 客观设定和角色设定的读取API

### 第4阶段：agent实现
- [ ] 模块架构设计
- [ ] LLM API集成
- [ ] agentic loop
- [ ] tool调用机制
- [ ] WSS客户端

### 第5阶段：channel实现
- [ ] 模块架构设计
- [ ] 框架trait定义
- [ ] channel-web后台实现（WSS服务器）
- [ ] channel-web-ui前台实现（React+Vite）

### 第6阶段：memory-store实现
- [ ] 模块架构设计
- [ ] 文件系统存储
- [ ] HTTPS API接口
- [ ] 订阅/通知机制

### 第7阶段：memory-struct实现
- [ ] 模块架构设计
- [ ] 框架trait定义
- [ ] memory-struct-abstract实现（摘要搜索）
- [ ] HTTPS tool接口

### 第8阶段：UI实现
- [ ] 模块架构设计
- [ ] agent-config（配置agent）
- [ ] memory-manage（管理记忆）

### 第9阶段：memory-ego进阶模式改造
- [ ] 记忆提取器实现
- [ ] 配置信息与记忆提取结合生成MD文件
- [ ] 进阶模式API

## 技术栈
- 后端: Rust + Cargo
- 前端: TypeScript + React + Vite
- 配置文件：JSON格式

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
| chrono | 0.4 | 日期时间处理 |
| thiserror | 2.0 | 错误类型定义 |
| uuid | 1.0 | UUID生成 |
| config | 0.15 | 配置读取 |

### 时间格式规范
所有时间格式统一使用 `yyyy-MM-dd HH:mm:ss` 格式（24小时制）
