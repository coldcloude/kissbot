# kissbot-agent 模块设计

## 模块概述
智能体核心模块，负责将消息加工为LLM可用的消息，通过agentic loop调用LLM执行操作，返回消息。

## 职责
- 支持三种agent模式：问答模式、工程模式、自主模式
- 实现agentic loop，负责与LLM交互
- 管理tool调用机制
- 通过WSS客户端与channel通信
- 通过HTTPS客户端与memory-store通信
- 在agentic loop外，直接调用API与memory-ego和memory-store交互
- 在agentic loop内，通过LLM生成tool call与memory-struct交互
- 根据不同模式管理会话和上下文
- 支持上下文压缩（LLM压缩或从memory-struct重新读取）

## Agent三种模式

- [问答模式](./kissbot-agent-chat.md) - 问答的过程就是全部内容
- [工程模式](./kissbot-agent-project.md) - 做一件事的持续过程
- [自主模式](./kissbot-agent-autonomous.md) - 持续收集信息，并与其他人或agent交换信息的持续过程

## Agent与记忆系统交互设计

### 交互原则
1. **在agentic loop之外，直接调用API**：
   - agent从memory-ego读取双重设定
   - agent向memory-store推送记忆片段数据

2. **在agentic loop之内，由LLM生成tool call执行**：
   - agent从memory-struct查询记忆

## 核心组件设计

### 1. ModeManager - 模式管理器
- 管理三种agent模式的切换和配置
- 根据不同模式加载相应的配置和组件

### 2. SessionManager - 会话管理器
- 问答模式：管理单个会话
- 工程模式：管理多个会话的创建和切换
- 自主模式：不使用会话，从记忆加载

### 3. ContextBuilder - 上下文构建器
- 根据当前模式构建发送给LLM的完整上下文
- 包含系统消息、对话消息等
- 处理上下文压缩

### 4. AgenticLoop - Agentic循环
- 核心执行循环
- 调用LLM API
- 处理tool调用
- 循环执行直到完成

### 5. ToolManager - Tool管理器
- 根据不同模式加载相应的tool集合
- 执行tool调用
- 处理tool返回结果

### 6. MemoryPushManager - 记忆推送管理器
- 在agentic loop外推送记忆到memory-store
- 推送思考内容、工具调用、回复文本等

### 7. WSSClient - WSS客户端
- 与channel建立WSS连接
- 接收来自channel的消息
- 发送消息到channel

### 8. HTTPSClient - HTTPS客户端
- 与memory-store、memory-ego、memory-struct通信
- 调用API推送和查询记忆

## 数据结构定义

### AgentMode
```rust
enum AgentMode {
    Chat,         // 问答模式
    Project,      // 工程模式
    Autonomous,   // 自主模式
}
```

### Session
```rust
struct Session {
    id: String,
    mode: AgentMode,
    created_at: String,
    messages: Vec<Message>,
    is_compressed: bool,
}
```

### Message
```rust
enum Message {
    User {
        content: String,
        timestamp: String,
    },
    Assistant {
        content: String,
        thinking_key: Option<String>,
        tool_call_keys: Vec<String>,
        timestamp: String,
    },
    ToolCall {
        id: String,
        name: String,
        params: serde_json::Value,
        timestamp: String,
    },
    ToolResult {
        id: String,
        result: serde_json::Value,
        timestamp: String,
    },
    Tombstone {
        summary: String,
        replaced_count: usize,
        timestamp: String,
    },
}
```

### SystemPromptConfig
```rust
struct SystemPromptConfig {
    setup_guide: String,
    basic_principles: String,
    setting_keep_instruction: String,
    ego_content: Option<String>,  // 自主模式使用
    role_setting: Option<String>,  // 工程模式使用
}
```

### ProjectConfig
```rust
struct ProjectConfig {
    workspace_path: String,
    role_setting: String,
    agents_md_path: Option<String>,
}
```

### AutonomousConfig
```rust
struct AutonomousConfig {
    agent_id: String,
    load_role: Option<String>,
    include_channels: Option<Vec<String>>,
    exclude_channels: Option<Vec<String>>,
    load_message_count: Option<usize>,
    idle_timeout_seconds: u64,
    reset_timeout_seconds: u64,
}
```

## Agentic Loop流程

```
1. 接收来自channel的消息（或自主触发）
   ↓
2. 将消息添加到会话/上下文中
   ↓
3. 在agentic loop外，将消息推送到memory-store
   ↓
4. 检查上下文是否需要压缩或重置
   ├─ 是 → 执行压缩或重置
   └─ 否 → 继续
   ↓
5. 构建完整的LLM上下文（系统消息 + 对话消息）
   ↓
6. 调用LLM API
   ↓
7. 处理LLM返回结果
   ├─ 有tool call → 执行tool
   │   ├─ 执行tool
   │   ├─ 将tool调用和结果推送到memory-store（在agentic loop外）
   │   └─ 返回步骤5
   └─ 无tool call → 生成最终回复
   ↓
8. 将思考内容（如果有）推送到memory-store（在agentic loop外）
   ↓
9. 将回复发送到channel
   ↓
10. 将回复推送到memory-store（在agentic loop外）
    ↓
11. 等待下一条消息或自主触发
```

## 配置文件

### agent配置文件示例
```json
{
    "mode": "chat",
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
    "memory_ego_https": {
        "url": "https://memory-ego.example.com"
    },
    "project_config": null,
    "autonomous_config": null
}
```

## 实现决策
- 使用tokio作为异步运行时
- 使用tokio-tungstenite实现WSS客户端
- 使用reqwest实现HTTPS客户端
- 使用serde进行JSON序列化
- 使用dashmap实现高并发数据结构
- 使用futures实现异步任务
- 支持从配置文件加载证书
- 单例通过关联函数获取

## 基础和公共部分开发计划

### 第1阶段：基础结构搭建
- [ ] 配置Cargo.toml，添加依赖
- [ ] 定义模块结构
- [ ] 定义错误类型
- [ ] 定义核心数据结构（AgentMode、Session、Message、配置等）

### 第2阶段：通信组件实现
- [ ] 实现WSSClient
- [ ] 实现HTTPSClient
- [ ] 实现与channel的通信
- [ ] 实现与memory-store的通信
- [ ] 实现与memory-ego的通信
- [ ] 实现与memory-struct的通信

### 第3阶段：模式和会话管理
- [ ] 实现ModeManager
- [ ] 实现SessionManager
- [ ] 实现配置加载和管理

### 第4阶段：Agentic Loop实现
- [ ] 实现ContextBuilder
- [ ] 实现LLM API集成
- [ ] 实现ToolManager
- [ ] 实现完整的Agentic Loop

### 第5阶段：记忆交互实现
- [ ] 实现MemoryPushManager
- [ ] 实现系统提示词构建
- [ ] 实现消息处理和状态管理

### 第6阶段：测试和完善
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化

---

**注意**：三种模式的具体实现请参考对应文档：
- [问答模式开发计划](./kissbot-agent-chat.md)
- [工程模式开发计划](./kissbot-agent-project.md)
- [自主模式开发计划](./kissbot-agent-autonomous.md)
