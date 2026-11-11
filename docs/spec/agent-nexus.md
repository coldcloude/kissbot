# kissbot-agent-nexus 技术规格

> 说明 kissbot-agent 的配置格式、管理命令、与外部组件的接口约定与关键数据结构。
> 模块调用关系见 [agent-modules.md](agent-modules.md)；station 侧约定见 [kissbot-agent-station](agent-station.md)。

## 配置文件格式

kissbot-agent 使用三份配置：

| 文件 | 内容 | 管理者 |
|------|------|--------|
| 公共配置（`KISSBOT_CONFIG`，默认 `./config.json`） | 各组件共享：`security` / `api` / `agent` / `memory` / 通道等段 | kissbot-config（只读，进程级单例） |
| `<data_dir>/nexus.json` | agent 可改配置：channels / providers / memory_structs / agents / sessions | ConfigManager（读写，落盘） |
| `<data_dir>/station.json` | station 可改配置：station_id / groups / sub_stations | ConfigManager（读写，落盘） |

`data_dir` 来自公共配置 `agent` 段。

### 公共配置（agent 相关段）

```jsonc
{
  "security": {
    "api_key": "user-key-456",        // 组件间互调（含 channel / station MCP）的 API key
    "admin_api_key": "admin-key-123"  // 管理 API 的 key
  },
  "api": {
    "memory_store_url": "http://127.0.0.1:8082",           // memory-store 地址（空 = 关闭读写）
    "memory_ego_url": "http://127.0.0.1:3001",             // memory-ego 地址（空 = ego 未配置）
    "memory_struct_summary_url": "http://127.0.0.1:8083"  // 摘要记忆地址（空 = 未部署，摘要记忆 pipeline 降级）
  },
  "agent": {
    "data_dir": "agent-data",                 // nexus.json / station.json / 会话上下文的根目录
    "mgmt_host": "127.0.0.1",
    "mgmt_port": 9090,                        // 管理 API 监听
    "station_host": "127.0.0.1",
    "station_port": 9100,                     // station MCP 端点监听（被其他 station 作为 sub 调用）
    "ws_reconnect_interval_secs": 5           // channel 断线重连间隔
  }
}
```

### nexus.json（NexusRepo）

```jsonc
{
  "channels": {
    "web1": {
      "channel_id": "web1",
      "ws_url": "ws://127.0.0.1:8201",
      "admins": [ { "messenger_id": "web", "user_id": "admin" } ],
      "bind_users": [ { "messenger_id": "web", "user_id": "u1" } ],
      "agent_id": "0",          // 绑定的 agent；缺省/空串归一化为保留 agent "0"
      "role_name": "",
      "enabled": true
    }
  },
  "providers": {
    "deepseek": {
      "provider_config": {
        "provider_type": "openai",              // "openai" | "anthropic"
        "base_url": "https://api.deepseek.com",
        "api_key": "sk-..."
      },
      "default_model_config": { "max_tokens_usage": 128000, "timeout_secs": 60, "retry_count": 3 },
      "model_configs": {
        "deepseek-flash": { "max_tokens_usage": 131072, "timeout_secs": 30 }
      }
    }
  },
  "memory_structs": {},
  "agent_roles": {
    "0": {                       // key = agent_id（"0" = 保留 agent）
      "config_map": {            // agent 级默认
        "llm": { "provider": "deepseek", "model": "deepseek-flash", "max_tokens": 4096, "has_max_tokens": true,
                 "temperature": 0.7, "has_temperature": true,
                 "thinking": "enabled", "has_thinking": true,
                 "reasoning_effort": "low", "has_reasoning_effort": true },
        "compress": { "compress_prompt": "…", "compress_threshold": 0.8 },
        "memory_recover": { "memory_time_secs": 3600, "memory_count": 50 },
        "memory_pipeline": { "load_memory": true },
        "channel_batch": { "channel_batch_interval_secs": 3 },
        "out_channel": { "out_channel": { "channel_id": "web1",
                                          "user": { "messenger_id": "web", "user_id": "u1" },
                                          "group_id": "g1" } },
        "group_set": { "group_set": ["filesystem"] },
        "pipeline": { "pipeline_name": "default_memory" }
      },
      "roles": { "dev": { "llm": { "model": "deepseek-v4-pro" } } }
    }
  },
  "sessions": {
    "0-": {
      "llm": { "provider": "deepseek", "model": "deepseek-flash", "max_tokens": null,
               "temperature": null, "thinking": null, "reasoning_effort": null }
    }
  }
}
```

说明：

- `providers` 段的配置结构（`provider_config` / `default_model_config` / `model_configs`）与「provider 默认 ← model 覆盖」的合成由 kissbot-llm 定义，结构与合成规则见 [kissbot-llm 技术规格](llm.md)；agent 只负责其存储、落盘与 CRUD。
- `has_*` 标志区分「未配置」与「显式置空」：`set_*` 置 `has_* = true`（取值可为 null），`unset_*` 置回 false；合并时只覆盖 `has_*` 为 true 的字段。
- session 条目里未覆盖的配置项序列化为 `null`（`SessionConfigMap` 各字段均为 `Option`）。
- `pipeline` 段只有一个 `pipeline_name`，取值为 `default`（无记忆）、`default_memory`（记忆读写：角色模式按开关决定是否读取，事件模式只写不读）或 `memory_summary`（摘要记忆）；pipeline 身份只由 agent + role 决定，当前 event 由 pipeline 运行态维护。从 agent/role 配置读取，未配置时默认 `default_memory`。pipeline 选择不做 session 级覆盖。
- `memory_summary`：一个 (agent + role) 一条 pipeline，按 channel 分别攒批，触发时先 locate，再按 case 上下文重建各自 session；当前 event 只影响 locate 与记忆范围，case session 不随 event 改变。
- `sessions` 是普通 JSON 对象，key 为 session_id；表中只存 session 级值（pipeline 从 agent/role 导入的有效配置或 `/model` 等覆盖）。

### 配置继承与合成

配置结构（`ProviderModelConfig` / `LLMConfig` / `EffectiveModelConfig` / `EffectiveLLMConfig`）与继承合并的通用约定由 kissbot-llm / kissbot-api 提供；agent 侧把「agent/role 可继承配置」与「session 级值」分开存储，由 pipeline 建立 session 时组合一次两层配置：

| 配置 | 继承链 | 合成结果 |
|------|--------|----------|
| 模型参数 | 全局常量 ← provider 默认（`default_model_config`）← model 覆盖（`model_configs`） | `EffectiveModelConfig`（max_tokens_usage / timeout_secs / retry_count） |
| LLM 参数 | LLMConfig 默认 ← agent 级 ← role 级；session 覆盖优先 | `EffectiveLLMConfig`（provider / model / max_tokens / temperature / thinking / reasoning_effort） |
| 其它会话配置 | 默认值 ← agent 级 ← role 级（compress / memory_recover / memory_pipeline / channel_batch / out_channel / group_set）；session 覆盖优先 | 各自 Effective 形态 |
| pipeline 选择 | 默认值 ← agent 级 ← role 级（pipeline） | `PipelineConfig` |
| 模型参数兜底 | 模型未在 `model_configs` 配置时直接用 provider 默认值合成 | — |

查询入口分两级，`ConfigManager` 不做隐式继承：

- `agent_role_config::<C>(agent_id, role_name)` / `set_agent_role_config::<C>`：agent → role 的可继承配置（含 `PipelineConfig`）。
- `session_config::<C, E>(session_id) -> Arc<E>`：按 session_id 读；缺失时用 `C::default()` 的有效配置建项并缓存（不落盘）。
- `set_session_config::<C, E>(session_id, config)`：把 `C` 覆盖合并进 session 有效配置并落盘；缺失时以 `C::default()` 为基底。
- `set_session_config_if_not_exists::<E>(session_id, config)`：session 配置项已存在则保留，否则写入并落盘。

`default_memory` / `memory_summary` 建立 session 前用 `set_session_config_if_not_exists` 从 agent/role 导入配置（llm、compress、memory_recover、memory_pipeline、channel_batch、out_channel、group_set）；`default_memory` 同时导入 memory_recover 与 compress 两套降阈值配置，运行期切换 `load_memory` 后立即生效；`default` 不依赖 agent/role，只读 session 配置，send 遇到 `ModelProviderNotFound` 时记录 error 并结束本次 run。运行期只按 session_id 读 session 配置，不再做 agent/role 合成。`/model` 读取当前 session 有效配置、合并 provider/model 后写回并落盘；因此建立 session 后修改 agent/role 配置不会影响该 session，重建 session 或重启后才重新导入。`default_memory` 每次切换 event 读取/新建 session 时导入一次配置（已有 session 配置不覆盖）；`/model`、`/bind-outgoing`、`/load-memory` 只写当前 event 的 session，切换 event 后新 session 使用 agent/role 初始配置。另有 `ConfigManager::provider_model_config(provider, model)`。

`out_channel` 由 pipeline 命令写入 pipeline 自己的 session 配置（`default_memory` 写当前 event 的 session，`memory_summary` 写基准 session `memory-summary-{agent_id}-{role_name}-none`），运行期从同一 session 读取；agent/role 级 `out_channel` 只作为建立 session 时的初始值导入，之后以 session 级值为准。

#### `default_memory` 的记忆读取策略

- `memory_pipeline.load_memory` 是 session 级开关，agent/role 级值只作为建立 session 时的初始值，之后以 session 级值为准；默认 `true`。
- **事件模式**（当前 `RoleEvent.event_id` 有值）：忽略 `load_memory`，永远不读取记忆（不调用 memory-store 查询），只写上行/下行记忆；上下文超阈值时用压缩。
- **角色模式**（当前 `RoleEvent.event_id` 缺省）+ `load_memory=true`：保持记忆回溯（`MemoryRecoverTokenReducer`）。
- **角色模式 + `load_memory=false`**：上下文超阈值时用压缩；每批 run 前用当前完整 session + 本批新消息直接调用 LLM（不执行工具、不读取 memory-store、不经过 `run_once`）：首次用普通提示词，回答先 `trim` 再转大写并精确匹配 `YES`/`NO`；首次失败或非精确回答时写死用更强的重试提示词再调用一次（不循环），重试仍精确匹配。两次都失败或都不是精确 `YES`/`NO` 按同一件事处理。判断为不同时归档旧上下文、清空 session（空上下文），随后统一在批次末尾由 run 追加本批新消息；判断为相同时保持上下文正常追加。
- `/load-memory <on|off>` 修改当前 event session 的开关；事件模式下写入但不生效。
- 切换 event 由 `/mode` 完成：`default` / `default_memory` 释放当前 session 并读取目标 event 的 session，`memory_summary` 只修改命令来源 channel 的 event 映射。
- `default` / `default_memory` 的 pipeline 命令与批次消息进入同一条 mpsc 任务队列串行执行，命令切换 session 与批次使用 session 不会并发。

## 会话与上下文本地缓存（pipeline 内部）

| 项 | 约定 |
|----|------|
| session 持有 | session 由 pipeline 创建并持有：`default` / `default_memory` 内存中只保留当前 event 的一个 `Arc<Session>`，切换 event 时释放旧 session 并从本地缓存重新读取，`memory_summary` 按 case 持有；pipeline 丢弃时 session 随 Arc 释放 |
| session 配置 | `default_memory` / `memory_summary` 建立 session 前从 agent/role 导入；`default` 只用 session 配置；运行期都只按 session_id 读 |
| 默认 pipeline 的 session_id | `pipelines::pipeline_session_id()`：角色模式 `{agent_id}-{role}`，事件模式 `{agent_id}-{role}-{event}` |
| 摘要记忆 case 的 session_id | 摘要记忆 pipeline 自己拼：`memory-summary-{agent_id}-{role_name}-{case_name}`（case_name 的 `/` 编码为 `.`、`[`/`]` 替换为 `_`），pipeline 只保存 case → session；基准（无 case）session 为 `memory-summary-{agent_id}-{role_name}-none`，同时承担 pipeline 入口配置与入口阶段配置读取 |
| 上下文本体 | 交给 kissbot-llm 的 Session/SessionContext（内存消息 + 本地缓存 + 版本一体） |
| 恢复 | 建立 session 时分别回读 `<data_dir>/context/<session_id>.sys.jsonl`（系统提示词，可为缺省）与 `<data_dir>/context/<session_id>.msgs.jsonl`（消息，正序全量）；崩溃一致性清理（丢弃悬挂 tool_calls 尾部与开头孤立 Tool 行）在 kissbot-llm 内完成 |
| 版本 | 归档由 SessionContext 自动生成日期后缀（`%Y%m%d%H%M%S`，重名等一秒后重试），调用方无需传入；版本同时保存 `<data_dir>/context/versions/<session_id>-<后缀>.sys.jsonl`（直接移动原 sys 文件，不使用待定值；原 sys 不存在时不写 sys）与 `.msgs.jsonl`；需要复用旧 session 时经 `Session::context_swap_version_with_cache` 把版本换回当前缓存，后续变更只发生在当前缓存 |
| 清理 | PipelineManager 只保留仍在 channel 绑定集合中的 (agent, role)；session 不单独清理，随 pipeline 的 Arc 释放自然回收。`default` / `default_memory` 切换 event 时旧 session 随 Arc 释放（命令与批次串行，无在途 run），再次切回该 event 时从本地缓存重新读取 |

缓存文件、版本文件命名与系统提示词的待定/应用语义见 [kissbot-llm 技术规格](llm.md)。

pipeline 身份（`RoleKey` = agent_id + role_name）是 channel 定位与配置的键；`MemoryKey`（agent_id + role_event）是记忆范围；session 是 pipeline 内部的执行资源，三者相互独立。

## 管理命令格式

### 通用规则

- 管理命令以 `/` 为前缀，参数以空格分隔（`split_whitespace`，不 trim 前置空格）
- 发送者必须在来源 channel 的 `admins` 列表中，否则命令被忽略（不回复也不进入 pipeline）
- 命令执行结果回复到原通道（不走 out_channel）
- 命令与命令回复不写入记忆、不进入会话上下文

### 命令列表

| 命令 | 语法 | 归属 | 说明 |
|------|------|------|------|
| bind | `/bind messenger <messenger_id> <user_id>` | 默认层 | 绑定 channel 用户（bind_users 追加，幂等） |
| unbind | `/unbind messenger <messenger_id> <user_id>` | 默认层 | 解绑 channel 用户 |
| admin | `/admin <messenger_id> <user_id>` | 默认层 | 添加管理权限 |
| unadmin | `/unadmin <messenger_id> <user_id>` | 默认层 | 移除管理权限 |
| agent | `/agent [agent_id] [role]` | 默认层 | 设置 agent（缺省 = 保留 agent "0"）与角色（缺省 = 保留 role 空串）；经 ego 校验 |
| role | `/role [role_name]` | 默认层 | 切换角色（缺省 = 保留 role） |
| mode | `/mode event [event-id]` | default / default_memory / memory_summary | 进入事件模式（缺省生成新 event-id）；`default` / `default_memory` 切换当前 session，`memory_summary` 只修改来源 channel 的 event 映射 |
| mode | `/mode role` | default / default_memory / memory_summary | 回到角色模式（`memory_summary` 只修改来源 channel 的映射） |
| pipeline | `/pipeline <pipeline_name>` | 默认层 | 设置来源 channel 的 (agent, role) 级 pipeline（默认 role 不可设置）；名称变化的已装配 pipeline 失效；可选 default / default_memory / memory_summary |
| agent-pipeline | `/agent-pipeline <pipeline_name>` | 默认层 | 设置来源 channel 的 agent 级 pipeline（默认 agent "0" 不可设置）；pipeline 名称变化的已装配 pipeline 失效 |
| bind-outgoing | `/bind-outgoing <messenger_id> <user_id> <group_id>` | default_memory / memory_summary | 设 pipeline 的回复通道（要求该身份已绑定来源 channel）；`default_memory` 写当前 event session |
| unbind-outgoing | `/unbind-outgoing` | default_memory / memory_summary | 清除 pipeline 的回复通道（回到只存不回复） |
| load-memory | `/load-memory <on\|off>` | default_memory | 设置当前 event session 是否读取记忆（事件模式固定不读取，配置写入但不生效） |
| model | `/model <provider> <model>` | default / default_memory / memory_summary | 切换当前 pipeline 的模型（先拉模型列表校验，失败保持原模型）；`default` / `default_memory` 写当前 event session，`memory_summary` 写基准与已有 case session |

命令分两层检测：默认层（`ChannelManager::execute_default_command`）处理上表的默认命令（含 `/pipeline`、`/agent-pipeline`，先于 pipeline 命令检测）；默认层未命中时，ChannelManager 按来源 channel 的 agent + role 定位 pipeline，并把来源通道三元组（messenger + 接收方用户 + 群组）连同命令交给 `AgentPipeline::execute_command` 识别执行，pipeline 不认识则回复 `⚠️ 未知命令: {命令}`。agent/role 变更与 bind/unbind 都经 channel 变更队列串行执行（写-写无竞态）；`/model`、`/mode`、outgoing 与 `/load-memory` 命令由 pipeline 处理，共享实现（解析、模型校验、事件切换、session 配置写入）在 `pipelines.rs`。命令回复文案形如：`✅ 已设置 agent: {agent_id} / role: {role}`、`✅ 已切换模型为: {provider}/{model}`、`✅ 新事件 ID: {event_id}`。

### 命令处理结果

| 结果 | 表现 |
|------|------|
| 成功 | `✅ …`（各命令自带描述） |
| 无权限 | 忽略，不回复 |
| 参数错误 | `⚠️ {提示}`（InvalidCommand） |
| 执行失败 | `❌ 命令执行失败: {错误描述}` |

## WS 通信协议

agent 通过 `kissbot-channel-client` 连接消息通道（kai-ws 客户端，JSON 消息 + 二进制心跳）：

1. `ChannelClient::connect(url, api_key)` 建立连接（header 携带 `X-Api-Key`）
2. 对 `bind_users` 中每个身份调用 `ChannelClient::bind(BindRequest { messenger_id, user_id })`（TYPE_BIND_AGENT_USER）
3. 心跳由 kai-ws 的 `WsHeartbeatHandler` 负责（二进制帧）
4. 上行：TYPE_INCOMING_MESSAGE → `Terminal::incoming_message`；群组/用户事件 → `join_group` / `leave_group` / `user_removed`（当前 no-op）；连接关闭 → `closed`（触发重连）
5. 下行：`ChannelClient::send_message(OutgoingMessage)`（TYPE_OUTGOING_MESSAGE），附件经上传分片 / 下载请求接口

断线后等待 `ws_reconnect_interval_secs` 重连，重连成功后重新绑定 `bind_users`（每次重连实时读取配置，`/bind` 后重连即生效）。

## Memory-Store API 交互

### 写入（MemoryStoreClient，经 appender 批量投递）

| 方法 | 路径 | 请求体 |
|------|------|--------|
| POST | `/store/channel` | ChannelRequest（数组；思考/工具内容内嵌在 `content` 里） |

请求结构体定义在 kissbot-api；`base_url` 为空时写入直接跳过（Ok），鉴权用 `X-Api-Key`。

### 读取（MemoryStoreClient）

| 方法 | 路径 | 请求体 |
|------|------|--------|
| POST | `/store/query/channel/recent` | RecentQuery — 按 (agent_id, role_event) 取最近 count 条（跨日期文件） |
| POST | `/store/query/channel` | QueryRequest — 按 (agent_id, role_event) + 时间范围查询（所有 channel 记录同文件，无需组合枚举） |

`build_context_from_memory_store` 用两次查询并集：① recent 取最近 N 条，`T_N` = 最旧一条时间；② 时间范围 `[M, T_N]`（`M = min(时间窗起点, T_N)`）取该段全部；结果按 (time, sn) 去重后时间正序，打包为交替的 User/Assistant 消息（结尾为 User 时补空 Assistant）。读路径在 `base_url` 为空时报错（不静默跳过）。

记忆回溯（`MemoryRecoverTokenReducer`）只读取当前会话中「最后一条 assistant（含）及之后」的消息（这部分可能还没形成通道记录，如工具轮次；也可能与打包结果末条重复），与上面的打包结果拼接后一次重置上下文；拼接前若打包结果末条与保留段首条同为 assistant，去掉打包结果末条，避免连续两条 assistant。

## Memory-Struct-Summary API 交互

摘要记忆 pipeline（`memory_summary`，经 `pipeline.pipeline_name` 显式启用）经 Nexus 持有的 `MemorySummaryClient` 调用摘要记忆，鉴权用 `X-Api-Key`。它自己实现 agentic loop（复用 pipelines.rs 的发送/工具/提示词函数），与 `default_memory` 的行为差异都在本 pipeline 内：

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/summary/locate` | 定位一批上行消息涉及的 case（请求含 agent_id、role_event 与带通道三元组与 msg_id 的消息） |
| POST | `/summary/case/summary` | 取 case 摘要（节点本身：摘要文本 + 层级 + 记录数 + 起止时间） |
| POST | `/summary/case/records-tail` | 取 case 结尾 count 条原文副本（升序） |

- 请求里的 `role_event` 与写记忆用同一个 `RoleEvent`（项目级基础类型）：`event_id` 缺省为角色模式，给出事件标识为事件模式。pipeline 维护 channel（messenger + 接收方用户 + 群组）→ 当前 event 的映射，`/mode` 只修改命令来源 channel 的映射，批次按自身 channel 的映射合成 `MemoryKey`；case 树与 case session 仍只由 role / case 决定。拼接记忆后缀是 memory 内部的事（只在构造文件名时进行），agent 不再自己拼字符串。后缀决定原始记录读取、水位、当前 case 指针与模型会话，角色名（`role_name`）决定 case 树；因此同一角色的角色模式与事件模式共用一棵 case 树（不同角色之间相互隔离）。
- 触发策略：一个 pipeline 为每个 channel（messenger + 接收方用户 + 群组）维护一个 `BatchProducer`（DashMap 按 `ChannelKey` 索引），各 channel 有独立的合批队列与消费者，互不干扰；最后一条消息超过 `channel_batch.channel_batch_interval_secs` 即触发该通道批次。入口阶段尚未 locate，合批配置从基准（无 case）session `memory-summary-{agent_id}-{role_name}-none` 读取。
- 批次处理与 case 执行分两级队列：各 channel 消费者把批次投递到批次队列，由唯一 worker 串行执行 `/summary/locate` 与本批上行记录写入（同一记忆范围的写入只能串行），随后按消息所属 case 并入该 case 的待执行任务并投递 case 名（同一 case 至多一个待执行任务，后续批次的消息直接在任务里合并），随即返回，下一批可以接着处理；case 队列的 worker 按到达顺序一次只取一个 case，取走该 case 的全部待执行消息后执行 run，完成后取下一个。批次进入时捕获当次 `MemoryKey` 并随待执行任务保存，case run 的摘要查询、结尾原文查询与下行记录都使用该快照。
- 每个 case 维护独立 session（懒创建）：`memory-summary-{agent_id}-{role_name}-{case_name}`；无 case 批次使用 `memory-summary-{agent_id}-{role_name}-none`。切换 event 不切换 case session。每个 case 一次 run：run 开头用「该 case 的摘要 + 该 case 结尾 `memory_recover.memory_count` 条通道原文副本」重置该 case 的 session 上下文，再把本次新增消息作为一条 user 消息运行 agentic loop。原文副本按摘要记忆返回的顺序组装，不做时间窗过滤、去重与重排；摘要为空时不加摘要块。同一 case 在排队等待执行期间又来的消息并入同一次 run；已经开始执行的 run 不受影响，其后的消息形成下一轮 run；还没轮到的 case 的待执行任务不受其他 case 执行的影响，继续与后续批次的消息合并。
- 上行记忆写入由 pipeline 自己负责（Nexus 不再统一写入）：`default_memory` 在 `incoming_message` 收到消息时立即写入（meta 为 None）；`memory_summary` 在 **locate 之后**写入，并带 `meta.memory_summary.case_name`（定位结果里该消息所属的 case），摘要记忆组件下次归档时直接按 case 名落位，不再走分组/逐层判定。
- 模型回复由 pipeline 调用 `pipelines.rs` 的自由函数 `send_outgoing` 发送并写下行记录；`memory_summary` 带本次 run 所属 case 的 meta 与批次捕获的 `MemoryKey`，`default_memory` 带 None。
- pipeline 命令由各 pipeline 的 `AgentPipeline::execute_command`（带来源 channel 三元组）识别执行：`default` 接 `/model`、`/mode`；`default_memory` / `memory_summary` 接 `/model`、`/mode` 与 outgoing 命令，`default_memory` 另接 `/load-memory`。共享实现（`command_parts` / `parse_mode_command` / `handle_mode_command` / `parse_model_command` / `handle_model_command` / `switch_model` / `bind_outgoing` / `unbind_outgoing` / `outgoing_channel`）在 pipelines.rs，各 pipeline 先按命令名 match 再调用。
- `/pipeline`、`/agent-pipeline`：修改 (agent, role) 或 (agent, "") 的 `pipeline_name`。修改前保存受影响 role 当前 merge 后的配置，修改后重新 merge；pipeline 名称变化的已装配 pipeline 从 PipelineManager 移除（下次按新配置装配）。默认 role / 默认 agent "0" 不可设置。
- `/model`：`default` / `default_memory` 写当前 event session 的 `LLMConfig`；摘要记忆 pipeline 写基准 session 与已有 case session，并保存模型覆盖供新建 case session 应用（进程重启后从基准 session 的已存配置恢复）。
- outgoing：`/bind-outgoing`、`/unbind-outgoing` 写 pipeline 自己的 session `OutChannelConfig`（`default_memory` 写当前 event session，摘要记忆 pipeline 写基准 session）；运行期 `run_once` 从同一 session 读取发送。
- channel 不再绑定本 pipeline（`/agent`、`/role` 切换）时，channel 侧对旧 pipeline 调用 `AgentPipeline::channel_unbound(messenger_id, self_user_id)`：摘要记忆 pipeline 丢弃该 channel 全部 group 的生产者与待处理消息；channel 侧 `apply_channel_key` 内联完成「解绑旧 channel + 清理无绑定 pipeline/session + 装配新 pipeline」。
- `memory_struct_summary_url` 为空或 locate 调用失败：记 warn，整批作为一个无 case 任务运行（不重建上下文，使用无 case session），本批上行记录不带 meta。
- 上下文重建（case 摘要/原文查询）失败：记 warn，清空该 case session 的上下文后只用本次新增消息运行该 case（不沿用其他 case 的上下文）。

## Memory-Ego API 交互

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/agent/get` | 获取 agent 元数据（按 agent_id）；`data` 为 null 表示不存在 |
| POST | `/individual/get-all` | 获取个体识别信息（按 agent_id） |
| POST | `/role/get` | 获取角色设定（agent_id + role_name） |

鉴权同样用 `X-Api-Key`（`security.api_key`）。ego 未配置 / 请求失败返回 Err，调用方按"失败静默跳过"处理。

### agent_id 绑定与解析

- channel 绑定直接使用 **agent_id**（配置字段 `ChannelConfig.agent_id`），pipeline 身份、memory-store 目录与 ego 查询均直接用该值，无需二次解析
- **保留 agent**：`agent_id == "0"` 表示保留 agent（不调 memory-ego，系统提示词回退默认值）
- **校验**：`/agent` 设置前经 ego `/agent/get` 校验存在（`verify_agent_exists`），role 非空时另经 `/role/get` 校验（`verify_role_exists`）；失败保持原值并报错

## LLM API 适配

provider 实现（OpenAI 兼容 / Anthropic）的构造（`create_*_provider`）、调用、重试与响应归一化由 kissbot-llm 提供；agent 侧的 ProviderManager 负责按 provider 名缓存实现实例（工厂去重）、解析有效模型配置（超时 / 重试 / token 上限）并按 `provider_type` 分派。

| provider_type | 兼容 API | 请求路径 | 模型列表 |
|---------------|----------|----------|----------|
| "openai" | OpenAI Chat Completions | `POST {base_url}/chat/completions` | `GET {base_url}/models` |
| "anthropic" | Anthropic Messages | `POST {base_url}/v1/messages` | `GET {base_url}/v1/models` |

请求参数（`EffectiveLLMConfig`）、响应归一化（`ModelResponse`）与重试语义见 [kissbot-llm 技术规格](llm.md)；未知 `provider_type` 由 ProviderManager 在分派处返回 `ModelProviderNotSupported`（不重试）。

## 数据结构定义

```rust
// 上下文消息、工具调用与模型响应结构（Message / ToolData / ToolCall / ModelResponse / ToolConfig）
// 由 kissbot-llm 定义（与 MCP 工具的互转在 agent 侧），见 [kissbot-llm 技术规格](llm.md)

// pipeline 身份：agent_id + role_name（kissbot-api 的 RoleKey，所有绑定 channel 的信息按此去重）
// 内部字段用 ArcStr，结构体实现 PartialEq / Hash，因此 Arc<RoleKey> 可直接作 Set / Map 键
struct RoleKey { agent_id: ArcStr, role_name: ArcStr }

// 记忆范围：agent_id + RoleEvent（role + 可选 event），用于 memory-store / memory-summary 与 session_id 推导
struct MemoryKey { agent_id: ArcStr, role_event: Arc<RoleEvent> }

// 当前 event 由 pipeline 在内存中维护（None = 角色模式），新建 pipeline 时为 None，不写配置

// session id（缓存/版本文件名段）由 pipelines::pipeline_session_id() 统一生成：
//   default / default_memory: {agent_id}-{role}[-{event}]
//   memory_summary: memory-summary-{agent_id}-{role_name}-{case_name|none}

// session 由 pipeline 创建并持有（Arc<Session>），id 由 pipeline 生成；
// default / default_memory 内存中只保留当前 event 的一个 session，切换 event 时释放旧 session 并从缓存重新读取；
// 建立时从 <data_dir>/context/<session_id>.sys.jsonl 与 .msgs.jsonl 恢复，并按 agent/role 固化 session 配置；
// 归档同时写 <data_dir>/context/versions/<session_id>-<后缀>.sys.jsonl 与 .msgs.jsonl，可与当前缓存互换

// 配置分两级：agent/role 可继承配置与 session 级值分开存储
impl ConfigManager {
    async fn agent_role_config<C>(&self, agent_id: &str, role_name: &str) -> Option<C>;
    async fn set_agent_role_config<C>(&self, agent_id: ArcStr, role_name: ArcStr, config: Arc<C>) -> Result<()>;
    async fn session_config<C, E>(&self, session_id: ArcStr) -> Arc<E>;
    async fn set_session_config<C, E>(&self, session_id: ArcStr, config: &C) -> Result<()>;
    async fn set_session_config_if_not_exists<E>(&self, session_id: ArcStr, config: Arc<E>) -> Result<()>;
}
```

## 错误类型

模型调用相关错误由 kissbot-llm 定义（`ModelApiError` / `ModelProviderNotFound` / `ModelProviderNotSupported` / IO 与解析类），agent 在自己的错误类型上逐变体映射，保持本枚举的对外错误表面不变。

```rust
enum Error {
    ConfigNotFound(ArcStr),
    ModelApiError(ArcStr),
    ModelProviderNotFound(ArcStr, ArcStr),
    ModelProviderNotSupported(ArcStr),
    MemoryStoreError(ArcStr),
    MemoryEgoError(ArcStr),          // 预留
    MemorySummaryError(ArcStr),           // 摘要记忆客户端（未配置 / 请求 / 解析 / 业务失败）
    InvalidCommand(ArcStr),
    IoError(std::io::Error),
    ReqwestError(reqwest::Error),
    JsonError(serde_json::Error),
    FileError(kai_file::Error),
    WsError(kai_ws::Error),
    McpError(kissbot_mcp::McpError),
    InternalError(ArcStr),           // 预留
    PipelineNotFound(ArcStr, Arc<RoleKey>),  // (配置名, pipeline 角色键)
}
```

## 管理 API（HttpServer）

认证：`x-api-key` 必须等于 `security.admin_api_key`（AuthLayer 拦截，缺失/错误返回 401）。

| 方法 | 路径 | 请求体 | 说明 |
|------|------|--------|------|
| GET | `/config` | — | 返回 NexusRepo 快照（channels / providers / memory_structs / agents / sessions） |
| POST | `/config/providers` | `{ "name": "...", "config": ProviderModelConfig }` | 新增 provider（重名失败） |
| POST | `/config/providers/remove` | `{ "name": "..." }` | 删除 provider |
| POST | `/config/channels` | `ChannelConfig` | 新增 channel |
| POST | `/config/channels/remove` | `{ "channel_id": "..." }` | 删除 channel |
| POST | `/config/admins` | `{ "channel_id", "messenger_id", "user_id" }` | 添加管理权限 |
| POST | `/config/admins/remove` | `{ "channel_id", "messenger_id", "user_id" }` | 移除管理权限 |

响应统一为 `{ "success": bool, "data": …, "error": … }`。
