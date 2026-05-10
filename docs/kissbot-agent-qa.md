# kissbot-agent 模块设计 - 问答模式

## 问答模式
- 问答的过程就是全部内容
- 没有工作区
- 不加载ego模块
- 不使用agent-id

### 问答模式上下文
- **会话**：只有一个会话
- **系统消息**：将Agent设定为通用助手
- **Tool**：仅加载用于获取信息的固定skill（如web-search）
- **对话消息**：所有的用户消息、agent消息（包括工具调用和结果），不压缩

## 问答模式配置文件示例
```json
{
    "mode": "qa",
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
    "memory_struct_https": null,
    "memory_ego_https": null,
    "engineering_config": null,
    "autonomous_config": null
}
```

## 问答模式开发计划

### 第1阶段：基础结构
- [ ] 配置Cargo.toml，添加依赖（复用自主模式基础结构）
- [ ] 实现问答模式的ModeManager配置
- [ ] 实现单个会话的SessionManager

### 第2阶段：问答模式核心
- [ ] 实现问答模式的ContextBuilder
- [ ] 实现固定skill集成（web-search等）
- [ ] 实现不压缩的对话历史管理

### 第3阶段：问答模式集成
- [ ] 集成问答模式到主agent
- [ ] 问答模式配置加载

### 第4阶段：测试和完善
- [ ] 问答模式单元测试
- [ ] 问答模式集成测试
