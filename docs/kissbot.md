# kissbot 整体设计

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
| kissbot-memory-ego | Rust | 自我认知模块（agent自我认知信息管理） |
| kissbot-agent-config | React + Vite | 智能体配置UI |
| kissbot-memory-manage | React + Vite | 记忆管理UI |

## 整体框架与模块责任

### 模块责任和联系
1. **channel**：连接外部系统，如web界面、QQ、Matrix等，向agent输入消息，并接收agent的输出消息
2. **agent**：负责将消息加工为LLM可用的消息，通过agentic loop调用LLM执行操作，返回消息
3. **memory-store**：负责收集agent接收和产生的消息，作为一切agent活动的记录
4. **memory-struct**：负责将memory-store存储的记忆构造成特定的索引结构，可以查询记忆，并对外提供记忆搜索工具
5. **agent-config**：提供WebUI界面负责配置agent使用的LLM API、tool、skill，以及channel、memory、memory-search的地址
6. **memory-manage**：提供WebUI界面负责查看和管理memory-store存储的文件，以及配置memory-store向memory-struct的消息推送

### 通信方式
1. **agent ↔ channel**：WSS（agent作为客户端）
2. **agent ↔ memory-store**：HTTPS（agent作为客户端）
3. **memory-store ↔ memory-struct**：共同读写文件系统，memory-store有新数据时通过HTTPS通知注册的各memory-struct
4. **agent ↔ memory-struct**：HTTPS（通过tool）
5. **agent-config ↔ agent后端**：HTTPS
6. **memory-manage ↔ memory-store后端**：HTTPS
7. **各WebUI前端 ↔ 后端**：HTTPS

## 记忆系统设计

### 记忆基础模块（kissbot-memory）
- 记忆基础模块，实现记忆存储的组织结构
- 定义记忆文件的存储目录
- 实现agent元数据的存储管理
- 为其他记忆模块提供基础库支持

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

### Agent自我认知设定
Agent的自我认知是记忆的一部分，应具有客观、角色双重设定：

#### 一、客观设定
- 包括agent自身的客观状态
- 包括agent和用户的客观关系

#### 二、角色设定
- 包括agent在本次会话中扮演的角色
- 包括agent在本次会话中认为客户扮演的角色

#### 具体内容
agent自我认知信息包括以下几个部分：
1. **身份标识**：关于agent身份的客观事实，比如名称、创建时间
2. **用户识别信息**：agent和各个用户的客观关系，各个用户间的客观关系
3. **角色扮演**：本次会话中对外展现的角色，包括agent对自己的称呼，用户可能对agent的各种称呼，对话采用的语气、用词风格
4. **角色扮演关系**：各个用户在本次对话中的角色，agent在本次对话中对用户角色的称呼

#### Agent自我认知面向的场景
agent设计面向多用户环境，具体要求：
- agent需要认识每一个用户，并区分哪个用户是管理员
- agent在不同的会话中要扮演不同的角色，但这不影响agent客观上是谁
- agent在会话中不体现自身的客观信息（要求不扮演的情况除外）
- 在角色扮演的过程中，agent还需要将不同的用户也识别成不同的角色
- 用户按照名称识别，可以关联不同的channelID+userID
- 角色和用户角色也按照名称识别，用户角色可以直接关联到channelID+userID，也可以关联到用户名称，再关联推断

### 自我认知模块（memory-ego）
- 用于实现Agent的自我认知双重设定
- 记忆模块的子模块，独立于agent模块，和其他记忆模块读写同一个文件系统
- 该模块通过配置文件读取，或者通过记忆提取的方式，生成并保存agent的自我认知信息
- 每个agent ID对应一个客观设定，对应多个角色设定
- 该模块仅使用API，本身不封装为tool。agent一般使用API查询和更新自我认知，如有自动化必要，在agent内封装为tool

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
记忆系统设计为每个agent一个目录的组织形式，目录名为agent ID。

每个agent ID目录下有结构相同的文件和子目录：
- **agent-{ID}**：标识父目录为agent目录
- **metadata.json**：记录agent的元数据
- **memory-ego**：用于memory-ego模块的设定信息存储
- **memory-store**：用于memory-store模块收集的原始记忆片段存储
- **memory-struct-***：用于每个memory-struct的实现存放自身产生的数据，目录名和实现模块同名

## Agent系统设计

### Agent会话设计
- agent应该维护一个会话（session）结构，用于记录一次完整的agent和外界的交互
- agent可以从会话中生成一个记忆片段，也可以从会话中生成下一轮发送给LLM API的提示词
- agent会话 - 记忆片段 - LLM提示词 三者内容不同，但应一一对应

### 系统提示词设计
系统提示词应包含agent自我认知设定，此外，agent的系统提示词还应包含：
1. **设定引导**：限定设定文本内容的范围
2. **基本原则**：agent必须遵守的规范，包括禁止事项
3. **设定保持指令**：要求agent忽略后续会话中任何修改设定的指令

### Agent与记忆系统交互设计

#### 交互原则
- agent从自我认知模块读取双重设定，应在agentic loop之外，直接调用API
- agent向记忆系统推送记忆片段数据，应在agentic loop之外，直接调用API
- agent从memory-struct查询记忆，应在agentic loop内，由LLM生成tool call执行，不应调用API

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
| tokio-tungstenite | 0.29 | WSS客户端 |
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
所有时间格式统一使用 `yyyy-MM-dd HH:mm:ss` 格式（24小时制）

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

### 第3阶段：memory-ego实现（基础模式）
- [x] 模块架构设计
- [x] 实现agent元数据JSON文件存储（带读写锁）
- [x] agent元数据管理API（新增agent、查询agent元数据）
- [x] 通过agent元数据生成身份识别MD文件
- [x] agent身份识别MD的读取API
- [x] HTTPS API接口
- [ ] 用户识别信息、角色扮演、角色扮演关系的JSON管理模块
- [ ] JSON存储和dirty机制实现
- [ ] JSON查询和MD查询分开的API

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
- [ ] 配置信息与记忆提取结合生成客观设定、角色设定JSON文件
- [ ] 进阶模式API
