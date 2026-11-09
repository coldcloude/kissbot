# kissbot-llm 技术规格

> 说明 kissbot-llm 的公共 API、配置结构、与模型服务商的接口约定与会话上下文持久化格式。
> 组件设计与职责边界见 [kissbot-llm 组件设计](../design/components-design/kissbot-llm.md)。

## 位置与依赖

代码位置：`kissbot-llm/`。

| 依赖 | 用途 |
|------|------|
| kissbot-api | 配置继承 merge 约定（`MergeBy` / `MergeSelf` / `MergeEffectiveConfig`） |
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
    async fn list_models(&self) -> Result<Vec<ArcStr>>;
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
    pub fn new(data_dir: &str, session_id: ArcStr) -> Self;
    /// 只保存待定值（不写文件、不会失败）
    pub fn set_system_message(&mut self, content: ArcStr);
    /// 构建完整上下文时应用待定值：需要变更则重写 sys 文件（不归档、不动消息）
    pub async fn apply_pending_system(&mut self) -> Result<()>;
    pub async fn append(&mut self, messages: Vec<Message>) -> Result<()>;
    pub async fn recover_from_cache(&mut self) -> Result<()>;
    /// 归档当前缓存为版本并清空缓存/内存（append 由 Session 封装）
    pub async fn archive_and_clear(&mut self) -> Result<()>;
    pub async fn swap_version_with_cache(&mut self, version_suffix: &str) -> Result<()>;
    pub fn build(&self) -> Vec<Message>;
}

pub struct Session { pub context: RwLock<SessionContext> }  // tokio::sync::RwLock（异步读写锁）
impl Session {
    pub fn new(data_dir: &str, session_id: ArcStr) -> Self;
    pub async fn recover_from_cache(&self) -> Result<()>;
    /// 只读构建：当前系统消息 + 内存消息（不应用待定、不写文件）
    pub async fn build_context(&self) -> Vec<Message>;
    /// 发送前入口：应用待定系统消息后构建（待定变更时重写 sys 文件）
    pub async fn build_context_full(&self) -> Vec<Message>;
    pub async fn context_append(&self, messages: Vec<Message>) -> Result<()>;
    /// 只读访问内存消息（不含系统消息）：闭包内只读，上下文不变
    pub async fn context_read<R>(&self, f: impl FnOnce(&Vec<Message>) -> R) -> R;
    pub async fn context_archive_and_clear_cache_and_reset_messages(
        &self, new_messages: Vec<Message>,
    ) -> Result<()>;
    pub async fn context_swap_version_with_cache(&self, version_suffix: &str) -> Result<()>;
    /// 只保存待定值（不写文件、不会失败）
    pub async fn set_system_message(&self, content: ArcStr);
}
```

约定：

- `Session.context` 用 tokio 的异步读写锁：`build_context` / `context_read` 取读锁（可并发），其余上下文方法取写锁。
- `build_context` 只读、不应用待定系统消息；`build_context_full` 是发送前入口，对比待定值与当前值，不一致时重写 sys 文件。
- `set_system_message` 只写待定值（同步、不会失败）；多次 set 保留最近一次。
- `apply_pending_system` / `build_context_full` 应用待定值时不归档、不改动消息文件。
- `archive_and_clear` 把旧缓存文件移动到版本并清空缓存/内存；`append` 仍由 SessionContext 提供，调用位置在 Session 的归档重建方法中。
- `Session::context_archive_and_clear_cache_and_reset_messages` 先调用 `archive_and_clear`，再把 `new_messages` 追加成当前上下文。
- `call_with_retry` 的失败重试为指数退避（第 n 次失败后等待 `2^n` 秒），`retry_count = 0` 表示不重试。
- 未知 `provider_type` 由调用方分派时判定；组件内不会出现该分支。
- `create_*_provider` 每次返回新实例（含独立 HTTP 客户端），组件不缓存、不共享客户端；按名复用（工厂去重）由调用方完成（agent 中为 ProviderManager 的 `AsyncCacheFactory`）。
- `session_id` 由调用方编码，须为文件名安全字符串；组件不解释其含义。
- `context_read` 只读暴露内存消息（不含系统消息）：不写缓存、不改变上下文。

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
- 配置的存储、落盘与 agent → role → session 的配置组合由调用方实现（agent 中 ConfigManager 负责两级存储，pipeline 在建立 session 时固化，见 [kissbot-agent-nexus 技术规格](agent-nexus.md)）。

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

## 会话上下文缓存与版本

`SessionContext::new(data_dir, session_id)` 的 `data_dir` 就是缓存所在目录（不再额外拼 `context`）：

| 项 | 约定 |
|----|------|
| 系统提示词文件 | `<data_dir>/<session_id>.sys.jsonl`，单行 System Message JSON；没有系统提示词时不创建该文件 |
| 消息文件 | `<data_dir>/<session_id>.msgs.jsonl`，每行一条 Message（JSON），追加写、不截断 |
| 恢复 | sys 与 msgs 分别回读；msgs 正序全量读取；崩溃一致性清理——丢弃末尾悬挂的 assistant tool_calls 行与开头的孤立 Tool 行；文件不存在视为对应内容为空 |
| 版本 | `<data_dir>/versions/<session_id>-<后缀>.sys.jsonl` 与 `<session_id>-<后缀>.msgs.jsonl`：当前缓存的两个文件直接移动成版本（不重新写入，sys 使用原先的系统提示词、不应用待定值；原 sys 不存在时版本也不写 sys） |
| 后缀 | 归档自动生成：本地时间 `%Y%m%d%H%M%S`；重名时等一秒后重试（时间戳前进），不使用 `_N` 后缀；互换仍按调用方给的后缀定位版本 |
| 互换 | `swap_version_with_cache` 只做 rename：先校验后缀（不合法按版本不存在处理，不报错）；当前缓存 msgs 非空 → 先把当前缓存移动成一个新版本（自动后缀），再把目标版本移入当前缓存；msgs 为空 → 丢弃当前缓存文件（含只有 sys 的情况），直接移入目标版本；目标版本不存在 → 当前缓存非空时仍先存成新版本，然后得到空上下文 |

- `set_system_message` 只保存待定值；`build_context_full` 发现待定值与当前不一致时重写 sys 文件：不保存当前版本、不清空消息、不改动 msgs 文件。
- 归档在移动完版本后，用 `new_messages` 新建当前上下文；归档与清空缓存文件配对执行；内存为空时不产生版本文件（仅有系统消息不归档）。
- 没有系统提示词文件时不写入空 sys 文件、不报错；版本/互换允许 sys 文件缺失。
- 归档后缀自动生成；互换时由调用方指定要恢复的版本后缀。生成版本时即使有待定系统提示词，版本也使用磁盘上原先的 sys 文件。
- 版本是只读快照：复用旧 session 时用互换把版本换回当前缓存，后续内容变更只发生在当前缓存。
- 旧的 `context-history` 与单文件缓存不兼容。
- 互换后目标版本内容成为当前内存与当前系统消息；当前缓存原有的非空内容会存成一个新版本（目标版本移入后即被消耗），待定系统消息清空。
- agent 的 session_id 编码（`<agent_id>-<role_name>[-<event>]`）与 pipeline 的 session 持有方式见 [kissbot-agent-nexus 技术规格](agent-nexus.md)。

## 数据结构定义

```rust
// 上下文消息（OpenAI 兼容，序列化时 role 为平级标签）
enum Message {
    System { content: ArcStr },
    User { content: ArcStr },
    Assistant { content: Value, reasoning_content: Value, tool_calls: Value },  // 后三者原样透传 API 值
    Tool { tool_call_id: ArcStr, content: ArcStr },                    // 无 name 字段
}

// 工具调用：调用方交互层结构（LLM 侧 tool call 与工具结果回填）；
// 工具宿主内部使用自己的工具类型，由调用方在边界转换
// arguments 执行前、result / error 执行后（三者均为 JSON）；
// 只承载 MCP content 字段转成的 Value（内容块数组）：成功进 result、失败进 error（result 留空）；
// structuredContent 仅用于工具宿主内部处理，不进入本结构
struct ToolData { arguments: Value, result: Value, error: Value }
struct ToolCall { id: ArcStr, name: ArcStr, data: ToolData }

// LLM 请求中的工具定义（OpenAI tools[].function 形状）；与 MCP 工具的互转在调用方
struct ToolConfig { name: ArcStr, description: ArcStr, parameters: Arc<Value> }

// 模型响应（归一化）
struct ModelResponse {
    raw_content: Value, content: Option<ArcStr>, thinking: Option<ArcStr>,
    raw_reasoning_content: Value, reasoning_content: Option<ArcStr>,
    raw_tool_calls: Value, tool_calls: Option<Vec<ToolCall>>,
    finish_reason: Option<ArcStr>, total_tokens: u64,
}
```

## 错误类型

```rust
enum Error {
    ModelApiError(ArcStr),                              // 服务商返回非成功状态或调用失败
    ModelProviderNotFound(ArcStr, ArcStr),              // (provider 名, model 名)
    ModelProviderNotSupported(ArcStr),                  // 未知 provider_type
    IoError(std::io::Error),
    ReqwestError(reqwest::Error),
    JsonError(serde_json::Error),
    FileError(kai_file::Error),
    InternalError(ArcStr),
}
pub type Result<T> = std::result::Result<T, Error>;
```

调用方在自己的错误类型上做逐变体映射（不共享同一枚举），以保持各自错误表面稳定。

## 验证

- 单元测试：`cd kissbot-llm && cargo test`（消息/工具结构序列化、两种协议的请求构造与响应归一化、重试与永久性错误、provider/metric 参数合成、上下文缓存与归档）。
- 使用方 kissbot-agent：`cd kissbot-agent && cargo test`（ProviderManager 的工厂去重与 provider_type 分派、有效配置解析、按 session_id 的会话去重与清理，配置模板 schema 校验）。
