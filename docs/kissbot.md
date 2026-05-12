# kissbot 整体设计

## 项目概述
实现一个具有永久记忆的分布式的智能机器人框架，采用多进程架构，模块间通过WSS、HTTPS或文件进行通信。

## 模块架构

### 顶层模块
- [kissbot-api](./kissbot-api.md) - API 定义模块（Rust lib）
- [kissbot-agent](./kissbot-agent.md) - 智能体核心，处理 LLM 调用和 agentic loop，支持三种模式（Rust app）
  - [问答模式](./kissbot-agent-chat.md) - 问答的过程就是全部内容
  - [工程模式](./kissbot-agent-project.md) - 做一件事的持续过程
  - [自主模式](./kissbot-agent-autonomous.md) - 持续收集信息并与他人交换信息
- [kissbot-channel](./kissbot-channel.md) - 消息通道框架，连接外部系统，管理非文本内容的本地存储。（Rust lib）
- [kissbot-memory](./kissbot-memory.md) - 记忆基础模块（记忆目录结构定义）（Rust lib）
- [kissbot-project](./kissbot-project.md) - 工程模块，管理工程职位和提供tool（Rust lib）

### 消息通道子模块
- [kissbot-channel-web](./kissbot-channel-web.md) - Web消息通道后台（WSS服务器）（Rust app）
- [kissbot-channel-web-ui](./kissbot-channel-web-ui.md) - Web消息通道前台（用户界面）（React + Vite）

### 记忆子模块
- [kissbot-memory-store](./kissbot-memory-store.md) - 记忆存储模块，收集和存储原始记忆（Rust app）
- [kissbot-memory-ego](./kissbot-memory-ego.md) - 自我认知模块（Rust app）
- [kissbot-memory-struct](./kissbot-memory-struct.md) - 记忆结构框架，提供记忆查询功能（Rust lib）

#### 记忆结构实现子模块
- [kissbot-memory-struct-recent](./kissbot-memory-struct-recent.md) - 记忆结构实现（最近记忆）（Rust app）
- [kissbot-memory-struct-abstract](./kissbot-memory-struct-abstract.md) - 记忆结构实现（摘要、搜索）（Rust app）

### 界面子模块
- [kissbot-agent-config](./kissbot-agent-config.md) - 智能体配置UI（React + Vite）
- [kissbot-memory-manage](./kissbot-memory-manage.md) - 记忆管理UI（React + Vite）

## 模块关系

### 模块间联系
1. **channel**：连接外部系统，如web界面、QQ、Matrix等，向agent输入消息，并接收agent的输出消息。
2. **agent**：负责将消息加工为LLM可用的消息，通过agentic loop调用LLM执行操作，返回消息。支持三种模式：问答模式、工程模式、自主模式。
3. **project**：工程模块（Rust库），管理工程中的职位配置，为工程模式的agent提供tool（文件操作、命令执行、扩展skill）。
4. **memory-store**：负责收集agent接收和产生的所有原始消息，作为一切agent活动的记录。结构化存储三种类型的记录：channel文本、思考内容、工具调用。
5. **memory-struct-\***：负责将memory-store存储的记忆构造成特定的索引结构，可以查询记忆，并对外提供记忆搜索工具。
6. **agent-config**：提供WebUI界面负责配置agent使用的LLM API、tool、skill，以及channel、memory、memory-struct的地址。
7. **memory-manage**：提供WebUI界面负责查看和管理memory-store存储的文件，以及配置memory-store向memory-struct的消息推送。

### 通信方式
1. **agent ↔ channel**：WSS（agent作为客户端）
2. **agent ↔ memory-store**：HTTPS（agent作为客户端）
3. **channel ↔ memory-store**：HTTPS（channel作为客户端）
4. **agent ↔ memory-ego**：HTTPS（agent作为客户端，仅自主模式）
5. **agent ↔ memory-struct-\***：HTTPS（通过tool调用）
6. **memory-store ↔ memory-struct-\***：共同读写文件系统，memory-store有新数据时通过WSS通知各已连接的memory-struct
7. **各WebUI前端 ↔ 后端**：HTTPS
8. **agent-config ↔ agent后端**：HTTPS
9. **memory-manage ↔ memory-store、memory-struct-*、memory-ego后端**：HTTPS

### 开发重心

**开发重心为：自主模式agent、记忆系统**

## 技术选型

### 技术栈
- 后端: Rust（2024 edition） + Cargo
- 前端: TypeScript + React + Vite
- 配置文件：JSON格式

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

### 后端依赖（所有后端项目通用）
| 依赖包 | 版本 | 用途 |
|--------|------|------|
| tokio | 1.51 | 异步运行时 |
| tokio-tungstenite | 0.29 | WSS客户端/服务器 |
| futures | 0.3 | 异步任务 |
| dashmap | 6.1 | 并发安全的哈希表 |
| reqwest | 0.13 | HTTPS客户端 |
| axum | 0.8 | HTTPS服务器 |
| serde | 1.0 | JSON序列化 |
| serde_json | 1.0 | JSON序列化 |
| chrono | 0.4 | 日期时间处理 |
| thiserror | 2.0 | 错误类型定义 |
| uuid | 1.0 | UUID生成 |
| config | 0.15 | 配置读取 |

### 时间格式规范
所有时间格式统一使用 `yyyy-MM-dd HH:mm:ss` 格式（24小时制），日期为其前缀 `yyyy-MM-dd` 格式，年为其前缀 `yyyy` 格式。

## 安全要求
- 所有通信都应使用HTTPS或WSS协议，避免明文传输敏感信息
- 所有HTTPS、WSS客户端都应具备自定义可信证书文件的配置
- 支持使用自签名证书密钥进行WSS、HTTPS通信

## 实现步骤

### 第1阶段：核心模块初始化
- [x] 初始化所有Rust项目
- [x] 初始化所有React+Vite项目
- [x] 建立项目目录结构

### 第2阶段：memory基础模块实现
- [x] 模块架构设计
- [x] 定义记忆存储目录结构
- [x] 提供基础库供其他记忆模块使用

### 第3阶段：memory-store实现
- [ ] 模块架构设计
- [ ] 三种记录类型的存储（JSON Lines格式）
- [ ] HTTPS API接口（推送、查询）
- [ ] 订阅/通知机制

### 第4阶段：channel实现
- [ ] 模块架构设计
- [ ] 框架trait定义
- [ ] 附件存储管理
- [ ] WSS服务器实现
- [ ] HTTPS API接口
- [ ] channel-web后台实现
- [ ] channel-web-ui前台实现

### 第5阶段：agent实现（自主模式）
- [ ] 模块架构设计
- [ ] 自主模式支持
- [ ] LLM API集成
- [ ] WSS客户端与channel集成
- [ ] 与memory-store的集成

### 第6阶段：memory-ego实现（基础模式）
- [x] 模块架构设计
- [x] 实现agent元数据JSON文件存储（带读写锁）
- [x] agent元数据管理API（新增agent、查询agent元数据）
- [x] HTTPS API接口
- [x] 用户识别信息、角色设定的JSON管理模块
- [x] 用户识别信息、角色设定的查询API
- [ ] 在AgentMetadata中增加forbidden_items和autonomous_goals字段
- [ ] 在role-play数据结构中增加autonomous_goals字段
- [ ] 实现更新禁止事项和自主运行目标的功能和API
- [ ] 完善metadata.json和role-play-{role-id}.json的读写

### 第7阶段：memory-struct实现
- [ ] 模块架构设计
- [ ] 框架trait定义
- [ ] memory-struct-recent实现（最近记忆）
- [ ] 最近记忆HTTPS API和tool

### 第8阶段：agent实现（自主模式step2）
- [ ] 与memory-ego的集成
- [ ] 与memory-struct-recent实现的集成
- [ ] 上下文重置功能
- [ ] 自主运行目标触发机制

### 第9阶段：agent摘要记忆
- [ ] memory-struct-abstract实现（摘要搜索）
- [ ] 摘要搜索记忆HTTPS API和tool
- [ ] agent tool call实现
- [ ] agentic loop实现

### 第10阶段：agent实现（问答模式）
- [ ] 问答模式支持
- [ ] 固定skill集成（如web-search）
- [ ] 不压缩对话历史

### 第11阶段：UI实现
- [ ] 模块架构设计
- [ ] agent-config（配置agent）
- [ ] memory-manage（管理记忆）

### 第12阶段：project模块实现（Rust库）
- [ ] 模块架构设计
- [ ] 职位管理功能
- [ ] Tool提供者实现（文件操作、命令执行、Skill）
- [ ] 笔记管理功能
- [ ] 指导文件加载器

### 第13阶段：agent实现（工程模式）
- [ ] 工程模式支持
- [ ] 与project模块集成
- [ ] 职位切换功能
- [ ] 工作区目录绑定
- [ ] 自定义指导文件加载
- [ ] LLM压缩和记忆丢弃两种上下文压缩方式

### 第14阶段：memory-ego进阶模式改造
- [ ] 记忆提取器实现
- [ ] 配置信息与记忆提取结合生成客观设定、角色设定JSON文件
- [ ] 进阶模式API

## API 约定

### 通用 API 设计原则
1. **路径无参数**：HTTP API 路径仅用于确定调用哪个处理函数，不包含任何动态参数
2. **参数全 JSON**：所有输入参数都通过请求体的 JSON 格式传递
3. **统一响应格式**：所有 API 响应都采用统一的 `ApiResponse` 格式，包含 success、data、error 字段
4. **数据结构分离**：
   - 模块内部使用优化的数据结构（可能包含 Arc、DashMap 等）
   - API 通信使用简化的标准数据结构（无 Arc，使用 HashMap，与 kissbot-api 模块保持一致）

### kissbot-api 模块
- 作为所有模块间通信的标准数据结构定义模块
- 提供各模块 API 的输入（Request）和输出（Response）数据结构
- 所有其他模块都应依赖 kissbot-api 模块来使用标准化的数据结构
- 避免不同模块间的数据结构不匹配问题
