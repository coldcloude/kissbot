# kissbot-llm 技术规格

> 说明 kissbot-llm 的公共 API、配置结构、与模型服务商的接口约定与会话上下文持久化格式。
> 组件设计与职责边界见 [kissbot-llm 组件设计](../design/components-design/kissbot-llm.md)。

## 位置与依赖

代码位置：`kissbot-llm/`。

| 依赖 | 用途 |
|------|------|
| kissbot-api | 配置继承 merge 约定（`MergeBy` / `MergeSelf` / `MergeEffectiveConfig`）、`ArcSwapHashMap` |
| kai-file | 会话缓存回读（`LineReader`） |
| tokio / serde / serde_json / reqwest / chrono / thiserror / async-trait | 异步运行时、序列化、HTTP、时间戳、错误、异步 trait |

不依赖 kissbot-mcp / kissbot-agent / kissbot-channel / kissbot-memory。

## 公共 API

```rust
// ===== provider =====
pub trait Provider: Send + Sync {
    type Request;
    fn build_request(&self, llm_cfg: &EffectiveLLMConfig, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Self::Request;
    async fn send_request(&self, request: &Self::Request, timeout: u64) -> Result<Value>;
    fn parse_response(&self, data: Value) -> ModelResponse;
    async fn list_models(&self) -> Result<Vec<String>>;
}

/// 按 provider 配置构造实现（调用方按 provider_type 分派）
pub fn create_openai_provider(provider_config: &ProviderConfig) -> OpenAiProvider;
pub fn create_anthropic_provider(provider_config: &ProviderConfig) -> AnthropicProvider;

/// 带重试调用：retry_count / timeout_secs 取自有效模型配置；永久性错误不重试
pub async fn call_with_retry<P: Provider>(
    provider: &P,
    llm_cfg: &EffectiveLLMConfig,
    pm_cfg: &EffectiveModelConfig,
    messages: Vec<Message>,
    tools: &Vec<Arc<ToolConfig>>,
) -> Result<ModelResponse>;

// ===== session =====
pub struct SessionContext { /* 私有 */ }
impl SessionContext {
    pub fn new(data_dir: &str, session_id: &str) -> Self;
    pub fn set_system_message(&mut self, content: String);
    pub async fn apply_pending_system(&mut self) -> Result<()>;
    pub async fn append(&mut self, messages: Vec<Message>) -> Result<()>;
    pub async fn recover_from_cache(&mut self) -> Result<()>;
    pub async fn archive_and_clear_cache_and_reset_messages(&mut self, messages: Option<Vec<Message>>) -> Result<()>;
    pub fn build(&self) -> Vec<Message>;
}

pub struct Session { pub context: Mutex<SessionContext> }
impl Session {
    pub fn new(data_dir: &str, session_id: &str) -> Self;
    pub async fn recover_from_cache(&self) -> Result<()>;
    pub async fn build_context(&self) -> Vec<Message>;
    pub async fn context_append(&self, messages: Vec<Message>);
    pub async fn context_archive_and_clear_cache_and_reset_messages(&self, new_messages: Vec<Message>);
    pub async fn set_system_message(&self, content: String);
}
```

约定：

- `call_with_retry` 的失败重试为指数退避（第 n 次失败后等待 `2^n` 秒），`retry_count = 0` 表示不重试。
- 未知 `provider_type` 由调用方分派时判定；组件内不会出现该分支。
- `create_*_provider` 每次返回新实例（含独立 HTTP 客户端），组件不缓存、不共享客户端；按名复用（工厂去重）由调用方完成（agent 中为 ProviderManager 的 `AsyncCacheFactory`）。
- `session_id` 由调用方编码，须为文件名安全字符串；组件不解释其含义。

## 配置结构

```jsonc
// provider 配置：连接信息 + 默认模型参数 + 单模型覆盖
{
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

// LLM 配置（可继承形态）：provider + model + 生成参数
{
  "provider": "deepseek",
  "model": "deepseek-flash",
  "max_tokens": 4096, "has_max_tokens": true,
  "temperature": 0.7, "has_temperature": true,
  "thinking": "enabled", "has_thinking": true,
  "reasoning_effort": "low", "has_reasoning_effort": true
}
```

| 合成 | 继承链 | 结果 |
|------|--------|------|
| 模型参数 | 全局常量（timeout 60s / retry 3）← provider 默认 ← model 覆盖 | `EffectiveModelConfig`（provider_config / max_tokens_usage / timeout_secs / retry_count） |
| LLM 参数 | 默认 ← 各级覆盖（调用方决定层数）← session 覆盖 | `EffectiveLLMConfig`（provider / model / max_tokens / temperature / thinking / reasoning_effort） |

- `has_*` 标志区分「未配置」与「显式置空」：`set_*` 置 `has_* = true`（取值可为 null），`unset_*` 置回 false；合并时只覆盖 `has_*` 为 true 的字段。
- 模型未在 `model_configs` 配置时用 provider 默认值合成。
- 配置的存储、落盘与 agent → role → session 三级继承由调用方实现（agent 中见 [kissbot-agent-nexus 技术规格](kissbot-agent-nexus.md)）。

## LLM API 适配

### 支持的提供商

| provider_type | 兼容 API | 请求路径 | 模型列表 |
|---------------|----------|----------|----------|
| "openai" | OpenAI Chat Completions | `POST {base_url}/chat/completions` | `GET {base_url}/models` |
| "anthropic" | Anthropic Messages | `POST {base_url}/v1/messages` | `GET {base_url}/v1/models` |

### 请求参数（EffectiveLLMConfig）

```jsonc
{
  "model": "deepseek-flash",
  "messages": [ { "role": "system", "content": "…" }, { "role": "user", "content": "…" } ],
  "max_tokens": 4096,          // 未配置时 anthropic 回退 65535
  "temperature": 0.7,
  "thinking": { "type": "enabled" },   // openai：映射为协议枚举；anthropic：原样 { "type": … }
  "reasoning_effort": "low",           // openai：原样字段；anthropic：output_config.effort
  "stream": false                      // 固定非流式
}
```

- messages 直接使用会话上下文的 `Message` 序列（OpenAI 兼容格式）；anthropic 分支把 system 提取为顶层 `system` 字段、丢弃 Tool 消息（本轮不支持工具消息）
- tools 非空时按 provider 协议包装（openai：`{type:"function", function:{name,description,parameters}}`）；为空则不携带该字段
- openai 的 `thinking` 仅识别 `enabled` / `disabled`，其它取值不发送该字段

### 响应归一化（ModelResponse）

| 字段 | 说明 |
|------|------|
| raw_content / content | API 原始 content 与剥离 `<think>…</think>` 后的正文 |
| raw_reasoning_content / reasoning_content | API 思考字段原值 / 解析值（openai `reasoning_content`；anthropic thinking block） |
| thinking | 正文中 `<think>` 标签内容（与 reasoning_content 相互独立） |
| raw_tool_calls / tool_calls | API 原始 tool_calls（原样回传上下文）/ 解析出的 `ToolCall` 列表 |
| finish_reason | 停止原因（当前无消费方） |
| total_tokens | openai：`usage.total_tokens`（缺省 0）；anthropic：固定 0 |

调用失败按 `retry_count` 指数退避重试；未知 `provider_type` 由调用方在分派处返回错误（不重试）。

## 会话上下文缓存与归档

| 项 | 约定 |
|----|------|
| 缓存文件 | `<data_dir>/context/<session_id>.jsonl`，每行一条 Message（JSON），追加写、不截断 |
| 行序 | 首行为当前系统消息（如有），其后为消息行 |
| 恢复 | 正序全量回读；崩溃一致性清理——丢弃末尾悬挂的 assistant tool_calls 行与开头的孤立 Tool 行；文件不存在视为空上下文 |
| 归档 | `<data_dir>/context-history/<session_id>-<时间戳>.jsonl`（时间戳 `%Y-%m-%d-%H%M%S`），格式与缓存一致 |

- 系统消息的 set 只写入待定值（多次 set 保留最近一次）；发送前对比应用：与当前一致仅清空待定，不一致则先归档并清空缓存再以新系统消息重建。
- 归档与清空缓存文件配对执行；内存为空时不产生归档文件。
- agent 的 session_id 编码（`<agent_id>-<role_name>[-<event>]`）与 SessionManager 的去重职责见 [kissbot-agent-nexus 技术规格](kissbot-agent-nexus.md)。

## 数据结构定义

```rust
// 上下文消息（OpenAI 兼容，序列化时 role 为平级标签）
enum Message {
    System { content: Arc<String> },
    User { content: Arc<String> },
    Assistant { content: Value, reasoning_content: Value, tool_calls: Value },  // 后三者原样透传 API 值
    Tool { tool_call_id: Arc<String>, content: Arc<String> },                    // 无 name 字段
}

// 工具调用：调用方交互层结构（LLM 侧 tool call 与工具结果回填）；
// 工具宿主内部使用自己的工具类型，由调用方在边界转换
// arguments 执行前、result / error 执行后（三者均为 JSON）；
// 只承载 MCP content 字段转成的 Value（内容块数组）：成功进 result、失败进 error（result 留空）；
// structuredContent 仅用于工具宿主内部处理，不进入本结构
struct ToolData { arguments: Value, result: Value, error: Value }
struct ToolCall { id: Arc<String>, name: Arc<String>, data: ToolData }

// LLM 请求中的工具定义（OpenAI tools[].function 形状）；与 MCP 工具的互转在调用方
struct ToolConfig { name: Arc<String>, description: Arc<String>, parameters: Arc<Value> }

// 模型响应（归一化）
struct ModelResponse {
    raw_content: Value, content: Option<Arc<String>>, thinking: Option<Arc<String>>,
    raw_reasoning_content: Value, reasoning_content: Option<Arc<String>>,
    raw_tool_calls: Value, tool_calls: Option<Vec<ToolCall>>,
    finish_reason: Option<Arc<String>>, total_tokens: u64,
}
```

## 错误类型

```rust
enum Error {
    ModelApiError(String),                              // 服务商返回非成功状态或调用失败
    ModelProviderNotFound(String, String),              // (provider 名, model 名)
    ModelProviderNotSupported(String),                  // 未知 provider_type
    IoError(std::io::Error),
    ReqwestError(reqwest::Error),
    JsonError(serde_json::Error),
    FileError(kai_file::Error),
    InternalError(String),
}
pub type Result<T> = std::result::Result<T, Error>;
```

调用方在自己的错误类型上做逐变体映射（不共享同一枚举），以保持各自错误表面稳定。

## 验证

- 单元测试：`cd kissbot-llm && cargo test`（消息/工具结构序列化、两种协议的请求构造与响应归一化、重试与永久性错误、provider/metric 参数合成、上下文缓存与归档）。
- 使用方 kissbot-agent：`cd kissbot-agent && cargo test`（ProviderManager 的工厂去重与 provider_type 分派、有效配置解析、会话集合去重与清理，配置模板 schema 校验）。
