# kissbot 整体设计

## 项目概述
实现一个具有永久记忆的分布式的智能机器人框架，采用多进程架构，模块间通过WSS、HTTPS或文件进行通信。

## 模块架构

### 顶层模块
| 模块名称 | 类型 | 说明 |
|---------|------|------|
| kissbot-agent | Rust | 智能体核心，处理 LLM 调用和 agentic loop，支持三种模式 |
| kissbot-channel | Rust | 消息通道框架，连接外部系统 |
| kissbot-memory | Rust | 记忆基础模块（存储组织结构） |
| kissbot-project | Rust Lib | 工程模块，管理工程职位和提供tool |
| kissbot-api | Rust | API 定义模块，各模块间通信的标准数据结构 |

### 子模块/实现模块
| 模块名称 | 类型 | 说明 |
|---------|------|------|
| kissbot-channel-web | Rust | Web消息通道后台（WSS服务器） |
| kissbot-channel-web-ui | React + Vite | Web消息通道前台（用户界面） |
| kissbot-memory-store | Rust | 记忆存储模块，收集和存储原始记忆 |
| kissbot-memory-struct | Rust | 记忆结构框架，提供记忆查询功能 |
| kissbot-memory-struct-abstract | Rust | 记忆结构实现（摘要搜索，纯后台） |
| kissbot-memory-ego | Rust | 自我认知模块（agent自我认知信息管理） |
| kissbot-agent-config | React + Vite | 智能体配置UI |
| kissbot-memory-manage | React + Vite | 记忆管理UI |

## Agent三种模式

### 一、问答模式
- 问答的过程就是全部内容
- 没有工作区
- 不加载ego模块
- 不使用agent-id

### 二、工程模式
- 做一件事的持续过程
- 不加载ego模块
- 不使用agent-id
- 每个工程绑定一个本地目录作为工作区
- 工作区内包含工程职位的配置文件
- agent每次选择一个职位，按照职位设定并完成工作
- 工程管理模块（kissbot-project）负责配置工作区目录、读写角色设定、提供tool

### 三、自主模式
- 持续收集信息，并与其他人或agent交换信息的持续过程
- 加载ego模块
- 固定使用一个agent-id
- 没有工作区

## 整体框架与模块责任

### 模块责任和联系
1. **channel**：连接外部系统，如web界面、QQ、Matrix等，向agent输入消息，并接收agent的输出消息。管理非文本内容的本地存储，生成反查key。
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
6. **memory-store ↔ memory-struct-\***：共同读写文件系统，memory-store有新数据时通过HTTPS通知注册的各memory-struct实现
7. **各WebUI前端 ↔ 后端**：HTTPS
8. **agent-config ↔ agent后端**：HTTPS
9. **memory-manage ↔ memory-store、memory-struct-*、memory-ego后端**：HTTPS

### 开发重心

**开发重心为：自主模式agent、记忆系统**

## agent设计

### agent上下文设计

#### 问答模式上下文
- 只有一个会话
- 系统消息：将Agent设定为通用助手
- Tool：仅加载用于获取信息的固定skill，如web-search
- 对话消息：所有的用户消息、agent消息（包括工具调用和结果），不压缩

#### 工程模式上下文
- 手动新建和切换会话
- 系统消息：
  - agent必须遵守的规范，包括禁止事项（由工程管理模块提供）
  - agent的职位设定（由工程管理模块提供）
  - 从工作区内加载自定义指导文件AGENTS.md等
- Tool：
  - 文件操作类（Read、Write、Edit）
  - 命令执行类（Bash）
  - 扩展类（Skill）
  - （可选）加载memory-struct-*模块，使用其提供的工具
- 对话消息：
  - 手动新建和切换会话，只包含会话内的用户消息、agent消息（包括工具调用和结果）
  - 如存在memory-struct-*模块，调用其工具读取同一工程或同一会话的记忆
  - 会话可以压缩，可选两种压缩方式：
    1. LLM压缩：当上下文空间不足时，或手动指定时，提取历史消息摘要，生成墓碑消息替换历史消息
    2. 如果加载了memory-struct-*模块，当上下文空间不足时，自动丢弃已推送至记忆模块的对话，然后重新通过memory-struct-*模块读取记忆

#### 自主模式上下文
- 没有会话概念，对话消息从记忆系统加载
- 系统消息：
  - agent必须遵守的规范，包括禁止事项（由memory-ego的agent元数据模块管理）
  - memory-ego模块读取的内容。可以选择加载一个角色，也可以不选择，只加载客观部分
  - agent自主运行时的目标（由memory-ego模块管理）
- Tool：
  - 记忆模块：选择加载一个memory-struct-*模块，使用其提供的工具
  - 扩展模块：仅加载API、MCP，或使用MCP封装的其他tool
- 对话消息：
  1. 使用memory-struct-*模块提供的工具，读取近期记忆（按时间）
  2. 加载最近的消息，可以按时间或者条数（可以指定include和exclude部分channel）
  3. 后续所有用户消息、agent消息（包括工具调用和结果，包括memory-struct工具的结果）
  4. 如果上下文超长，或长时间未重置时，进行重置，即将所有消息存入记忆，然后清空对话消息并重新加载1和2
  5. 长时间空闲时，加载自主运行目标（从memory-ego模块加载），使agent主动进行一些信息收集或输出

## 记忆系统设计

### 记忆基础模块（kissbot-memory）
- 记忆基础模块，实现记忆存储的组织结构
- 定义记忆文件的存储目录
- 为其他记忆模块提供基础库支持

### 记忆存储模块（kissbot-memory-store）
- 存储所有原始记忆数据

#### 记忆来源
1. **Channel消息**：包含channel中的全部文本内容。非文本内容（图片、音频、视频、二进制文件等）以附件形式存储在channel本地，记忆中仅存储用于反查非文本内容的key。
2. **大模型输出**：
   - 思考内容：全文发送至记忆系统单独存储，仅发送用于反查的key至channel
   - 工具调用指令：tool call的name和parameter全部发送至记忆系统，仅发送用于反查的key至channel
   - 回复文本：全文发送至channel，由channel发送至记忆系统
3. **工具输出**：仅包含工具API直接返回的内容，不包含副产物（如工具写入的文件等）。全文发送至记忆系统，将摘要信息和调用指令的key发送至channel。工具输出没有独立的key，应和工具调用指令（含key）一并存储。

#### 记忆存储方式
- 记忆不按照channel区分，所有channel的记忆按时间顺序混合存储
- 问答模式：每个问答会话存一个记忆
- 工程模式：切分多个会话时，每个会话一个记忆
- 自主模式：每个agent-id一个记忆
- 实现上，一个记忆按日期拆分多个文件存储，按年分多个目录存储

#### 结构化存储文件
根据记忆来源，每个记忆形成3个记忆存储文件，文件内按时间顺序存储一条条记录：

1. **channel文本记录文件**：存储channel内的文本内容，每条记录包括channel-id、user-id、是否为agent自己、时间、序号、原文（非文本内容的原文为反查key）。
2. **思考内容原文文件**：存储思考内容，每条记录包括原文、key、时间。
3. **工具调用记录文件**：存储工具调用和返回信息，包括调用信息（工具的name、parameter）、返回信息、key、时间。

### 自我认知模块（memory-ego）
- 用于实现Agent的自我认知双重设定
- 记忆模块的子模块，独立于agent模块，和其他记忆模块读写同一个文件系统
- 该模块通过配置文件读取，通过API配置或者通过记忆提取的方式，生成并保存agent的自我认知信息
- 每个agent ID对应一个客观设定，对应多个角色设定
- 该模块仅使用API，本身不封装为tool。agent一般使用API查询、更新自我认知。如有自动化必要，在使用其API的模块中封装为tool

#### Agent自我认知设定详细内容
Agent的自我认知是记忆的一部分，应具有客观、角色双重设定：

##### 一、客观设定
- 包括agent自身的客观状态
- 包括agent和用户的客观关系

##### 二、角色设定
- 包括agent在本次会话中扮演的角色
- 包括agent在本次会话中认为客户扮演的角色

##### 具体内容
agent自我认知信息包括以下几个部分：
1. **身份标识**：关于agent身份的客观事实，比如名称、创建时间
2. **用户识别信息**：agent和各个用户的客观关系，各个用户间的客观关系
3. **角色扮演**：本次会话中对外展现的角色，包括agent对自己的称呼，用户可能对agent的各种称呼，对话采用的语气、用词风格
4. **角色扮演关系**：各个用户在本次对话中的角色，agent在本次对话中对用户角色的称呼

##### Agent自我认知面向的场景
agent设计面向多用户环境，具体要求：
- agent需要认识每一个用户，并区分哪个用户是管理员
- agent在不同的会话中要扮演不同的角色，但这不影响agent客观上是谁
- agent在会话中不体现自身的客观信息（要求不扮演的情况除外）
- 在角色扮演的过程中，agent还需要将不同的用户也识别成不同的角色
- 用户按照名称识别，可以关联不同的channelID+userID
- 角色和用户角色也按照名称识别，用户角色可以直接关联到channelID+userID，也可以关联到用户名称，再关联推断

#### memory-ego模块的结构化设计

##### 数据结构设计
身份识别信息来源于agent元数据（JSON存储），用户识别信息、角色扮演、角色扮演关系也采用结构化设计

##### 管理模块设计
agent元数据有独立的管理模块，用户识别信息、角色扮演信息、角色扮演关系信息也有对应的管理模块，包括增加和修改功能

##### 存储机制
- 所有内容在内部使用JSON文件存储，只有要求生成MD时才生成MD
- 所有JSON转MD采用dirty机制，JSON更新时只标记，需要生成MD时再同步

##### 对外接口设计
查询数据（JSON格式返回）和查询MD文件分开为两个接口

##### 身份识别信息结构
身份识别信息应为一个对象，包括：
- **ID**：agent ID（自动生成，不生成到MD中）
- **创建时间**：agent创建时间（自动生成）
- **名称**：agent名称（外部填写，无自动生成）
- **描述字段**：可以自由填写任意文本

##### 用户识别信息结构
用户识别信息应为一个列表，列表每项为一个用户，至少包括：
- **名称**：用户名称（外部填写，无自动生成）
- **身份**：枚举值，只有 所有者、管理员、其他用户 3项
- **关联的用户标识**：一个列表，每项标识应由用户使用的channel标识+用户在channel中的标识组成。可以从channel中获取列表后选择，也可回退为手动填写
- **和其他用户的关系**：一个列表，每项至少包括对方用户、关系字段，可以有描述字段
- **描述字段**（可选）：可以自由填写任意文本

##### 角色扮演信息结构
角色扮演信息为agent在本次会话中扮演的角色，至少包括：
- **名称**：角色名称（外部填写，无自动生成）
- **描述字段**：可以自由填写任意文本
- 单独存放一个JSON文件

##### 角色扮演关系信息结构
角色扮演关系信息为agent在本次会话中认为用户是哪个角色，至少包括：
- **名称**：角色名称（外部填写，无自动生成）
- **关联的用户名**：关联的用户名称
- **和agent角色的关系**：和agent角色的关系
- **和其他角色的关系**：一个列表，每项是一个关系，至少包括对方角色名，关系字段
- **描述字段**（可选）：可以自由填写任意文本

### 记忆系统文件存储目录设计

#### Agent记忆目录（自主模式使用）
```
记忆系统根目录/
├── {agent-id}/
│   ├── agent-{agent-id}           # agent存在标识文件
│   ├── metadata.json              # agent元数据
│   ├── memory-ego/                # memory-ego模块设定信息
│   ├── memory-store/              # memory-store原始记忆
│   │   └── {year}/                # 按日期分组
│   │       ├── channel-records-{date}.jsonl
│   │       ├── thinking-records-{date}.jsonl
│   │       └── tool-records-{date}.jsonl
│   └── memory-struct-*/           # memory-struct实现数据
└── ...
```

#### 工程工作区目录（工程模式使用）
```
{workspace-path}/
├── .kissbot/
│   ├── project.json               # 工程配置
│   └── roles/                     # 职位配置
│       ├── developer.json
│       └── ...
├── AGENTS.md                      # 自定义指导文件
└── (工程实际文件)
```

## Agent与记忆系统交互设计

### 交互原则
1. **在agentic loop之外，直接调用API**：
   - agent从memory-ego读取设定（仅自主模式）
   - agent向memory-store推送记忆片段数据
2. **在agentic loop之内，由LLM生成tool call执行**：
   - agent从memory-struct-*查询记忆
3. **记忆和上下文的关系**：
   - 记忆的内容不直接加入agent上下文，agent上下文也不直接产生记忆。

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

## 模块设计文档索引
各模块详细设计请参考以下文档：

- [kissbot-agent.md](./kissbot-agent.md) - 智能体核心模块设计
- [kissbot-channel.md](./kissbot-channel.md) - 消息通道框架设计
- [kissbot-memory-store.md](./kissbot-memory-store.md) - 记忆存储模块设计
- [kissbot-project.md](./kissbot-project.md) - 工程模块设计
- [kissbot-memory.md](./kissbot-memory.md) - 记忆基础模块设计
- [kissbot-memory-ego.md](./kissbot-memory-ego.md) - 自我认知模块设计
- [kissbot-api.md](./kissbot-api.md) - API定义模块设计

## 实现步骤

### 第1阶段：核心模块初始化
- [x] 初始化所有Rust项目
- [x] 初始化所有React+Vite项目
- [x] 建立项目目录结构

### 第2阶段：memory基础模块实现
- [x] 模块架构设计
- [x] 定义记忆存储目录结构
- [x] 提供基础库供其他记忆模块使用

### 第3阶段：memory-ego实现（基础模式）
- [x] 模块架构设计
- [x] 实现agent元数据JSON文件存储（带读写锁）
- [x] agent元数据管理API（新增agent、查询agent元数据）
- [x] HTTPS API接口
- [x] 用户识别信息、角色设定的JSON管理模块
- [ ] 用户识别信息、角色设定的查询API

### 第4阶段：memory-store实现
- [ ] 模块架构设计
- [ ] 三种记录类型的存储（JSON Lines格式）
- [ ] HTTPS API接口（推送、查询）
- [ ] 订阅/通知机制

### 第5阶段：agent实现
- [ ] 模块架构设计
- [ ] 三种模式支持（问答、工程、自主）
- [ ] LLM API集成
- [ ] agentic loop实现
- [ ] tool调用机制
- [ ] WSS客户端
- [ ] 上下文压缩功能

### 第6阶段：channel实现
- [ ] 模块架构设计
- [ ] 框架trait定义
- [ ] 附件存储管理
- [ ] WSS服务器实现
- [ ] HTTPS API接口
- [ ] channel-web后台实现
- [ ] channel-web-ui前台实现

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
- [ ] 配置信息与记忆提取结合生成客观设定、角色设定JSON文件
- [ ] 进阶模式API

### 第10阶段：project模块设计（Rust库）
- [ ] 模块架构设计
- [ ] 职位管理功能
- [ ] Tool提供者实现

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
