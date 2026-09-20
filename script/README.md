# kissbot 手动测试脚本说明

本目录提供 kissbot 各组件的手动启动脚本（`start-*.sh`、`reset-*.sh`），配置模板在 `template/`。

- `reset-agent.sh`：按 `template/nexus.json`、`template/station.json` 生成 `../workspace/agent-data/` 下的运行配置（并注入 API key）
- `reset-workspace.sh`：重置工作区（由 `template/config.json` 生成 `../workspace/config.json`，并重建 `channel-data/`）
- `start-memory-store.sh` / `start-memory-ego.sh` / `start-channel-web.sh` / `start-agent.sh`：依次启动各组件
- `inject-key.mjs`：向模板注入 API key

## 模型上下文系统本地模式验证（手动）

本节的会话配置段与 Station 工具用于验证上下文管理、合批与本地工具调用（多轮 agentic loop）。

### 1. 配置 nexus.json（`../workspace/agent-data/nexus.json`）

运行配置由 `reset-agent.sh` 从 `template/nexus.json` 生成；直接改模板或改生成后的文件都可以。当前结构如下（省略 `station.json` 部分）：

```json
{
  "channels": {
    "web1": {
      "channel_id": "web1",
      "ws_url": "ws://127.0.0.1:8201",
      "admins": [{ "messenger_id": "web", "user_id": "admin" }],
      "bind_users": [{ "messenger_id": "web", "user_id": "u1" }],
      "agent_id": "0",
      "role_name": "",
      "enabled": true
    }
  },
  "providers": {
    "deepseek": {
      "provider_config": {
        "provider_type": "openai",
        "base_url": "https://api.deepseek.com",
        "api_key": "sk-xxxx"
      },
      "default_model_config": {
        "max_tokens_usage": 128000,
        "timeout_secs": 60,
        "retry_count": 3
      },
      "model_configs": {
        "deepseek-flash": { "max_tokens_usage": 131072, "timeout_secs": 30 }
      }
    }
  },
  "memory_structs": {},
  "agents": {
    "0": {
      "config_map": {
        "llm": {
          "provider": "deepseek",
          "model": "deepseek-flash",
          "max_tokens": 4096,
          "has_max_tokens": true,
          "temperature": 0.7,
          "has_temperature": true,
          "thinking": "enabled",
          "has_thinking": true,
          "reasoning_effort": "low",
          "has_reasoning_effort": true
        },
        "compress": { "compress_threshold": 0.8 },
        "memory_recover": { "memory_time_secs": 3600, "memory_count": 50 },
        "channel_batch": { "channel_batch_interval_secs": 3 },
        "out_channel": {
          "out_channel": {
            "channel_id": "web1",
            "user": { "messenger_id": "web", "user_id": "u1" },
            "group_id": "g1"
          }
        },
        "toolkit_set": { "toolkit_set": ["filesystem"] },
        "pipeline": {
          "input_processor": ["batch_input_processor"],
          "system_prompter": ["memory_ego_system_prompter"],
          "message_sender": "memory_recover_message_sender",
          "tool_caller": "station_tool_caller",
          "output_processor": "channel_output_processor"
        }
      },
      "roles": {}
    }
  },
  "sessions": []
}
```

说明：

- **channels**：`agent_id` 为绑定的 agent（空串/缺省归一化为保留 agent `"0"`），`bind_users` 为绑定的通道身份，`admins` 为可发管理命令的身份
- **providers**：`provider_config` 承载 `provider_type`（`openai` / `anthropic`）、`base_url`、`api_key`；`default_model_config` 为 provider 默认模型参数（`max_tokens_usage` 必填，`timeout_secs` / `retry_count` 缺省回落全局默认 60 / 3）；`model_configs` 按模型名覆盖（未配置的模型直接用 provider 默认值合成）
- **agents**：按 `agent_id` 配置，`config_map` 为 agent 级默认、`roles.<role_name>` 为 role 级覆盖（两级都缺省时回落内置默认）。可配项：
  - `llm`：`provider` / `model` / `max_tokens` / `temperature` / `thinking` / `reasoning_effort`；其中后四者带 `has_*` 标志——`set_*` 置 true（取值可为 null，用于显式置空），`unset_*` 置回 false（回落上一级），合并时只覆盖 `has_*` 为 true 的字段
  - `compress`：`compress_prompt`（默认压缩模板）/ `compress_threshold`（默认 0.8）
  - `memory_recover`：`memory_time_secs`（默认 3600）/ `memory_count`（默认 50）
  - `channel_batch`：`channel_batch_interval_secs`（合批间隔，默认 3）
  - `out_channel`：回复通道（`channel_id` + 绑定身份 + `group_id`）；也可用 `/bind-outgoing`、`/unbind-outgoing` 在运行期设置/清除；未配置 = 只存不回复
  - `toolkit_set`：会话启用的 toolkit 白名单（工具聚合只考虑这些 toolkit；空/缺省 = 无工具）
  - `pipeline`：会话执行流装配。可用组件名（写错会在启动会话时报 `PipelineNotFound`）：
    - `input_processor`（数组）：`batch_input_processor`
    - `system_prompter`（数组）：`default_system_prompter` / `memory_ego_system_prompter`
    - `message_sender`：`raw_message_sender` / `compress_message_sender` / `memory_recover_message_sender`
    - `tool_caller`：`station_tool_caller`
    - `output_processor`：`channel_output_processor`
    - 也可只写 `"preset": "preset_role"`（或 `preset_event`）由预设填充；显式字段会覆盖预设
- **sessions**：运行期按会话合成/覆盖的配置（key 为 `agent_id+role_name+mode` 三元组，JSON 以 `[[key, value], ...]` 数组对承载，一般无需手改）
- **memory_structs**：记忆结构组件配置（占位，本轮未实现）
- **station.json**（`toolkits.filesystem`）：由内置注册表填充 `read` 工具元数据与实现，工具调用在进程内执行；`sub_stations` 为空

**与旧版的差异（旧配置需迁移，否则相关字段被忽略/解析失败）**：

- `channels.<id>.agent_name` → `agent_id`（空串/缺省 = 保留 agent `"0"`）
- `agent_contexts` → `agents`；`agent_contexts.<id>.default_context_config` → `agents.<id>.config_map`，其 `out_channel` 字段值外包一层 `{ "out_channel": … }`；`toolkits` → `toolkit_set.toolkit_set`；`memory_time_secs` / `memory_count` → `memory_recover.<字段>`；`compress_prompt` / `compress_threshold` → `compress.<字段>`
- `providers.<name>` 旧的扁平 `provider_type` / `base_url` / `api_key` → 收进 `provider_config`；`default_model_config` 只保留 `max_tokens_usage` / `timeout_secs` / `retry_count`（`max_tokens` / `temperature` / `thinking` / `reasoning_effort` 移到 `agents.<id>.config_map.llm`）；`models` → `model_configs`
- 顶层 `default_model` → `agents.<由该默认适用的 agent>.config_map.llm`（`provider` + `model`）；顶层 `default_system_prompt` → 不再可配（系统提示词由 `system_prompter` 决定：固定默认提示词，或从 memory-ego 读取自我认知信息）
- 顶层 `context` → `agents`；`providers` 与 `agents` 必填，`sessions`（`[]`）/ `memory_structs`（`{}`）必填但可为空

### 2. 启动与验证步骤

```bash
# 1) 生成 workspace 运行配置、生成 agent 运行配置并注入 key
./reset-workspace.sh
./reset-agent.sh
node inject-key.mjs

# 2) 启动依赖（先 memory 后 channel，再 agent）
./start-memory-store.sh
./start-memory-ego.sh
./start-channel-web.sh
./start-agent.sh
```

> 注意：各启动脚本均以 `../workspace` 为 cwd 启动，读取 `../workspace/config.json`（由 `reset-workspace.sh` 从 `template/config.json` 生成，其中路径按 workspace 相对书写）。该文件中 `api.memory_store_url` / `memory_ego_url` 为空时 agent 会跳过记忆读写、系统提示词回退默认值，需要记忆功能时在其中填 URL。

验证点：

1. **合批**：通过 web 通道连续发送多条消息 → agent 日志显示等待合批间隔（默认 3s）后打包为一条 user 消息进入 pipeline；内容为 `user_name: 消息` 逐行
2. **多轮工具调用**：发送需要读文件的请求（如「读取 test.txt 的内容」，`test.txt` 需位于 agent 启动目录 `../workspace/` 或其子目录内）→ 日志显示 LLM 返回 tool_call（模型需支持工具调用）→ 本地执行 `read` → 工具结果作为 tool 消息继续调用 LLM → 第二轮返回最终回复
3. **上下文缓存**：`../workspace/agent-data/context/` 下生成按 session_key 编码的 `.jsonl`，随对话逐条追加（含 user/assistant/tool 消息，System 行在最前）
4. **上下文超长处理**：当上一次模型响应的 `usage.total_tokens` 超过 `max_tokens_usage × compress_threshold`（默认 0.8）时，下次请求前由消息发送器处理——`memory_recover_message_sender` 从 memory-store 回溯打包重建、`compress_message_sender` 用 LLM 总结压缩；旧上下文归档到 `../workspace/agent-data/context-history/<key编码>-<时间戳>.jsonl`
5. **记忆打包（memory_recover）**：触发回溯时，重建上下文的首条为记忆打包的 user 消息——内容为最近 N 条（默认 50）与时间窗（默认 1h）并集、时间正序的 `user_name: 消息` 逐行（同时间组不拆散；不足 N 条返回全部）
6. **工具占位记录与 key 关联**：多轮工具调用后，channel 时间线出现 `ToolCall(key)`/`ToolResult(key)` 占位记录，且 `tool-call-records` / `tool-result-records` 文件中 ToolCallRequest 与 ToolResultRequest 的 key 相同
7. **路径安全**：让模型尝试读取 cwd（`../workspace/`）之外的路径（如 `../test/out-of-cwd/sample.txt`，该目录由 `test/global-setup.ts` 生成），`read` 工具返回路径越界错误，不实际读取

### 3. 清理

```bash
./reset-workspace.sh   # 重置数据目录
```
