# kissbot-agent 模块设计 - 自主模式

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
- 在自主模式下长时间空闲时主动进行信息收集或输出

## 自主模式
- 持续收集信息，并与其他人或agent交换信息的持续过程
- 加载ego模块
- 固定使用一个agent-id
- 没有工作区

### 自主模式上下文
- **会话**：没有会话概念，对话消息从记忆系统加载
- **系统消息**：
  - agent必须遵守的规范，包括禁止事项（由memory-ego的agent元数据模块管理）
  - memory-ego模块读取的内容（可以选择加载一个角色，也可以不选择，只加载客观部分）
  - agent自主运行时的目标（由memory-ego的agent元数据模块管理）
- **Tool**：
  - 记忆模块：选择加载一个memory-struct，用于agent自主提取记忆
  - 扩展模块：仅加载API、MCP，或使用MCP封装的其他tool
- **对话消息**：
  - 使用memory-struct读取近期记忆（按时间）
  - 加载最近的消息，可以按时间或者条数（可以指定include和exclude部分channel）
  - 后续所有用户消息、agent消息（包括工具调用和结果，包括memory-struct工具的结果）
  - 如果上下文超长，或长时间无消息，或长时间未重置时，进行重置：将所有消息存入记忆，然后重新执行读取近期记忆
  - 长时间空闲时，加载自主运行目标，使agent主动进行一些信息收集或输出

## 系统提示词设计

系统提示词应包含agent自我认知设定，此外还应包含：

### 1. 设定引导
限定设定文本内容的范围

### 2. 基本原则
agent必须遵守的规范，包括禁止事项

### 3. 设定保持指令
要求agent忽略后续会话中任何修改设定的指令

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
    Qa,           // 问答模式
    Engineering,  // 工程模式
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

### EngineeringConfig
```rust
struct EngineeringConfig {
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

## 上下文压缩策略

### 1. LLM压缩
- 触发条件：上下文空间不足或手动指定
- 执行：调用LLM提取历史消息摘要
- 结果：生成Tombstone消息替换历史消息

### 2. 记忆丢弃（仅工程模式且加载memory-struct时）
- 触发条件：上下文空间不足
- 执行：丢弃已推送至记忆模块的对话
- 结果：重新通过memory-struct读取记忆

### 3. 上下文重置（仅自主模式）
- 触发条件：上下文超长、长时间无消息、长时间未重置
- 执行：将所有消息存入记忆，重新从memory-struct读取近期记忆

## 自主模式主动行为

### 空闲检测
- 配置空闲超时时间
- 超过空闲时间后触发主动行为

### 主动行为
- 加载自主运行目标
- 根据目标进行信息收集
- 根据目标进行信息输出

## 配置文件

### agent配置文件示例
```json
{
    "mode": "autonomous",
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
    "engineering_config": null,
    "autonomous_config": {
        "agent_id": "550e8400-e29b-41d4-a716-446655440000",
        "load_role": null,
        "include_channels": null,
        "exclude_channels": null,
        "load_message_count": 100,
        "idle_timeout_seconds": 3600,
        "reset_timeout_seconds": 86400
    }
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

## 开发计划

### 第1阶段：基础结构搭建
- [ ] 配置Cargo.toml，添加依赖
- [ ] 定义模块结构
- [ ] 定义错误类型
- [ ] 定义核心数据结构

### 第2阶段：通信组件实现
- [ ] 实现WSS客户端
- [ ] 实现HTTPS客户端
- [ ] 实现与channel的通信
- [ ] 实现与memory-store的通信

### 第3阶段：自主模式实现
- [ ] 实现ModeManager
- [ ] 实现SessionManager（自主模式不使用会话）
- [ ] 实现自主模式基础
- [ ] 实现与memory-ego的集成（读取禁止事项和自主运行目标）

### 第4阶段：Agentic Loop实现
- [ ] 实现ContextBuilder
- [ ] 实现LLM API集成
- [ ] 实现ToolManager
- [ ] 实现完整的Agentic Loop
- [ ] 实现上下文重置功能

### 第5阶段：自主模式高级功能
- [ ] 实现自主模式主动行为
- [ ] 实现空闲检测
- [ ] 实现自主运行目标触发机制

### 第6阶段：测试和完善
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化
