# kissbot-agent 模块设计 - 自主模式

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

## 自主模式上下文压缩策略

### 上下文重置（仅自主模式）
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

## 自主模式配置文件示例
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

## 自主模式开发计划

### 第1阶段：自主模式基础
- [ ] 实现自主模式的ModeManager配置
- [ ] 实现与memory-ego的集成（读取禁止事项和自主运行目标）
- [ ] 实现自主模式的ContextBuilder

### 第2阶段：Agentic Loop实现
- [ ] 实现完整的Agentic Loop
- [ ] 实现上下文重置功能

### 第3阶段：自主模式高级功能
- [ ] 实现自主模式主动行为
- [ ] 实现空闲检测
- [ ] 实现自主运行目标触发机制

### 第4阶段：自主模式集成
- [ ] 集成自主模式到主agent
- [ ] 自主模式配置加载

### 第5阶段：测试和完善
- [ ] 自主模式单元测试
- [ ] 自主模式集成测试
- [ ] 性能优化
