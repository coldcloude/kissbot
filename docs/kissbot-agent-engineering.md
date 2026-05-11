# kissbot-agent 模块设计 - 工程模式

## 工程模式
- 做一件事的持续过程
- 不加载ego模块
- 不使用agent-id
- 每个工程绑定一个本地目录作为工作区
- 工作区内包含工程职位的配置文件
- agent每次选择一个职位，按照职位设定读写笔记并完成工作
- 工程管理模块负责配置工作区目录、读写角色设定、提供tool

### 工程模式上下文
- **会话**：手动新建和切换会话
- **系统消息**：
  - agent必须遵守的规范，包括禁止事项（由工程管理模块提供）
  - agent的职位设定（由工程管理模块提供）
  - 从工作区内加载自定义指导文件AGENTS.md等
- **Tool**：
  - 文件操作类（Read、Write、Edit）
  - 命令执行类（Bash）
  - 扩展类（Skill）
- **记忆模块**（可选）：加载一个memory-struct模块读取同一工程或同一会话的记忆
- **对话消息**：
  - 手动新建和切换会话，只包含会话内的用户消息、agent消息（包括工具调用和结果）
  - 可选两种压缩方式：
    1. **LLM压缩**：当上下文空间不足时，或手动指定时，提取历史消息摘要，生成墓碑消息替换历史消息
    2. **记忆丢弃**：如果加载了memory-struct，当上下文空间不足时，自动丢弃已推送至记忆模块的对话，然后重新通过memory-struct读取记忆

## 工程模式上下文压缩策略

### 1. LLM压缩
- 触发条件：上下文空间不足或手动指定
- 执行：调用LLM提取历史消息摘要
- 结果：生成Tombstone消息替换历史消息

### 2. 记忆丢弃（仅工程模式且加载memory-struct时）
- 触发条件：上下文空间不足
- 执行：丢弃已推送至记忆模块的对话
- 结果：重新通过memory-struct读取记忆

## 工程模式配置文件示例
```json
{
    "mode": "engineering",
    "llm_api": {
        "base_url": "https://api.example.com",
        "api_key": "your-api-key",
        "model": "gpt-4",
        "max_tokens": 4096
    },
    "channel_wss": {
        "url": "wss://channel.example.com/ws"
    },
    "memory_store_https": {
        "url": "https://memory-store.example.com"
    },
    "memory_struct_https": {
        "url": "https://memory-struct.example.com"
    },
    "memory_ego_https": null,
    "engineering_config": {
        "workspace_path": "/path/to/workspace",
        "role_setting": "developer",
        "agents_md_path": "/path/to/workspace/AGENTS.md"
    },
    "autonomous_config": null
}
```

## 工程模式核心组件

### EngineeringManager - 工程管理器
- 与project模块交互
- 管理工作区目录
- 读取和应用职位设定
- 加载自定义指导文件（AGENTS.md）
- 提供职位笔记读写功能

### 工程模式Tool
工程模式提供以下tool：
1. **Read** - 读取文件
2. **Write** - 写入文件
3. **Edit** - 编辑文件
4. **Bash** - 执行命令
5. **Skill** - 扩展skill

## 工程模式开发计划

### 第1阶段：基础结构
- [ ] 配置Cargo.toml，添加依赖（复用自主模式基础结构）
- [ ] 实现工程模式的ModeManager配置
- [ ] 实现多个会话的SessionManager

### 第2阶段：project模块集成
- [ ] 实现EngineeringManager
- [ ] 与project模块集成
- [ ] 实现工作区目录绑定
- [ ] 实现职位切换功能

### 第3阶段：工程模式核心
- [ ] 实现工程模式的ContextBuilder
- [ ] 实现系统消息构建（包含禁止事项、职位设定、自定义指导）
- [ ] 实现文件操作类Tool（Read、Write、Edit）
- [ ] 实现命令执行类Tool（Bash）
- [ ] 实现扩展类Tool（Skill）

### 第4阶段：上下文压缩
- [ ] 实现LLM压缩
- [ ] 实现记忆丢弃压缩
- [ ] 集成memory-struct

### 第5阶段：工程模式集成
- [ ] 集成工程模式到主agent
- [ ] 工程模式配置加载

### 第6阶段：测试和完善
- [ ] 工程模式单元测试
- [ ] 工程模式集成测试
