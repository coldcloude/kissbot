# kissbot 手动测试脚本说明

本目录提供 kissbot 各组件的手动启动脚本（`start-*.sh`、`reset-*.sh`），配置模板在 `template/`。

- `reset-agent.sh`：按 `template/nexus.json`、`template/station.json` 生成 `../workspace/agent-data/` 下的运行配置
- `reset-workspace.sh`：重置测试数据目录
- `start-memory-store.sh` / `start-memory-ego.sh` / `start-channel-web.sh` / `start-agent.sh`：依次启动各组件
- `inject-key.mjs`：向模板注入 API key

## 模型上下文系统本地模式验证（手动）

本节的 `context` 配置段与 Station 工具用于验证上下文重构与本地工具调用（多轮 agentic loop）。

### 1. 配置 nexus.json（`../workspace/agent-data/nexus.json`）

在 `providers.<provider>.models` 或 provider 默认上配置条数上限（溢出阈值），并增加 `context` 段（toolkit 白名单）：

```json
{
  "providers": {
    "deepseek": {
      "name": "deepseek",
      "provider_type": "openai",
      "base_url": "https://api.deepseek.com",
      "api_key": "sk-xxxx",
      "default_model_config": {
        "max_tokens": 4096,
        "max_tokens_usage": 128000,
        "timeout_secs": 60,
        "retry_count": 3
      },
      "models": {}
    }
  },
  "context": {
    "": {
      "default_context_config": {
        "channel_batch_interval_secs": 3,
        "memory_time_secs": 3600,
        "memory_count": 50,
        "compress_prompt": "请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。",
        "toolkits": ["filesystem"]
      },
      "roles": {}
    },
    "a1": {
      "default_context_config": {
        "channel_batch_interval_secs": 3,
        "memory_time_secs": 3600,
        "memory_count": 50,
        "compress_prompt": "请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。",
        "toolkits": ["filesystem"]
      },
      "roles": {}
    }
  },
  "default_model": { "provider": "deepseek", "model": "deepseek-chat" },
  "default_system_prompt": "你是 kissbot 智能助手"
}
```

说明：
- 本地工具由 `../workspace/agent-data/station.json` 声明：`toolkits.filesystem` 为内置 toolkit（声明即由内置注册表填充 `read` 工具元数据与实现，工具调用在进程内执行），`sub_stations` 为空
- `context` 段按 `agent → role` 两级配置（本例 role 覆盖为空），全局默认值为 `3s / 1h / 50条 / 默认压缩模板 / 空 toolkits`；agent 默认在 `default_context_config` 中声明（未声明的字段回落全局默认；`toolkits` 未声明时为空，示例显式声明 `["filesystem"]`）；`toolkits` 声明该 agent/role 启用的 toolkit（工具聚合只考虑这些 toolkit）
- `default_model_config` 承载 provider 默认模型参数（`max_tokens` / `max_tokens_usage` / `timeout_secs` / `retry_count` / `temperature` / `thinking` / `reasoning_effort`，未声明的数值字段回落全局默认 4096 / 60 / 3，`temperature`/`thinking`/`reasoning_effort` 未配时不发送（无全局默认）；`max_tokens_usage` 为必填项（旧配置缺该字段解析失败））；会话记录最近一次模型响应的 `usage.total_tokens`，下次请求开头超过 `max_tokens_usage` 的 80% 时触发重置（event 模式压缩、role 模式归档重建），无新消息不触发
- **迁移注意**：`context.toolkits` 缺省时为空（无工具）；`context` 段旧版 `stations` 键与旧 `nexus.json` 顶层 `stations` 段由 serde 忽略（兼容旧配置），需迁移为 `context.default_context_config.toolkits`（toolkit 名集合）。`context` 段旧版扁平 `default_*` 字段（如 `default_channel_batch_interval_secs`）已不再识别，需迁移为 `default_context_config.<字段>`（字段名去掉 `default_` 前缀，如 `default_context_config.channel_batch_interval_secs`），否则旧值静默回落全局默认（如 toolkit 不再启用）。`providers.<provider>` 旧版扁平 `default_*` 字段（如 `default_max_tokens`）同样已不再识别，需迁移为 `default_model_config.<字段>`（字段名去掉 `default_` 前缀，如 `default_model_config.max_tokens`），否则旧值静默回落全局默认（4096/60/3）；`context_length`、`max_context_messages` 已移除，`max_tokens_usage` 为必填项（旧配置缺该字段解析失败，需删除前两者并新增 `max_tokens_usage`，如 `"max_tokens_usage": 128000`）

### 2. 启动与验证步骤

```bash
# 1) 生成运行配置并注入 key
./reset-agent.sh
node inject-key.mjs

# 2) 启动依赖（先 memory 后 channel，再 agent）
./start-memory-store.sh
./start-memory-ego.sh
./start-channel-web.sh
./start-agent.sh
```

> 注意：`script/config.json` 的 `api.memory_store_url` / `memory_ego_url` 为空时 agent 会跳过记忆读写；需要记忆功能时在 `../workspace/config.json`（测试工作区）中配置 URL 并按其启动。

验证点：

1. **合批**：通过 web 通道连续发送多条消息 → agent 日志显示等待合批间隔（默认 3s）后打包为一条 user 消息进入 agentic loop；内容为 `user_name: 消息` 逐行
2. **多轮工具调用**：发送需要读文件的请求（如「读取 test.txt 的内容」，`test.txt` 需位于 agent 启动目录或其子目录内）→ 日志显示 LLM 返回 tool_call（模型需支持工具调用）→ 本地执行 `read` → 工具结果作为 tool 消息继续调用 LLM → 第二轮返回最终回复
3. **上下文缓存**：`../workspace/agent-data/context/` 下生成按 session_key 编码的 `.jsonl`，随对话逐条追加（含 user/assistant/tool 消息）
4. **历史归档与重置强制合并**：发送 `/reset`（或上下文超长触发）后，`../workspace/agent-data/context-history/` 出现 `<key编码>-<时间戳>.jsonl` 归档副本；event 模式超长时走压缩（system + user 压缩指令 + assistant 总结）；重置期间（上下文重建过程中）到达的新消息在重置完成后被强制合并进新上下文（不等待合批间隔）
5. **记忆打包（role 模式）**：role 模式会话重启/重置后，上下文首条为记忆打包的 user 消息——内容为最近 N 条（默认 50）与时间窗（默认 1h）并集、时间正序的 `user_name: 消息` 逐行（同时间组不拆散；不足 N 条返回全部）
6. **工具占位记录与 key 关联**：多轮工具调用后，channel 时间线出现 `ToolCall(key)`/`ToolResult(key)` 占位记录，且 `tool-call-records` / `tool-result-records` 文件中 ToolCallRequest 与 ToolResultRequest 的 key 相同
7. **路径安全**：让模型尝试读取 cwd 之外的路径（如 `../config.json` 的越界写法），`read` 工具返回路径越界错误，不实际读取

### 3. 清理

```bash
./reset-workspace.sh   # 重置数据目录
```
