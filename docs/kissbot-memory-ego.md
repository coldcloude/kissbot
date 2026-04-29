# kissbot-memory-ego 模块设计

## 模块概述
自我认知模块，记忆模块的子模块，独立于agent模块，和其他记忆模块读写同一个文件系统，负责管理agent的自我认知信息。

## 职责
- 调用memory基础模块的DirectoryManager进行目录管理
- 实现agent元数据管理（AgentManager）
- 提供管理agent元数据的HTTPS API（如新增agent接口）
- 每个agent ID对应一个客观设定，对应多个角色设定
- 实现用户识别信息管理、角色扮演管理、角色扮演关系管理，并提供HTTPS API
- 通过agent元数据生成agent的自我认知信息中的身份标识（MD文件）
- 实现agent自我认知信息管理（EgoManager），管理自我认知MD文件
- 通过记忆提取的方式生成agent的自我认知信息（第二阶段）
- 仅提供API，本身不封装为tool

## 架构设计
### 核心组件
- agent元数据管理器（AgentManager）
- 自我认知信息管理器（EgoManager）
- 用户识别信息管理器（UserRecognitionManager）
- 角色扮演管理器（RolePlayManager）
- 角色扮演关系管理器（RolePlayRelationManager）
- HTTPS API服务器
- 记忆提取器

## 数据结构
### Agent自我认知信息
- agent ID（唯一标识）
- 客观设定
  - 身份标识（JSON结构存储，可生成MD）
  - 用户识别信息（JSON结构存储，可生成MD）
- 多个角色设定
  - 角色扮演（JSON结构存储，可生成MD）
  - 角色扮演关系（JSON结构存储，可生成MD）

### Agent元数据结构
- id：agent唯一标识符（UUID）
- name：agent名称
- description：agent描述
- created_at：创建时间（格式：yyyy-MM-dd HH:mm:ss）

## Agent元数据操作函数
- create_agent：创建新的agent，需要name和description
- get_metadata：获取agent元数据
- update_agent_name：修改agent名称
- update_agent_description：修改agent描述
- update_agent_name_description：同时修改agent名称和描述

## 结构化设计

### 数据结构设计
除身份识别信息来源于agent元数据（JSON存储）外，用户识别信息、角色扮演、角色扮演关系也采用结构化设计。

### 管理模块设计
除agent元数据管理模块外，用户识别信息、角色扮演信息、角色扮演关系信息也增加对应的管理模块，包括增加和修改功能。

### 存储机制
- 所有内容在内部使用JSON文件存储，只有要求生成MD时才生成MD
- 所有JSON转MD采用dirty机制，JSON更新时只标记，需要生成MD时再同步

### 对外接口设计
查询数据（JSON格式返回）和查询MD文件分开为两个接口。

### 用户识别信息结构
用户识别信息应为一个列表，列表每项为一个用户，至少包括：
- **名称**：用户名称（外部填写，无自动生成）
- **身份**：枚举值，只有 所有者、管理员、其他用户 3项
- **关联的用户标识**：一个列表，每项标识应由用户使用的channel标识+用户在channel中的标识组成。可以从channel中获取列表后选择，也可回退为手动填写
- **和其他用户的关系**：一个列表，每项至少包括对方用户、关系字段，可以有描述字段
- **描述字段**（可选）：可以自由填写任意文本

### 角色扮演信息结构
角色扮演信息为agent在本次会话中扮演的角色，至少包括：
- **名称**：角色名称（外部填写，无自动生成）
- **描述字段**：可以自由填写任意文本
- 单独存放一个JSON文件

### 角色扮演关系信息结构
角色扮演关系信息为agent在本次会话中认为用户是哪个角色，至少包括：
- **名称**：角色名称（外部填写，无自动生成）
- **关联的用户名**：关联的用户名称
- **和agent角色的关系**：和agent角色的关系
- **和其他角色的关系**：一个列表，每项是一个关系，至少包括对方角色名，关系字段
- **描述字段**（可选）：可以自由填写任意文本

## API设计

### Agent元数据管理接口

#### 新增agent接口
- **接口**：新增agent
- **输入**：agent元数据（名称、描述）
- **返回**：成功或失败状态

#### 查询agent元数据接口
- **接口**：按agent ID查询
- **返回内容**：agent元数据（名称、描述、创建时间等）

#### 查询所有agent列表接口
- **接口**：查询所有agent
- **返回内容**：agent元数据列表（并发获取）

#### 修改agent名称接口
- **接口**：修改agent名称
- **输入**：agent ID、新名称
- **返回**：成功或失败状态

#### 修改agent描述接口
- **接口**：修改agent描述
- **输入**：agent ID、新描述
- **返回**：成功或失败状态

#### 同时修改名称和描述接口
- **接口**：同时修改agent名称和描述
- **输入**：agent ID、新名称、新描述
- **返回**：成功或失败状态

#### 按名称搜索agent接口
- **接口**：按名称搜索agent
- **输入**：搜索关键词
- **返回**：匹配的agent元数据列表

#### 按描述搜索agent接口
- **接口**：按描述搜索agent
- **输入**：搜索关键词
- **返回**：匹配的agent元数据列表

### 用户识别信息管理接口

#### 查询用户识别信息（JSON格式）
- **接口**：按agent ID查询用户识别信息JSON
- **返回内容**：用户识别信息列表（JSON格式）

#### 查询用户识别信息（MD格式）
- **接口**：按agent ID查询用户识别信息MD
- **返回内容**：用户识别信息内容（MD格式）

#### 增加用户
- **接口**：增加用户
- **输入**：用户信息（名称、身份、关联的用户标识等）
- **返回**：成功或失败状态

#### 修改用户
- **接口**：修改用户
- **输入**：用户信息
- **返回**：成功或失败状态

### 角色扮演信息管理接口

#### 查询角色扮演信息列表
- **接口**：按agent ID查询所有角色列表
- **返回内容**：角色名称列表

#### 查询角色扮演信息（JSON格式）
- **接口**：按agent ID + 角色名称查询角色扮演JSON
- **返回内容**：角色扮演信息（JSON格式）

#### 查询角色扮演信息（MD格式）
- **接口**：按agent ID + 角色名称查询角色扮演MD
- **返回内容**：角色扮演信息内容（MD格式）

#### 增加角色
- **接口**：增加角色
- **输入**：角色信息（名称、描述等）
- **返回**：成功或失败状态

#### 修改角色
- **接口**：修改角色
- **输入**：角色信息
- **返回**：成功或失败状态

### 角色扮演关系信息管理接口

#### 查询角色扮演关系信息列表
- **接口**：按agent ID + 角色名称查询所有角色关系列表
- **返回内容**：角色关系列表

#### 查询角色扮演关系信息（JSON格式）
- **接口**：按agent ID + 角色名称查询角色扮演关系JSON
- **返回内容**：角色扮演关系信息（JSON格式）

#### 查询角色扮演关系信息（MD格式）
- **接口**：按agent ID + 角色名称查询角色扮演关系MD
- **返回内容**：角色扮演关系信息内容（MD格式）

#### 增加角色关系
- **接口**：增加角色关系
- **输入**：角色关系信息
- **返回**：成功或失败状态

#### 修改角色关系
- **接口**：修改角色关系
- **输入**：角色关系信息
- **返回**：成功或失败状态

## 通信接口
- 输入：通过HTTPS API接收agent的查询请求
- 输出：通过HTTPS API返回agent的自我认知信息（JSON格式或MD格式分开接口）
- 文件系统：读取配置文件，与其他记忆模块共享文件系统

## 文件存储目录结构
根据记忆系统的文件存储目录设计：
- **记忆系统根目录**
  - **agent ID目录**
    - **agent-{agent-id}**：agent存在标识文件（由memory模块管理）
    - **metadata.json**：agent元数据JSON文件
    - **memory-ego**：memory-ego模块的设定信息单独存放
      - **identity.md**：自动生成的身份标识MD文件
      - **user-recognition.json**：用户识别信息JSON文件
      - **user-recognition.md**：用户识别信息MD文件（按需生成）
      - **role-play-{role-id}.json**：角色扮演JSON文件
      - **role-play-{role-id}.md**：角色扮演MD文件（按需生成）
      - **role-play-relation-{role-id}.json**：角色扮演关系JSON文件
      - **role-play-relation-{role-id}.md**：角色扮演关系MD文件（按需生成）

## 文件生成机制

### JSON文件来源
- 身份识别信息：来源于metadata.json
- 用户识别信息、角色扮演信息、角色扮演关系信息：外部填写或从记忆提取

### MD文件生成
- 所有JSON转MD采用dirty机制，JSON更新时只标记，需要生成MD时再同步
- 身份识别信息：从metadata.json中提取agent名称、创建时间等信息，自动生成identity.md
- 其他信息：根据对应的JSON文件按需生成MD文件

### 进阶模式
- 结合配置信息和从记忆中提取的信息生成JSON文件
- 支持自动化生成设定内容

## AgentManager实现
- 实现agent元数据的JSON文件存储（metadata.json）
- 使用DashMap实现高并发的manager_lock
- AgentMetadata共享元数据，内部使用Arc<String>减少复制开销
- 使用tokio::sync::RwLock实现读写锁防止竞争
- 使用内存缓存，首次读取后缓存到内存
- 双重锁定机制确保数据一致性
- 单例通过关联函数获取：AgentManager::get()

## EgoManager实现
- 使用本地kai-index库的DistinctIndex实现全文搜索
- 使用DashMap和DashSet实现高并发数据结构
- 使用Arc共享数据，避免克隆
- 脏标记机制（identity_dirty）：延迟更新identity.md和搜索索引
- force_sync_identity_md：强制同步，更新搜索索引并生成identity.md
- sync_identity_md：检查脏标记，仅在需要时同步
- search_by_name/search_by_description：先同步脏数据再搜索
- 启动时自动加载：get()初始化时加载所有agent到搜索索引
- 单例通过tokio::sync::OnceCell实现异步初始化

## 实现决策
- 不作为memory-struct的子模块，不实现memory-struct的接口
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 使用serde进行JSON序列化
- 支持从配置文件加载证书
- AgentManager在本模块实现，管理agent元数据的JSON文件存储
- AgentMetadata内部使用Arc<String>减少复制开销
- 使用dashmap实现高并发数据结构
- 使用本地kai-index库实现全文搜索
- 使用futures实现并发操作
- 使用Arc共享数据，减少克隆开销

## 开发计划

### 第1阶段：基础模式实现
- [x] 调用memory基础模块库（DirectoryManager）
- [x] 实现AgentManager管理agent元数据
- [x] agent元数据管理API（新增agent、查询agent元数据、修改agent名称描述）
- [x] 通过agent元数据生成身份识别MD文件
- [x] agent身份识别MD的读取API
- [x] HTTPS API接口

### 第2阶段：全文搜索实现
- [x] 使用本地kai-index库实现全文搜索
- [x] 使用dashmap实现高并发数据结构
- [x] 实现脏标记机制延迟更新
- [x] 实现name和description字段的全文搜索
- [x] 添加搜索API接口
- [x] 启动时自动加载所有agent到索引

### 第3阶段：用户识别信息、角色扮演、角色扮演关系管理
- [ ] 用户识别信息、角色扮演、角色扮演关系的JSON管理模块
- [ ] 用户识别信息、角色扮演、角色扮演关系MD文件生成的dirty机制
- [ ] JSON查询和MD查询分开的API

### 第4阶段：进阶模式改造
- [ ] 记忆提取器实现
- [ ] 配置信息与记忆提取结合生成客观设定、角色设定JSON文件
- [ ] 进阶模式API
