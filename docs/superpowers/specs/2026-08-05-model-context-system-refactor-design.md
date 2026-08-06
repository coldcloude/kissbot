# 模型上下文系统重构设计

## 目标

彻底重构模型上下文系统，覆盖五个方面：

1. 上下文内存表示改为 OpenAI API 格式——role 为枚举变体（type），数据字段与 role 同级
2. 支持多轮对话——system → user → assistant(tool_calls) → tool → assistant(tool_calls) → tool → … → assistant(无 tool_calls) → user → … 的形式（参考 DeepSeek / Kimi thinking 模式文档）
3. Station 框架基本实现——StationConfig 增加 Tool 列表（Map），Tool 为 trait（统一参数/返回值），base_url 非空走 REST、为空走本地；本轮只实现并测试本地模式
4. 上下文与记忆格式拆分——上下文在 agent-data 下有本地缓存，按 session_key 存储（直接上下文格式），存储不截断、读取全量（缓存天然有界）；上下文来源为 channel 合批 + 记忆打包
5. 上下文重置流程——event 模式缓存恢复/压缩，role 模式记忆重建；压缩/重置前当前上下文归档为历史（本轮只写不读）

## 1. 上下文表示

`Message` 枚举替换现有 `ContextMessage` 与 `MessageItem`，`Provider` 直接消费新枚举：

```rust
/// OpenAI 兼容上下文消息：role 即枚举变体，数据字段与 role 同级
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    System { content: String },
    User { content: String },
    Assistant {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,   // 工具调用场景必须随请求回传（DeepSeek 带 tools 请求须完整回传否则 400；Kimi 单轮工具循环保留并回传）；非工具调用可选（API 忽略）；openai_body 自动序列化
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
    },
    Tool {
        tool_call_id: String,   // 对应 assistant.tool_calls[].id
        name: String,           // 调用的工具名（内部元数据）
        content: String,        // 调用结果（JSON 字符串或文本）
    },
}

/// OpenAI function call：wire 为 {id, type:"function", function:{name, arguments(JSON 字符串)}}
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,  // 内部为解析后的对象；wire 时序列化为 JSON 字符串
}
```

要点：

- User 无独立 name 字段，说话人信息拼进 content（单条 `"user1: 你好"`；合批/打包多行 `"user1: 你好\nuser2: 在吗"`）
- Assistant 带 tool_calls 时 content 通常为空串；`reasoning_content` 工具调用场景必须随请求回传（DeepSeek 带 tools 请求须完整回传否则 400；Kimi 单轮工具循环保留并回传），非工具调用可选（API 忽略）；openai_body 自动序列化
- 多轮模式由消息序列天然保证，无需额外结构

## 2. 会话与配置

### Session 变化

`Session` 增加 `agent_name: Arc<String>` 字段（从 SessionKey 复制），用于查找 context 配置。

### NexusRepo 新增 context 配置段

结构为「全局默认 ← agent 配置 ← role 配置」三层继承，与 provider-model 的 default_*/Option 继承模式一致：

```rust
/// agent 级 context 配置（key = agent_name，覆盖全局默认）
pub struct AgentContextConfig {
    pub default_channel_batch_interval_secs: u64,
    pub default_memory_time_secs: u64,
    pub default_memory_count: usize,
    pub default_compress_prompt: String,
    pub default_stations: HashSet<String>,   // 启用的 station_id（Set 形式）
    pub roles: Arc<ArcSwapHashMap<String, RoleContextConfig>>,  // key = role_name
}

/// role 级 context 配置（可选覆盖 agent 默认值）
pub struct RoleContextConfig {
    pub channel_batch_interval_secs: Option<u64>,
    pub memory_time_secs: Option<u64>,
    pub memory_count: Option<usize>,
    pub compress_prompt: Option<String>,
    pub stations: Option<HashSet<String>>,
}
```

- `NexusRepo.context: Arc<ArcSwapHashMap<String, AgentContextConfig>>`（key = agent_name）
- 合并规则：role 配置 Some 值覆盖 agent 默认；无 role 配置用 agent 默认；无 agent 配置用内置全局默认
- 全局默认值：`channel_batch_interval_secs=3`、`memory_time_secs=3600(1h)`、`memory_count=50`、`compress_prompt=默认模板`、`stations=空集合`
- 保留 agent（`agent_name=""`）同样按此查找（配置缺失走全局默认）
- 会话查配置：`agent_name → AgentContextConfig → role_name → RoleContextConfig`，合并为运行时 `EffectiveContextConfig`

### 上下文条数上限

沿用 provider/model 配置，不放在 context 段：

- `ProviderConfig` 增加 `default_max_context_messages: usize`
- `ModelConfig` 增加 `max_context_messages: Option<usize>`（继承模式同其它参数）
- 合成进 `EffectiveModelConfig`；溢出判断用会话模型的 `effective.max_context_messages`
- 现有 token 语义的 `context_length` 保留不动

## 3. 上下文缓存与历史归档

### 缓存（当前上下文）

- 位置：`<data_dir>/context/<session_key编码>.jsonl`，每行一条 `Message`（JSON）
- 存储：`tokio::fs` 追加（OpenOptions append），**存储时不截断**——正常追加永不全量重写
- 读取：`ReverseLineReader` 从尾部读，event 恢复时**全部回读**（无需截断配置：缓存生命周期 = 当前上下文，超长即压缩，天然有界，见第 5 节）
- 重建（压缩/重置）时：先归档当前缓存到历史，再**清空重写**缓存文件为新上下文（否则新旧消息混在一起，尾部读取会读到压缩前内容）
- session_key 编码：`agent|role|mode` 字符串做文件名安全编码（如 base64url），避免路径/非法字符问题

### 历史归档（已结束的上下文，本轮只写不读）

- 位置：`<data_dir>/context-history/<session_key编码>-<时间戳>.jsonl`
- 归档 = **直接复制当前缓存文件**到历史目录并加上时间戳文件名（无需包装格式）
- 归档时机：event 压缩前、role 重置/重新进入前
- 本轮只写不读，为后续长期记忆/历史查询铺路

## 4. 上下文来源

### (1) Channel 合批

- 每个会话一个待合批缓冲：`Vec<(user_name, 文本)>` + debounce 计时器
- 普通消息到达（已过回显判定、已逐条写记忆）→ 加入缓冲 → 重置计时器（`channel_batch_interval_secs`，默认 3s）
- 超时无新消息 → 打包为**一条 User 消息**（content = 逐行 `"name: text"`，name 为空只留 text）→ 追加上下文 → 进 agentic loop
- 管理命令**不走缓冲**（立即处理）；回显判定/记忆写入仍逐条即时执行，合批只在入 loop 前
- 会话重置/销毁时清空缓冲

### (2) 记忆打包（仅 mode=role）

- 读两次 memory-store：
  1. 时间窗 `[now - memory_time_secs, now]` 全量
  2. `limit = memory_count` 的最近 N 条
- 比较两者**首条记录时间**（升序）：① 更早 → 结果 = ①（窗口更大，可分批读到末尾）；② 更早 → 结果 = ②
- 均为空 → 不打包（上下文仅 system）
- 打包为一条 User 消息：content = 逐行 `"name: text"`（取 channel record 的 user_name + 文本；本轮只读 channel 记录，即「最近消息」；无 name 记录跳过）
- **memory-store 查询 API 扩展**：`QueryChannelRequest`/`QueryRequest` 增加可选 `limit`（服务端返回该范围内最近 N 条）

## 5. 上下文重置 / 压缩流程

触发：新 session_key（会话创建/重新进入）、条数超上限（`effective.max_context_messages`）。重置时清空该会话合批缓冲。

| 场景 | 流程 |
|---|---|
| **event · 新 key / 重新进入** | 缓存全部回读恢复（空则为仅 system）→ 等 channel 消息继续 |
| **event · 超长**（压缩模式） | ① 当前缓存复制归档历史 → ② 用会话模型调 LLM（prompt = `compress_prompt` 模板 + 当前上下文）得总结 → ③ 缓存清空重写为 `system + user(压缩指令) + assistant(总结)` → ④ 等 channel 消息继续 |
| **role · 新 key / 重启 / 重新进入 / 超长** | ① 缓存有内容则复制归档历史 → ② 缓存清空 → ③ 记忆打包构造首条 user 消息 → ④ 等 channel 消息继续 |

启动流程（`ensure_session → build_initial_context`）改为按 mode 走对应重置分支（role 走记忆打包、event 走缓存恢复），ego/system 消息照旧在最前。

## 6. Station 框架

### 配置结构

```rust
pub struct StationConfig {
    pub station_id: Arc<String>,
    pub base_url: Arc<String>,   // 非空 = REST 调用；空 = 本地调用
    pub timeout_secs: u64,
    pub tools: Arc<ArcSwapHashMap<String, ToolConfig>>,  // key = 工具名
}

pub struct ToolConfig {
    pub name: String,            // 与 map key 一致
    pub description: String,     // 发给 LLM 的工具描述
    pub parameters: serde_json::Value,  // JSON Schema（OpenAI tools[].function.parameters）
}
```

### Tool trait（统一参数/返回值）

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    async fn call(&self, params: serde_json::Value) -> Result<serde_json::Value>;
}
```

### StationRuntime

```rust
pub struct StationRuntime {
    config: Arc<StationConfig>,
    local_tools: DashMap<String, Arc<dyn Tool>>,  // 本地工具实现表（base_url 为空时注册）
    http_client: reqwest::Client,
}
// call_tool(tool_name, params)：
//   base_url 为空 → 查 local_tools 本地执行（未注册报错）
//   base_url 非空 → POST {base_url}/tool/{tool_name}（本轮骨架，返回未实现错误，不测试）
```

- 内置示例工具 **Read**（读文本文件）：参数 `{path}`，返回文件内容，注册到 base_url 为空的本地 station
- Read 路径校验（安全）：参数 path 先基于 cwd 解析为绝对路径 → `canonicalize`（消解 `..` 与符号链接）→ 校验规范绝对路径等于 cwd 或在其子目录内（前缀判断在规范化后的绝对路径上进行，不直接检查原始输入，防穿透），越界拒绝；校验通过后读取内容返回，限制返回大小（如截断到前 64KB）防大文件
- 启动时对每个 StationConfig 构建一个 StationRuntime

### 给 LLM 的 tools 列表与路由

- 每次调用模型时，从**启用的 station**（会话 context 配置的 `stations` 集合 ∩ 实际配置的 station）的工具聚合：`tools: [{type:"function", function:{name, description, parameters}}]`
- 聚合为空则不发送 tools 字段（兼容无工具场景）
- 工具路由：LLM 返回 tool_call 后，按 tool name 在启用的 StationRuntime 中查找所属 station → `call_tool` 执行；找不到 → 生成错误 Tool 消息（如「工具不存在: xxx」）

## 7. Agentic Loop 多轮改造

```
run_agentic_loop(session, out_channel):
  loop (上限 MAX_TOOL_ROUNDS=10 防死循环):
    response = model.call(session.model, context.build(tools聚合), &tools聚合)
    if response.tool_calls 非空:
      上下文追加 Assistant{content:"", tool_calls}
      逐个执行：路由 → call_tool → 上下文追加 Tool{tool_call_id, name, content}
      记忆写入 push_tool_call / push_tool_result（现有 API 启用）
      继续下一轮
    else:
      上下文追加 Assistant{content, reasoning_content, tool_calls:None}
      think 写入记忆（现有流程保留）
      发送回复到 out_channel
      break
  缓存：每步追加后立即 append 到缓存文件
  检查溢出（>= effective.max_context_messages）→ 按第 5 节 mode 分支重置/压缩
```

## 8. Provider wire 格式

**OpenAI 兼容**（deepseek/kimi/openai）：

- 请求：`Message` → `{role, content}`；Assistant 带 tool_calls → `{role, content, tool_calls:[{id, type:"function", function:{name, arguments:JSON 字符串}}]}`；Tool → `{role:"tool", tool_call_id, content}`；`reasoning_content` 工具调用场景随请求回传（自动序列化），非工具调用省略；`tools` 数组按第 6 节聚合
- 响应：解析 `message.tool_calls[]` → `ToolCall{id, name, arguments(解析后)}`（`finish_reason="tool_calls"` 时走工具分支）

**Anthropic**：保持纯文本映射（content-only，tool_calls/工具消息本轮不支持，遇 tool_calls 记录日志）

## 9. 测试范围

- 单元测试：Message serde/wire 转换、缓存追加 + ReverseLineReader 全量读取、历史归档文件复制、合批打包、记忆两查询比较、压缩上下文构造、StationRuntime 本地调用（Read 真实文件 + mock tool）、Read 路径校验（越界拒绝）
- Agentic loop：用 provider 测试桩（返回固定 tool_call）验证多轮流程
- **REST 分支不实现不测试**（只测本地模式）
