# 模型上下文系统重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 OpenAI API 格式重构 kissbot-agent 的上下文系统：Message 枚举表示、多轮 tool 调用循环、Station 框架（本地模式）、上下文缓存与历史归档、重置/压缩流程。

**Architecture:** 上下文表示为 `Message` 枚举（role 即变体）；Session 持内存上下文 + 本地缓存（agent-data 下 JSONL，tokio::fs 追加 + ReverseLineReader 读取）；Station 框架以 `Tool` trait + `StationRuntime`（base_url 空=本地、非空=REST 骨架）承载工具执行；agentic loop 多轮循环处理 tool_calls。

**Tech Stack:** Rust 2024、tokio、serde/serde_json、kai-file（ReverseLineReader）、arc-swap、dashmap、axum（测试 mock）、reqwest。

## Global Constraints

- 遵守 `.claude/rules/coding-standards.md`：时间格式 `yyyy-MM-dd HH:mm:ss`；非枚举/非 Map Key/非 Vec 字段用 `Arc<T>` 包裹（HashSet 字段也用 `Arc<HashSet>`）
- **用户已定（pre-flight）**：`Message`/`ToolCall` 字段按编码规范用 `Arc<String>`（String 字段，含 Option 内）/ `Arc<serde_json::Value>`（arguments）；构造一律 `Arc::new(...)`，读取用 `.as_str()`/`(*x).clone()`/`x.as_ref()`；`ToolConfig` 同（name/description 用 `Arc<String>`，parameters 用 `Arc<Value>`）；`MemoryMsg` 中间结构用普通 String（不参与 wire）
- 不要删除代码中的注释（CLAUDE.md）
- 禁止用 sed/python 修改文件；读写用工具
- 测试运行：各 crate 独立，`cd <crate> && cargo test`（无根 workspace）
- 新增模块需在 `kissbot-agent/src/main.rs` 加 `mod` 声明
- 文本文件 UTF-8、`\n` 换行
- 提交 comment 用中文，包含本次改动全部内容
- 缓存/历史/合批/Station 新模块文件放 `kissbot-agent/src/` 下

---

### Task 1: Message 枚举与 ToolCall 类型

**Files:**
- Modify: `kissbot-agent/src/types.rs`（替换旧 ToolCall 定义，新增 Message 枚举）

**Interfaces:**
- Produces: `types::Message`（System/User/Assistant/Tool 四变体）、`types::ToolCall { id: String, name: String, arguments: serde_json::Value }`——后续所有任务消费

- [ ] **Step 1: 写失败测试**（types.rs 底部 tests 模块追加）

```rust
#[test]
fn message_serde_roundtrip() {
    let msgs = vec![
        Message::System { content: Arc::new("你是助手".into()) },
        Message::User { content: Arc::new("你好".into()) },
        Message::Assistant {
            content: Arc::new(String::new()),
            reasoning_content: Some(Arc::new("思考".into())),
            tool_calls: Some(vec![ToolCall { id: Arc::new("call_1".into()), name: Arc::new("read".into()), arguments: Arc::new(serde_json::json!({"path": "/tmp/a.txt"})) }]),
        },
        Message::Tool { tool_call_id: Arc::new("call_1".into()), name: Arc::new("read".into()), content: Arc::new("内容".into()) },
    ];
    for m in &msgs {
        let json = serde_json::to_value(m).unwrap();
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(m).unwrap());
    }
    // ToolCall arguments 保持 JSON 对象（非字符串）
    let tc = &msgs[2];
    if let Message::Assistant { tool_calls: Some(tcs), .. } = tc {
        assert_eq!(tcs[0].arguments["path"], "/tmp/a.txt");
    } else { panic!("应解析为 Assistant with tool_calls"); }
}

#[test]
fn message_assistant_optional_fields_omitted() {
    let m = Message::Assistant { content: Arc::new("回答".into()), reasoning_content: None, tool_calls: None };
    let v = serde_json::to_value(&m).unwrap();
    assert!(v.get("reasoning_content").is_none(), "None 字段不应序列化");
    assert!(v.get("tool_calls").is_none(), "None 字段不应序列化");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-agent && cargo test message_serde_roundtrip -- --nocapture`
Expected: 编译失败——`Message` 未定义

- [ ] **Step 3: 实现**

`kissbot-agent/src/types.rs` 顶部 import 增加 `use std::sync::Arc;`。把旧的 ToolCall 定义替换为：

```rust
/// OpenAI function call：wire 为 {id, type:"function", function:{name, arguments(JSON 字符串)}}
/// 字段按编码规范用 Arc<String>/Arc<Value>（与 ToolCallRequest.tool_params 先例一致）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Arc<String>,
    pub name: Arc<String>,
    /// 内部为解析后的参数对象；wire 时序列化为 JSON 字符串
    pub arguments: Arc<serde_json::Value>,
}
```

在 `ToolCall` 定义之后新增：

```rust
/// OpenAI 兼容上下文消息：role 即枚举变体，数据字段与 role 同级
/// 字段按编码规范用 Arc<String>（Option 内同样 Arc 包裹）；tool_calls 为 Vec 不包裹
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    System { content: Arc<String> },
    User { content: Arc<String> },
    Assistant {
        content: Arc<String>,
        /// 本地保留（缓存/历史），wire 不发送（DeepSeek/Kimi 文档要求）
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<Arc<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
    },
    Tool {
        tool_call_id: Arc<String>,
        /// 调用的工具名（内部元数据）
        name: Arc<String>,
        /// 调用结果（JSON 字符串或文本）
        content: Arc<String>,
    },
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-agent && cargo test message_`
Expected: 两个测试 PASS（`Message` 枚举定义，`ContextMessage`/`MessageItem` 仍存在未动）

- [ ] **Step 5: 全量构建确认绿**

Run: `cd kissbot-agent && cargo test`
Expected: 全部现有测试通过（旧 ToolCall 替换后无构造点，编译不受影响）

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/types.rs
git commit -m "feat(agent): Message 枚举（OpenAI 格式，role 即变体）与 ToolCall{id,name,arguments} 替换旧 ToolCall——system/user 有 content，assistant 有 content/reasoning_content/tool_calls，tool 有 tool_call_id/name/content"
```

---

### Task 2: 全链路类型切换（Message 替换 ContextMessage/MessageItem）

**Files:**
- Modify: `kissbot-agent/src/session_manager.rs`
- Modify: `kissbot-agent/src/provider.rs`
- Modify: `kissbot-agent/src/model_client.rs`
- Modify: `kissbot-agent/src/memory_reader.rs`
- Modify: `kissbot-agent/src/coordinator.rs`
- Modify: `kissbot-agent/src/types.rs`（删除 ContextMessage、MessageItem）

**Interfaces:**
- Consumes: `Message`/`ToolCall`（Task 1）
- Produces: `SessionContext { push(Message), load_messages(Vec<Message>), build() -> Vec<Message>, is_overflow(max: usize), clear(), set_system_message(String), system_message() -> Option<String> }`；`Session { agent_name: Arc<String>, role_name, mode, context, model, agent_id }`；`Provider::send(effective, messages: &[Message]) -> Result<ModelResponse>`；`ModelClient::call(pm, &[Message])`；`MemoryReader::read_history(...) -> Result<Vec<Message>>`

- [ ] **Step 1: 改 session_manager.rs——SessionContext 用 Vec\<Message\>，Session 加 agent_name**

把 `SessionContext` 整个替换：

```rust
/// 会话上下文：纯内存消息序列 + system 消息（缓存/历史持久化由 coordinator 负责）
pub struct SessionContext {
    messages: Vec<Message>,
    system_message: Option<String>,
}

impl SessionContext {
    pub fn new() -> Self {
        Self { messages: Vec::new(), system_message: None }
    }

    /// 设置系统消息（会话创建或重置时）
    pub fn set_system_message(&mut self, content: String) {
        self.system_message = Some(content);
    }

    /// 取系统消息（压缩/恢复用）
    pub fn system_message(&self) -> Option<&str> {
        self.system_message.as_deref()
    }

    /// 从缓存/记忆加载历史消息重建上下文（system 之外的部分）
    pub fn load_messages(&mut self, messages: Vec<Message>) {
        self.messages.clear();
        self.messages = messages;
    }

    /// 追加一条消息
    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// 构建模型消息列表（system 在最前）
    pub fn build(&self) -> Vec<Message> {
        let mut items = Vec::new();
        if let Some(system) = &self.system_message {
            items.push(Message::System { content: Arc::new(system.clone()) });
        }
        items.extend(self.messages.iter().cloned());
        items
    }

    /// 消息条数（不含 system）
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// 检查是否超长（threshold 来自模型 effective 配置的 max_context_messages）
    pub fn is_overflow(&self, max: usize) -> bool {
        self.messages.len() >= max
    }

    /// 清空上下文（重置时调用；system 保留）
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}
```

`Session` 增加 `agent_name` 字段并在 `new` 中复制：

```rust
pub struct Session {
    pub agent_name: Arc<String>,   // 运行态：从 key 复制（context 配置查找用）
    pub role_name: Arc<String>,
    pub mode: Arc<Mode>,
    pub context: tokio::sync::Mutex<SessionContext>,
    pub model: ArcSwap<Option<ProviderModel>>,
    pub agent_id: Arc<String>,
}

impl Session {
    pub fn new(key: &SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> Self {
        Self {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new()),
            model: ArcSwap::from_pointee(model),
            agent_id,
        }
    }
}
```

`session_manager.rs` 顶部 import 改为 `use crate::types::{Message, Mode, SessionKey};`，删除对 `ContextMessage`/`MessageItem` 的引用。

- [ ] **Step 2: 改 provider.rs——Provider 消费 &[Message]**

`provider.rs` 顶部 import：`use crate::types::{Error, Message, ModelResponse, Result, ToolCall};`（删除 MessageItem）。trait 签名：

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[Message]) -> Result<ModelResponse>;
    #[allow(dead_code)]
    async fn list_models(&self) -> Result<Vec<String>>;
    #[allow(dead_code)]
    fn provider_type(&self) -> &str { "" }
}
```

`openai_body` 替换为：

```rust
fn openai_body(effective: &EffectiveModelConfig, messages: &[Message]) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages.iter().map(|m| match m {
        Message::System { content } => json!({ "role": "system", "content": content }),
        Message::User { content } => json!({ "role": "user", "content": content }),
        Message::Assistant { content, tool_calls, .. } => {
            let mut v = json!({ "role": "assistant", "content": content });
            if let Some(tcs) = tool_calls {
                v["tool_calls"] = json!(tcs.iter().map(|tc| json!({
                    "id": tc.id,
                    "type": "function",
                    "function": { "name": tc.name, "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default() },
                })).collect::<Vec<_>>());
            }
            v
        }
        Message::Tool { tool_call_id, content, .. } => {
            json!({ "role": "tool", "tool_call_id": tool_call_id, "content": content })
        }
    }).collect();
    let mut body = json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
        "stream": false,
    });
    if let Some(t) = effective.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = &effective.thinking {
        body["thinking"] = json!({ "type": t });
    }
    if let Some(e) = &effective.reasoning_effort {
        body["reasoning_effort"] = json!(e);
    }
    body
}
```

`parse_openai_response` 增加 tool_calls 解析：

```rust
fn parse_openai_response(data: &serde_json::Value) -> ModelResponse {
    let choice = &data["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("").to_string();
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop").to_string();
    let reasoning_content = choice["message"]["reasoning_content"].as_str()
        .map(String::from).filter(|s| !s.is_empty());
    let (content, tag_reasoning) = strip_think_tag(&content);
    let thinking = tag_reasoning.filter(|s| !s.is_empty());
    let tool_calls = choice["message"]["tool_calls"].as_array().map(|arr| {
        arr.iter().filter_map(|tc| {
            let id = tc["id"].as_str()?.to_string();
            let name = tc["function"]["name"].as_str()?.to_string();
            let arguments = tc["function"]["arguments"].as_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(serde_json::Value::Null);
            Some(ToolCall { id: Arc::new(id), name: Arc::new(name), arguments: Arc::new(arguments) })
        }).collect()
    }).unwrap_or_default();
    ModelResponse { content, reasoning_content, thinking, tool_calls, finish_reason }
}
```

`anthropic_body` 映射 Message（content-only，Tool 消息与 tool_calls 本轮不支持）：

```rust
fn anthropic_body(effective: &EffectiveModelConfig, messages: &[Message]) -> serde_json::Value {
    let system_parts: Vec<String> = messages.iter()
        .filter_map(|m| match m {
            Message::System { content } => Some(content.to_string()),
            _ => None,
        })
        .collect();
    let system = system_parts.join("\n");

    let msgs: Vec<serde_json::Value> = messages.iter().filter_map(|m| match m {
        Message::System { .. } => None,
        Message::User { content } => Some(json!({ "role": "user", "content": content })),
        Message::Assistant { content, .. } => Some(json!({ "role": "assistant", "content": content })),
        Message::Tool { .. } => None,  // 本轮不支持工具消息
    }).collect();

    let mut body = json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if let Some(t) = effective.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = &effective.thinking {
        body["thinking"] = json!({ "type": t });
    }
    if let Some(e) = &effective.reasoning_effort {
        body["output_config"] = json!({ "effort": e });
    }
    body
}
```

两个 Provider 实现里 `async fn send` 签名同步改为 `messages: &[Message]`（内容不变，只改类型）。

- [ ] **Step 3: 改 model_client.rs**

```rust
use crate::types::{Error, Message, ModelResponse, Result};

pub async fn call(&self, pm: &ProviderModel, messages: &[Message]) -> Result<ModelResponse> {
    let effective = self.config_manager.resolve_effective_config(pm).await
        .ok_or_else(|| Error::ModelProviderNotSupported(format!(
            "provider/model 不存在: {}/{}", pm.provider, pm.model)))?;
    let provider: Box<dyn Provider> = self.build_provider(&effective)?;
    self.call_with_retry(&effective, provider, messages).await
}

async fn call_with_retry(
    &self,
    effective: &EffectiveModelConfig,
    provider: Box<dyn Provider>,
    messages: &[Message],
) -> Result<ModelResponse> {
    // 函数体不变，仅 messages 参数类型改为 &[Message]
}
```

- [ ] **Step 4: 改 memory_reader.rs——records_to_messages 输出 Message**

`read_history` 返回类型改为 `Result<Vec<Message>>`。`records_to_messages` 替换为（`time` 不再进 Message，content 按文本提取；本轮仍走旧 `/query` 端点，Task 9 重构）：

```rust
fn records_to_messages(&self, records: &[serde_json::Value]) -> Vec<Message> {
    records.iter().filter_map(|r| {
        let msg_type = r["msg_type"].as_str().unwrap_or("");
        let content = Arc::new(extract_record_text(&r["content"]));
        match msg_type {
            "channel" | "text" => Some(Message::User { content }),
            "think" => Some(Message::Assistant { content, reasoning_content: None, tool_calls: None }),
            "tool_call" => Some(Message::Assistant {
                content: Arc::new(String::new()),
                reasoning_content: None,
                tool_calls: Some(vec![ToolCall {
                    id: Arc::new(String::new()),
                    name: Arc::new(r["tool_name"].as_str().unwrap_or("").to_string()),
                    arguments: Arc::new(r["tool_params"].as_str()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(serde_json::Value::Null)),
                }]),
            }),
            "tool_result" => Some(Message::Tool {
                tool_call_id: Arc::new(String::new()),
                name: Arc::new(r["tool_name"].as_str().unwrap_or("").to_string()),
                content: Arc::new(r["tool_result"].to_string()),
            }),
            _ => None,
        }
    }).collect()
}

/// 从 Content 枚举 JSON（{"Text": "...", "Multi": [...]}）提取文本（与 coordinator.extract_text 同语义）
fn extract_record_text(content: &serde_json::Value) -> String {
    match content.get("Text") {
        Some(t) => t.as_str().unwrap_or("").to_string(),
        None => match content.get("Multi") {
            Some(arr) => arr.as_array().map(|items| items.iter()
                .filter_map(|c| c.get("Text").and_then(|t| t.as_str()).map(String::from))
                .collect::<Vec<_>>().join("\n")).unwrap_or_default(),
            None => String::new(),
        },
    }
}
```

`memory_reader.rs` 顶部 import：`use crate::types::{Message, Mode, Result, Error, ToolCall};`（删除 ContextMessage）。

- [ ] **Step 5: 改 coordinator.rs 调用点**

`run_agentic_loop` 中：

```rust
// 1. 追加用户消息到该会话上下文（time/messenger 等不再保留，只留文本）
{
    let mut ctx = session.context.lock().await;
    ctx.push(Message::User { content: Arc::new(content_text.clone()) });
}
```

`build_initial_context` 中 `session.context.lock().await.load_history(history);` 改为 `load_messages(history);`。

`types.rs` 删除 `MessageItem` 与 `ContextMessage` 定义。`coordinator.rs` 顶部 import 的 `ContextMessage` 删除，加 `Message`。

- [ ] **Step 6: 改 provider.rs 测试构造 Message**

`provider.rs` tests 中所有 `MessageItem { role, content }` 构造替换为：

```rust
let msgs = vec![
    Message::System { content: Arc::new("你是助手".into()) },
    Message::User { content: Arc::new("你好".into()) },
];
```

`sample_effective()` 不变。`session_manager.rs` 测试中 `Session::new` 调用无需改（key 里已有 agent_name）。

- [ ] **Step 7: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过。若有编译错误按上述签名逐处修复。

- [ ] **Step 8: Commit**

```bash
git add kissbot-agent/src/
git commit -m "refactor(agent): Message 全链路替换 ContextMessage/MessageItem——SessionContext 改 Vec<Message>（push/load_messages/build/is_overflow(max)），Session 加 agent_name；Provider/ModelClient 消费 &[Message]；openai wire 支持 tool_calls/tool 消息、parse 解析 tool_calls；anthropic 保持纯文本；memory_reader 输出 Message；删除 ContextMessage/MessageItem"
```

---

### Task 3: context 配置段（agent→role 三层继承）与 max_context_messages

**Files:**
- Create: `kissbot-agent/src/context_config.rs`
- Modify: `kissbot-agent/src/config_manager.rs`
- Modify: `kissbot-agent/src/http_server.rs`
- Modify: `kissbot-agent/src/main.rs`
- Modify: `kissbot-agent/src/coordinator.rs`（溢出检查改用 effective.max_context_messages）

**Interfaces:**
- Produces: `context_config::{AgentContextConfig, RoleContextConfig, EffectiveContextConfig, merge_context_config(Option<&AgentContextConfig>, Option<&RoleContextConfig>) -> EffectiveContextConfig}`；`ConfigManager::context_config(agent_name, role_name) -> EffectiveContextConfig`；`EffectiveModelConfig.max_context_messages: usize`；常量 `DEFAULT_CHANNEL_BATCH_INTERVAL_SECS=3`、`DEFAULT_MEMORY_TIME_SECS=3600`、`DEFAULT_MEMORY_COUNT=50`

- [ ] **Step 1: 写失败测试**

`kissbot-agent/src/context_config.rs` 底部 tests：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_none_uses_globals() {
        let eff = merge_context_config(None, None);
        assert_eq!(eff.channel_batch_interval_secs, DEFAULT_CHANNEL_BATCH_INTERVAL_SECS);
        assert_eq!(eff.memory_time_secs, DEFAULT_MEMORY_TIME_SECS);
        assert_eq!(eff.memory_count, DEFAULT_MEMORY_COUNT);
        assert!(eff.stations.is_empty());
    }

    #[test]
    fn merge_agent_then_role_override() {
        let agent = AgentContextConfig {
            default_channel_batch_interval_secs: 5,
            default_memory_time_secs: 7200,
            default_memory_count: 100,
            default_compress_prompt: "agent模板".into(),
            default_stations: Arc::new(["s1".into()].into_iter().collect()),
            roles: Arc::new(ArcSwapHashMap::new()),
        };
        let role = RoleContextConfig {
            channel_batch_interval_secs: Some(7),
            memory_time_secs: None,
            memory_count: None,
            compress_prompt: None,
            stations: None,
        };
        let eff = merge_context_config(Some(&agent), Some(&role));
        assert_eq!(eff.channel_batch_interval_secs, 7, "role 覆盖 agent");
        assert_eq!(eff.memory_time_secs, 7200, "role 未配继承 agent");
        assert_eq!(eff.memory_count, 100);
        assert_eq!(eff.compress_prompt, "agent模板");
        assert!(eff.stations.contains("s1"));
    }

    #[test]
    fn role_stations_override_agent() {
        let agent = AgentContextConfig {
            default_channel_batch_interval_secs: 3,
            default_memory_time_secs: 3600,
            default_memory_count: 50,
            default_compress_prompt: "t".into(),
            default_stations: Arc::new(["s1".into()].into_iter().collect()),
            roles: Arc::new(ArcSwapHashMap::new()),
        };
        let role = RoleContextConfig {
            channel_batch_interval_secs: None,
            memory_time_secs: None,
            memory_count: None,
            compress_prompt: None,
            stations: Some(["s2".into()].into_iter().collect()),
        };
        let eff = merge_context_config(Some(&agent), Some(&role));
        assert!(eff.stations.contains("s2") && !eff.stations.contains("s1"), "role stations 整体覆盖");
    }

    #[test]
    fn agent_role_config_serde_roundtrip() {
        let agent = AgentContextConfig {
            default_channel_batch_interval_secs: 3,
            default_memory_time_secs: 3600,
            default_memory_count: 50,
            default_compress_prompt: "t".into(),
            default_stations: Arc::new(HashSet::new()),
            roles: Arc::new(ArcSwapHashMap::new()),
        };
        let json = serde_json::to_string(&agent).unwrap();
        let back: AgentContextConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.default_memory_count, 50);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test merge_none_uses_globals`
Expected: 编译失败——context_config 模块不存在

- [ ] **Step 3: 实现 context_config.rs**

```rust
use std::collections::HashSet;
use std::sync::Arc;

use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};

// ========== 全局默认值 ==========

/// channel 合批最小间隔（秒）
pub const DEFAULT_CHANNEL_BATCH_INTERVAL_SECS: u64 = 3;
/// 记忆提取时间窗（秒）
pub const DEFAULT_MEMORY_TIME_SECS: u64 = 3600;
/// 记忆提取条数
pub const DEFAULT_MEMORY_COUNT: usize = 50;
/// event 压缩指令默认模板
pub const DEFAULT_COMPRESS_PROMPT: &str = "请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。";

// ========== 配置结构 ==========

/// agent 级 context 配置（key = agent_name，覆盖全局默认；类似 ProviderConfig 的 default_*）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContextConfig {
    pub default_channel_batch_interval_secs: u64,
    pub default_memory_time_secs: u64,
    pub default_memory_count: usize,
    pub default_compress_prompt: Arc<String>,
    /// 启用的 station_id 集合（Set 形式）
    pub default_stations: Arc<HashSet<String>>,
    /// key = role_name
    pub roles: Arc<ArcSwapHashMap<String, RoleContextConfig>>,
}

/// role 级 context 配置（可选覆盖 agent 默认；类似 ModelConfig 的 Option 继承）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleContextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_batch_interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_time_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compress_prompt: Option<Arc<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stations: Option<Arc<HashSet<String>>>,
}

/// 合并后的有效配置（现场合成，不持久化）
#[derive(Debug, Clone)]
pub struct EffectiveContextConfig {
    pub channel_batch_interval_secs: u64,
    pub memory_time_secs: u64,
    pub memory_count: usize,
    pub compress_prompt: String,
    pub stations: HashSet<String>,
}

/// 三层合并：全局默认 ← agent ← role（role Some 覆盖 agent；无 agent 用全局默认）
/// role 只可能来自 agent.roles（role 无 agent 时不可达），故 agent 为 None 时直接返回全局默认
pub fn merge_context_config(
    agent: Option<&AgentContextConfig>,
    role: Option<&RoleContextConfig>,
) -> EffectiveContextConfig {
    let Some(a) = agent else {
        return EffectiveContextConfig {
            channel_batch_interval_secs: DEFAULT_CHANNEL_BATCH_INTERVAL_SECS,
            memory_time_secs: DEFAULT_MEMORY_TIME_SECS,
            memory_count: DEFAULT_MEMORY_COUNT,
            compress_prompt: DEFAULT_COMPRESS_PROMPT.to_string(),
            stations: HashSet::new(),
        };
    };
    EffectiveContextConfig {
        channel_batch_interval_secs: role.and_then(|x| x.channel_batch_interval_secs)
            .unwrap_or(a.default_channel_batch_interval_secs),
        memory_time_secs: role.and_then(|x| x.memory_time_secs)
            .unwrap_or(a.default_memory_time_secs),
        memory_count: role.and_then(|x| x.memory_count)
            .unwrap_or(a.default_memory_count),
        compress_prompt: role.and_then(|x| x.compress_prompt.clone())
            .unwrap_or_else(|| a.default_compress_prompt.to_string()),
        stations: role.and_then(|x| x.stations.clone())
            .map(|s| (*s).clone())
            .unwrap_or_else(|| (*a.default_stations).clone()),
    }
}
```

`main.rs` 加 `mod context_config;`。

- [ ] **Step 4: 改 config_manager.rs——NexusRepo.context 与 ProviderConfig/ModelConfig**

`NexusRepo` 加字段：

```rust
use crate::context_config::AgentContextConfig;

pub struct NexusRepo {
    pub channels: Arc<ArcSwapHashMap<String, ChannelConfig>>,
    pub providers: Arc<ArcSwapHashMap<String, ProviderConfig>>,
    pub memory_structs: Arc<ArcSwapHashMap<String, MemoryStructConfig>>,
    pub stations: Arc<ArcSwapHashMap<String, StationConfig>>,
    /// agent_name → AgentContextConfig（上下文配置，三层继承见 context_config 模块）
    pub context: Arc<ArcSwapHashMap<String, AgentContextConfig>>,
    pub default_model: Arc<ProviderModel>,
    pub default_system_prompt: Arc<String>,
}
```

`Default` 实现加 `context: Arc::new(ArcSwapHashMap::new()),`。

`ProviderConfig` 加：

```rust
pub default_max_context_messages: usize,     // 上下文消息条数上限（溢出触发重置/压缩）
```

`ModelConfig` 加：

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub max_context_messages: Option<usize>,
```

`EffectiveModelConfig` 加：

```rust
pub max_context_messages: usize,
```

`resolve_effective_config` 合成行加：

```rust
max_context_messages: model_cfg.as_ref().and_then(|m| m.max_context_messages).unwrap_or(provider.default_max_context_messages),
```

新增 ConfigManager 方法：

```rust
/// 按 (agent_name, role_name) 合并 context 配置（三层继承：全局默认 ← agent ← role）
pub async fn context_config(&self, agent_name: &str, role_name: &str) -> EffectiveContextConfig {
    let repo = self.nexus_repo.read().await;
    let agent = repo.context.get(agent_name).map(|s| s.load_full());
    let role = agent.as_ref().and_then(|a| a.roles.get(role_name).map(|s| s.load_full()));
    crate::context_config::merge_context_config(agent.as_deref(), role.as_deref())
}
```

- [ ] **Step 5: 同步 NexusRepo 构造点**

`config_manager.rs` 测试中所有内联 `NexusRepo { ... }` 构造（约 4 处：`write_config_op_error_skips_persist`、`add_remove_admin_missing_channel_errors`、`update_channel_mutates_and_persists`、`resolve_effective_config_*` 系列）加 `context: Arc::new(ArcSwapHashMap::new()),`；`sample_provider` 加 `default_max_context_messages: 100,`；`ModelConfig` 构造处（`resolve_effective_config_merges_provider_and_model` 等）加 `max_context_messages: Some(80),` 或 `None`。`nexus_repo_serde_roundtrip`/`nexus_repo_default_empty` 测试加 context 断言可选。

`http_server.rs:184` 附近的内联 `NexusRepo` 构造加 `context: Arc::new(ArcSwapHashMap::new()),`（该处已 `use kissbot_api::ArcSwapHashMap`）。

- [ ] **Step 6: coordinator 溢出检查改用 effective**

`run_agentic_loop` 末尾的溢出检查：

```rust
// 6. 检查上下文超长（阈值来自会话模型的 effective.max_context_messages）
let overflow = {
    let ctx = session.context.lock().await;
    let model = session.model.load_full();
    match model.as_ref() {
        Some(pm) => match self.config.resolve_effective_config(pm).await {
            Some(eff) => ctx.is_overflow(eff.max_context_messages as usize),
            None => false,
        },
        None => false,
    }
};
```

- [ ] **Step 7: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过（含 merge 三个测试）

- [ ] **Step 8: Commit**

```bash
git add kissbot-agent/src/
git commit -m "feat(agent): context 配置段（agent→role 三层继承，全局默认 3s/1h/50条/默认压缩模板/空stations）——AgentContextConfig/RoleContextConfig/EffectiveContextConfig + merge_context_config；条数上限入 provider/model 配置（default_max_context_messages/max_context_messages 继承合成）；溢出检查改用 effective.max_context_messages"
```

---

### Task 4: 上下文缓存 ContextCache

**Files:**
- Create: `kissbot-agent/src/context_cache.rs`
- Modify: `kissbot-agent/src/main.rs`

**Interfaces:**
- Consumes: `Message`、`SessionKey`、`Mode`
- Produces: `context_cache::{encode_session_key(&SessionKey) -> String, ContextCache { new(data_dir: &str), path_for(&SessionKey) -> PathBuf, append(&SessionKey, &[Message]) -> Result<()>, read_all(&SessionKey) -> Result<Vec<Message>>, clear(&SessionKey) -> Result<()> }`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, Mode, SessionKey};

    fn key() -> SessionKey {
        SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) }
    }

    fn sample_msgs() -> Vec<Message> {
        vec![
            Message::User { content: Arc::new("你好".into()) },
            Message::Assistant { content: Arc::new("在的".into()), reasoning_content: Some(Arc::new("思考".into())), tool_calls: None },
        ]
    }

    #[test]
    fn encode_session_key_distinguishes() {
        let k1 = key();
        let k2 = SessionKey { agent_name: "a1".into(), role_name: "r2".into(), mode: Mode::Role };
        let k3 = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e2".into()) };
        assert_ne!(encode_session_key(&k1), encode_session_key(&k2), "不同 role 不同编码");
        assert_ne!(encode_session_key(&k1), encode_session_key(&k3), "不同 event 不同编码");
        // 编码不含分隔符原始字符（文件名安全）
        let enc = encode_session_key(&key());
        assert!(!enc.contains('|') && !enc.contains('/'), "编码应文件名安全");
    }

    #[tokio::test]
    async fn append_then_read_all_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContextCache::new(dir.path().to_str().unwrap());
        let k = key();
        assert!(cache.read_all(&k).await.unwrap().is_empty(), "初始为空");
        cache.append(&k, &sample_msgs()).await.unwrap();
        let back = cache.read_all(&k).await.unwrap();
        assert_eq!(back.len(), 2);
        assert!(matches!(&back[0], Message::User { content } if content.as_str() == "你好"));
        assert!(matches!(&back[1], Message::Assistant { reasoning_content: Some(r), .. } if r.as_str() == "思考"), "reasoning_content 应保留");
    }

    #[tokio::test]
    async fn append_twice_accumulates_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContextCache::new(dir.path().to_str().unwrap());
        let k = key();
        cache.append(&k, &sample_msgs()).await.unwrap();
        cache.append(&k, &[Message::User { content: Arc::new("再问".into()) }]).await.unwrap();
        assert_eq!(cache.read_all(&k).await.unwrap().len(), 3, "追加不截断");
        cache.clear(&k).await.unwrap();
        assert!(cache.read_all(&k).await.unwrap().is_empty(), "clear 后为空");
        // 文件不存在时 clear 幂等
        cache.clear(&k).await.unwrap();
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test encode_session_key_distinguishes`
Expected: 编译失败——context_cache 模块不存在

- [ ] **Step 3: 实现 context_cache.rs**

```rust
use std::path::{Path, PathBuf};

use serde_json;
use tokio::io::AsyncWriteExt;

use crate::types::{Message, Mode, Result, SessionKey};
use crate::Error;

/// session_key → 文件名安全编码（十六进制，避免路径/非法字符；agent|role|mode 含 event id）
pub fn encode_session_key(key: &SessionKey) -> String {
    let mode = match &key.mode {
        Mode::Role => "role".to_string(),
        Mode::Event(e) => format!("event:{}", e),
    };
    let raw = format!("{}|{}|{}", key.agent_name, key.role_name, mode);
    raw.as_bytes().iter().map(|b| format!("{:02x}", b)).collect()
}

/// 会话上下文本地缓存：<data_dir>/context/<session_key编码>.jsonl
/// 存储时不截断（tokio::fs 追加）；读取时全量回读（ReverseLineReader 从尾读再反转）
pub struct ContextCache {
    dir: PathBuf,
}

impl ContextCache {
    pub fn new(data_dir: &str) -> Self {
        Self { dir: PathBuf::from(data_dir).join("context") }
    }

    pub fn path_for(&self, key: &SessionKey) -> PathBuf {
        self.dir.join(format!("{}.jsonl", encode_session_key(key)))
    }

    /// 追加消息（每行一条 Message JSON；不截断）
    pub async fn append(&self, key: &SessionKey, messages: &[Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| Error::IoError(e.to_string()))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true).append(true).open(&path).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        for m in messages {
            let line = serde_json::to_string(m)?;
            file.write_all(line.as_bytes()).await
                .map_err(|e| Error::IoError(e.to_string()))?;
            file.write_all(b"\n").await
                .map_err(|e| Error::IoError(e.to_string()))?;
        }
        Ok(())
    }

    /// 全量回读（按时间顺序）；文件不存在返回空
    pub async fn read_all(&self, key: &SessionKey) -> Result<Vec<Message>> {
        let path = self.path_for(key);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut reader = kai_file::ReverseLineReader::new(&path, None, None).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        let mut msgs = Vec::new();
        while let Some(line) = reader.next_line().await
            .map_err(|e| Error::IoError(e.to_string()))?
        {
            let s = line.line.trim();
            if s.is_empty() { continue; }
            if let Ok(m) = serde_json::from_str::<Message>(s) {
                msgs.push(m);
            }
        }
        msgs.reverse();
        Ok(msgs)
    }

    /// 清空缓存（重建/重置时调用；文件不存在幂等）
    pub async fn clear(&self, key: &SessionKey) -> Result<()> {
        let path = self.path_for(key);
        match tokio::fs::remove_file(&path).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::IoError(e.to_string())),
        }
    }
}
```

`main.rs` 加 `mod context_cache;`。

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-agent && cargo test context_cache`
Expected: 3 个测试 PASS

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/
git commit -m "feat(agent): 上下文本地缓存 ContextCache——context/<session_key编码>.jsonl，tokio::fs 追加不截断、ReverseLineReader 全量回读、clear 幂等；session_key 十六进制编码（agent|role|mode）"
```

---

### Task 5: 历史归档 HistoryArchive

**Files:**
- Create: `kissbot-agent/src/history.rs`
- Modify: `kissbot-agent/src/main.rs`

**Interfaces:**
- Consumes: `ContextCache::path_for`（Task 4 的编码）、`SessionKey`
- Produces: `history::HistoryArchive { new(data_dir: &str), archive(&SessionKey, source: &Path) -> Result<PathBuf> }`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_cache::{ContextCache, encode_session_key};
    use crate::types::{Message, Mode, SessionKey};

    fn key() -> SessionKey {
        SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role }
    }

    #[tokio::test]
    async fn archive_copies_cache_file_with_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContextCache::new(dir.path().to_str().unwrap());
        let history = HistoryArchive::new(dir.path().to_str().unwrap());
        let k = key();
        cache.append(&k, &[Message::User { content: Arc::new("你好".into()) }]).await.unwrap();
        let source = cache.path_for(&k);
        let dest = history.archive(&k, &source).await.unwrap();
        // 目标文件名 = <key编码>-<时间戳>.jsonl
        let fname = dest.file_name().unwrap().to_str().unwrap().to_string();
        assert!(fname.starts_with(&encode_session_key(&k)), "文件名以 key 编码开头: {}", fname);
        assert!(fname.ends_with(".jsonl"));
        // 内容与源一致
        assert_eq!(tokio::fs::read_to_string(&dest).await.unwrap(),
                   tokio::fs::read_to_string(&source).await.unwrap());
        // 原文件保留（压缩后仍要重写）
        assert!(source.exists());
    }

    #[tokio::test]
    async fn archive_missing_source_errors() {
        let dir = tempfile::tempdir().unwrap();
        let history = HistoryArchive::new(dir.path().to_str().unwrap());
        let missing = dir.path().join("nonexistent.jsonl");
        assert!(history.archive(&key(), &missing).await.is_err(), "源不存在应报错");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test archive_copies_cache_file_with_timestamp`
Expected: 编译失败——history 模块不存在

- [ ] **Step 3: 实现 history.rs**

```rust
use std::path::{Path, PathBuf};

use chrono::Local;

use crate::context_cache::encode_session_key;
use crate::types::{Result, SessionKey};
use crate::Error;

/// 历史上下文归档：<data_dir>/context-history/<session_key编码>-<时间戳>.jsonl
/// 归档 = 直接复制当前缓存文件（无包装格式），本轮只写不读
pub struct HistoryArchive {
    dir: PathBuf,
}

impl HistoryArchive {
    pub fn new(data_dir: &str) -> Self {
        Self { dir: PathBuf::from(data_dir).join("context-history") }
    }

    /// 复制缓存文件到历史目录（文件名带时间戳）；返回目标路径
    pub async fn archive(&self, key: &SessionKey, source: &Path) -> Result<PathBuf> {
        if !source.exists() {
            return Err(Error::IoError(format!("缓存文件不存在: {}", source.display())));
        }
        tokio::fs::create_dir_all(&self.dir).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        let ts = Local::now().format("%Y-%m-%d-%H%M%S").to_string();
        let dest = self.dir.join(format!("{}-{}.jsonl", encode_session_key(key), ts));
        tokio::fs::copy(source, &dest).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        Ok(dest)
    }
}
```

`main.rs` 加 `mod history;`。

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-agent && cargo test history`
Expected: 2 个测试 PASS

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/
git commit -m "feat(agent): 历史上下文归档 HistoryArchive——复制当前缓存文件到 context-history/<key编码>-<时间戳>.jsonl，无包装格式，本轮只写不读"
```

---

### Task 6: 缓存/历史接入 coordinator（构建、追加、重置归档）

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: `ContextCache`、`HistoryArchive`、`SessionKey`、`Message`
- Produces: `AgentCoordinator` 持有 `cache: Arc<ContextCache>`、`history: Arc<HistoryArchive>`；`build_initial_context` event 分支读缓存、role 分支暂用 `read_history`；`run_agentic_loop` 每步追加缓存；`reset_context` 归档 + 清空 + 重建；`session_key_of_session(&Session) -> SessionKey`（内部辅助）

- [ ] **Step 1: 改 coordinator.rs——字段与初始化**

`AgentCoordinator` 加字段：

```rust
    /// 上下文本地缓存（agent-data/context）
    cache: Arc<ContextCache>,
    /// 历史上下文归档（agent-data/context-history）
    history: Arc<HistoryArchive>,
```

`new()` 中初始化（在 `config` 之后）：

```rust
let data_dir = config.data_dir().to_string();
```

并在结构体构造里加：

```rust
cache: Arc::new(ContextCache::new(&data_dir)),
history: Arc::new(HistoryArchive::new(&data_dir)),
```

顶部 import：`use crate::context_cache::ContextCache;`、`use crate::history::HistoryArchive;`、`use crate::types::{Mode, Message, ...};`。

新增辅助方法：

```rust
/// 从 Session 运行态构造 SessionKey（缓存/历史定位用）
fn session_key_of_session(&self, session: &Arc<Session>) -> SessionKey {
    session_key_of(session.agent_name.as_str(), session.role_name.as_str(), (*session.mode).clone())
}
```

- [ ] **Step 2: 改 build_initial_context——event 读缓存，role 暂用记忆**

`build_initial_context` 的「历史记忆加载」段替换为：

```rust
// 按模式加载上下文：event 从缓存恢复；role 从记忆读取（Task 10 改为记忆打包）
let key = self.session_key_of_session(session);
match &*session.mode {
    Mode::Event(_) => {
        if let Ok(history) = self.cache.read_all(&key).await {
            session.context.lock().await.load_messages(history);
        }
    }
    Mode::Role => {
        if let Ok(history) = self.memory_reader
            .read_history(&self.config, session.agent_id.as_str(), session.role_name.as_str(), &session.mode)
            .await
        {
            session.context.lock().await.load_messages(history);
        }
    }
}
```

删除原 `read_history` 调用与 `load_history`（已改名 load_messages）。`read_memory_struct_index` 调用保持不变。

- [ ] **Step 3: 改 run_agentic_loop——每步追加缓存**

`run_agentic_loop` 中：
- 步骤 1 追加 user 消息后加：

```rust
// 1b. 用户消息写缓存
let key = self.session_key_of_session(session);
self.cache.append(&key, &[Message::User { content: Arc::new(content_text.clone()) }]).await;
```

- 步骤 3 追加 assistant 后加：

```rust
// 3b. assistant 回复写缓存
self.cache.append(&key, &[Message::Assistant { content: Arc::new(model_resp.content.clone()), reasoning_content: model_resp.reasoning_content.clone().map(Arc::new), tool_calls: None }]).await;
```

注意：`key` 在步骤 1b 定义，后续复用。步骤 3 的 `push_assistant` 改为 `ctx.push(Message::Assistant { ... })`。

- [ ] **Step 4: 改 reset_context——归档 + 清空 + 重建**

```rust
/// 上下文重置：归档当前缓存 → 清空缓存 → 清空内存 → 重建（按模式）
async fn reset_context(&self, session: &Arc<Session>) {
    let key = self.session_key_of_session(session);
    let path = self.cache.path_for(&key);
    if path.exists() {
        let _ = self.history.archive(&key, &path).await;
    }
    let _ = self.cache.clear(&key).await;
    session.context.lock().await.clear();
    self.build_initial_context(session).await;
    info!("会话上下文已重置: role={} mode={:?}", session.role_name, session.mode);
}
```

- [ ] **Step 5: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过（现有 coordinator 测试不依赖缓存行为）

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "feat(agent): 缓存/历史接入 coordinator——build_initial_context 按模式加载（event 缓存恢复/role 记忆读取），agentic loop 每步追加缓存，reset 归档缓存+清空+重建；SessionKey 由 Session 运行态构造"
```

---

### Task 7: Channel 合批（batching.rs + coordinator 接入）

**Files:**
- Create: `kissbot-agent/src/batching.rs`
- Modify: `kissbot-agent/src/main.rs`
- Modify: `kissbot-agent/src/session_manager.rs`（Session 加 batch 缓冲与代数）
- Modify: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: `Message`、`EffectiveContextConfig.channel_batch_interval_secs`
- Produces: `batching::{BatchBuffer { new(), push(&mut,String), is_empty(), take() -> Vec<(String,String)>, clear() }, pack_batch(&[(String,String)]) -> String}`；`Session.batch: tokio::sync::Mutex<BatchBuffer>`、`Session.batch_gen: Arc<AtomicU64>`；`run_agentic_loop(channel_id, session, content_text, out_channel)`（签名重构，不再收 event）

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_batch_formats_name_content_lines() {
        let items = vec![
            ("u1".to_string(), "你好".to_string()),
            ("u2".to_string(), "在吗".to_string()),
            (String::new(), "无名字".to_string()),
        ];
        assert_eq!(pack_batch(&items), "u1: 你好\nu2: 在吗\n无名字");
    }

    #[test]
    fn batch_buffer_push_take_clear() {
        let mut b = BatchBuffer::new();
        assert!(b.is_empty());
        b.push("u1", "a");
        b.push("u2", "b");
        assert!(!b.is_empty());
        let items = b.take();
        assert_eq!(items.len(), 2);
        assert!(b.is_empty(), "take 后清空");
        b.push("u1", "c");
        b.clear();
        assert!(b.is_empty());
        assert!(b.take().is_empty());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test pack_batch_formats_name_content_lines`
Expected: 编译失败——batching 模块不存在

- [ ] **Step 3: 实现 batching.rs**

```rust
// ========== Channel 合批 ==========

/// 每会话待合批缓冲：消息先入缓冲，超时（channel_batch_interval_secs）无新消息才打包为一条 user 消息
#[derive(Default)]
pub struct BatchBuffer {
    items: Vec<(String, String)>,  // (user_name, 文本)
}

impl BatchBuffer {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, name: &str, text: &str) {
        self.items.push((name.to_string(), text.to_string()));
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 取出全部并清空（打包用）
    pub fn take(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.items)
    }

    /// 清空（会话重置时）
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// 打包为一条 user 消息的 content：逐行 "name: text"（name 为空只留 text）
pub fn pack_batch(items: &[(String, String)]) -> String {
    items.iter().map(|(name, text)| {
        if name.is_empty() { text.clone() } else { format!("{}: {}", name, text) }
    }).collect::<Vec<_>>().join("\n")
}
```

`main.rs` 加 `mod batching;`。

- [ ] **Step 4: session_manager.rs——Session 加 batch 缓冲与代数**

```rust
use std::sync::atomic::AtomicU64;

pub struct Session {
    pub agent_name: Arc<String>,
    pub role_name: Arc<String>,
    pub mode: Arc<Mode>,
    pub context: tokio::sync::Mutex<SessionContext>,
    /// 待合批缓冲（合批超时后打包进上下文）
    pub batch: tokio::sync::Mutex<crate::batching::BatchBuffer>,
    /// 合批代数：重置时递增使旧计时任务失效
    pub batch_gen: Arc<AtomicU64>,
    pub model: ArcSwap<Option<ProviderModel>>,
    pub agent_id: Arc<String>,
}

impl Session {
    pub fn new(key: &SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> Self {
        Self {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new()),
            batch: tokio::sync::Mutex::new(crate::batching::BatchBuffer::new()),
            batch_gen: Arc::new(AtomicU64::new(0)),
            model: ArcSwap::from_pointee(model),
            agent_id,
        }
    }
}
```

- [ ] **Step 5: coordinator.rs——合批接入 + run_agentic_loop 签名重构**

`handle_incoming` 第 3 步替换为（合批入口）：

```rust
// 3. 普通消息：无 out_channel 不进 Agentic Loop（ChannelRecord 已存，结束）
let Some(out_channel) = self.resolve_out_channel(channel_id).await else {
    return;
};
let key = self.session_key_for(&ch);
let (session, _) = self.ensure_session(&key, channel_id).await;
self.enqueue_batch(channel_id, &session, &out_channel, event.incoming_message.user_name.as_str(), &content_text).await;
```

新增方法：

```rust
/// 普通消息入合批缓冲；首条消息启动延时打包任务（超时后打包为一条 user 消息进 agentic loop）
async fn enqueue_batch(
    &self,
    channel_id: &str,
    session: &Arc<Session>,
    out_channel: &OutChannel,
    user_name: &str,
    content_text: &str,
) {
    let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
    let interval = Duration::from_secs(cfg.channel_batch_interval_secs);

    let mut batch = session.batch.lock().await;
    let was_empty = batch.is_empty();
    batch.push(user_name, content_text);
    drop(batch);
    if !was_empty {
        return;  // 已有计时任务在跑，等待汇合
    }
    let gen = session.batch_gen.load(std::sync::atomic::Ordering::SeqCst);
    let session = session.clone();
    let out_channel = out_channel.clone();
    let coordinator = self.clone();
    tokio::spawn(async move {
        tokio::time::sleep(interval).await;
        let mut b = session.batch.lock().await;
        if session.batch_gen.load(std::sync::atomic::Ordering::SeqCst) != gen { return; }
        if b.is_empty() { return; }
        let items = b.take();
        drop(b);
        let content = crate::batching::pack_batch(&items);
        coordinator.run_agentic_loop(channel_id, &session, content, &out_channel).await;
    });
}
```

`handle_incoming` 中调用处传 `event.incoming_message.user_name.as_str()`。

`run_agentic_loop` 签名改为 `(channel_id: &str, session: &Arc<Session>, content_text: String, out_channel: &OutChannel)`，函数体内删除从 event 提取 content/messenger/user/group/time 的代码，直接使用 `content_text`。

- [ ] **Step 6: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过（新增 pack_batch/batch_buffer 测试 PASS；现有测试适配 run_agentic_loop 无直接调用）

- [ ] **Step 7: Commit**

```bash
git add kissbot-agent/src/
git commit -m "feat(agent): channel 合批——每会话 BatchBuffer（首条启动延时任务，超时后打包为一条 user 消息进 loop，content 逐行 name: text）；管理命令不走缓冲；Session 加 batch/batch_gen，重置时代数失效旧任务；run_agentic_loop 签名改为收 content_text"
```

---

### Task 8: memory-store 查询 API 增加 limit 与目录聚合

**Files:**
- Modify: `kissbot-api/src/memory.rs`
- Modify: `kissbot-memory/src/index.rs`
- Modify: `kissbot-memory/src/data.rs`（新增目录枚举辅助）
- Modify: `kissbot-memory/src/lib.rs`（若需要导出）
- Test: `kissbot-memory/src/index.rs` 测试模块

**Interfaces:**
- Produces: `QueryChannelRequest.limit: Option<usize>`、`QueryRequest.limit: Option<usize>`（`#[serde(default)]`）；`MemoryIndexer::query_channel_records(query)` 在 messenger_id/user_id/group_id 均为空串时走目录聚合（枚举 `<root>/<agent_id>/memory-store/<year>-<role_name>/channel-*.jsonl` + ReverseLineReader 尾部读取 + 按 (time,sn) 排序 + 时间窗过滤 + limit 截取最近 N）；非空走原精确 key 路径 + limit 截取

- [ ] **Step 1: 写失败测试（kissbot-memory/src/index.rs tests 追加）**

```rust
    static TEST_ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    static INIT_CONFIG: Once = Once::new();

    fn init_agg_config() -> std::path::PathBuf {
        let dir = TEST_ROOT.get_or_init(|| tempfile::tempdir().unwrap());
        let config_path = dir.path().join("config.json");
        let root = dir.path().join("data");
        INIT_CONFIG.call_once(|| {
            std::fs::write(&config_path, format!(r#"{{"memory": {{"root_dir": "{}"}}}}"#, root.display())).unwrap();
            unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
            MemoryConfig::get();
        });
        root
    }

    #[tokio::test]
    async fn test_query_channel_recent_with_limit_and_directory_aggregate() {
        use tokio::io::AsyncWriteExt;

        let root = init_agg_config();
        // 直接写两个 channel 文件（格式与 ChannelParser.get_path 一致），模拟既有记忆记录
        let file = |messenger: &str| {
            root.join("agg_agent").join("memory-store").join("2026-r1")
                .join(format!("channel-{}={}={}-records-2026-08-05.jsonl", messenger, "self1", "g1"))
        };
        let rec = |time: &str, text: &str, messenger: &str| serde_json::json!({
            "user_id": "u1", "is_self": 0,
            "messenger_name": "", "user_name": format!("name-{}", messenger),
            "group_name": "", "content": { "Text": text },
            "time": time, "sn": 1,
        });
        let write = |path: &std::path::Path, line: &str| async move {
            tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
            let mut f = tokio::fs::OpenOptions::new().create(true).append(true).open(path).await.unwrap();
            f.write_all(line.as_bytes()).await.unwrap();
        };
        let line = |r: serde_json::Value| format!("{}\n", r);
        write(&file("web"), &line(rec("2026-08-05 10:00:00", "m1-早", "web"))).await;
        write(&file("tg"), &line(rec("2026-08-05 10:01:00", "m2-中", "tg"))).await;
        write(&file("web"), &line(rec("2026-08-05 10:02:00", "m1-晚", "web"))).await;

        let indexer = MemoryIndexer::new();
        // 目录聚合：messenger/user/group 空串 + limit=2
        let query = QueryChannelRequest {
            agent_id: Arc::new("agg_agent".into()),
            role_name: Arc::new("r1".into()),
            messenger_id: Arc::new(String::new()),
            user_id: Arc::new(String::new()),
            group_id: Arc::new(String::new()),
            start_time: Arc::new("2026-08-05 00:00:00".into()),
            end_time: Arc::new("2026-08-05 23:59:59".into()),
            limit: Some(2),
        };
        let results = indexer.query_channel_records(query).await.unwrap();
        let mut flat: Vec<(String, String)> = Vec::new();  // (time, text)
        for (_, records) in &results {
            for (_, r) in records {
                if let Content::Text(t) = &r.content {
                    flat.push((r.time.to_string(), t.as_str().to_string()));
                }
            }
        }
        flat.sort();
        assert_eq!(flat.len(), 2, "limit=2 只返回最近 2 条");
        assert_eq!(flat[0].1, "m2-中");
        assert_eq!(flat[1].1, "m1-晚");
    }
```

（若 `kissbot-memory/Cargo.toml` 无 tempfile，在 dev-dependencies 加 `tempfile = "3"`。）

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-memory && cargo test test_query_channel_recent_with_limit_and_directory_aggregate`
Expected: 失败——`QueryChannelRequest` 无 `limit` 字段

- [ ] **Step 3: kissbot-api/src/memory.rs——加 limit 字段**

```rust
pub struct QueryChannelRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
    /// 可选：返回该范围内最近 N 条（合并排序后截取）；None 返回全部
    #[serde(default)]
    pub limit: Option<usize>,
}

pub struct QueryRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
    /// 可选：返回该范围内最近 N 条；None 返回全部
    #[serde(default)]
    pub limit: Option<usize>,
}
```

修复 `kissbot-api` 现有测试中 `QueryChannelRequest`/`QueryRequest` 构造处（`test_serde_query_channel_request`、`test_serde_query_request` 等）补 `limit: None`。

- [ ] **Step 4: kissbot-memory/src/index.rs——目录聚合 + limit 截取**

在 `MemoryIndexer` 增加方法，并把 `query_channel_records` 改为先判断是否目录聚合：

```rust
    pub async fn query_channel_records(&self, query: QueryChannelRequest) -> Result<Vec<(ChannelRecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        // 目录聚合模式：messenger/user/group 均为空串 → 按 agent+role 扫描全部 channel 文件
        if query.messenger_id.is_empty() && query.user_id.is_empty() && query.group_id.is_empty() {
            return self.query_channel_aggregate(query).await;
        }
        let mut result = self.channel_indices.query_all(query.clone()).await?;
        take_recent(&mut result, query.limit);
        Ok(result)
    }

    /// 目录聚合：枚举 <root>/<agent_id>/memory-store/<year>-<role_name>/channel-*.jsonl，
    /// 每文件 ReverseLineReader 尾部读取（上限 1024 行），合并按 (time, sn) 排序，时间窗过滤，limit 截取最近 N
    async fn query_channel_aggregate(&self, query: QueryChannelRequest) -> Result<Vec<(ChannelRecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        use kai_file::ReverseLineReader;
        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(query.agent_id.as_str()).await?;
        // 收集所有 <year>-<role_name> 目录下的 channel 文件
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&store_dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if let Some((_, role)) = dir_name.split_once('-') {
                if role == query.role_name.as_str() && entry.path().is_dir() {
                    let mut year_dir = tokio::fs::read_dir(entry.path()).await?;
                    while let Some(f) = year_dir.next_entry().await? {
                        let fname = f.file_name().to_string_lossy().to_string();
                        if fname.starts_with("channel-") && fname.ends_with(".jsonl") {
                            files.push(f.path());
                        }
                    }
                }
            }
        }
        let mut merged: Vec<(u32, Arc<ChannelRecord>)> = Vec::new();
        for path in files {
            let mut reader = ReverseLineReader::new(&path, None, None).await?;
            let mut count = 0;
            while let Some(line_with_pos) = reader.next_line().await? {
                let s = line_with_pos.line.trim();
                if s.is_empty() { continue; }
                if let Ok(rec) = serde_json::from_str::<ChannelRecord>(s) {
                    merged.push((rec.sn as u32, Arc::new(rec)));
                    count += 1;
                    if count >= 1024 { break; }
                }
            }
        }
        merged.sort_by(|a, b| {
            a.1.time.as_str().cmp(b.1.time.as_str()).then(a.1.sn.cmp(&b.1.sn))
        });
        // 时间窗过滤
        merged.retain(|(_, r)| {
            r.time.as_str() >= query.start_time.as_str() && r.time.as_str() <= query.end_time.as_str()
        });
        // limit 截取最近 N
        if let Some(limit) = query.limit {
            if merged.len() > limit {
                merged.drain(..merged.len() - limit);
            }
        }
        let key = ChannelRecordKey {
            agent_id: query.agent_id.clone(),
            role_name: query.role_name.clone(),
            messenger_id: Arc::new(String::new()),
            user_id: Arc::new(String::new()),
            group_id: Arc::new(String::new()),
            date: Arc::new(String::new()),
        };
        Ok(vec![(key, merged)])
    }
```

在 `MemoryIndexer` impl 外新增通用截取函数：

```rust
/// 精确 key 路径 + limit：每组记录按 (time, sn) 排序后截取最近 N（保持现有返回结构）
fn take_recent<K, R>(grouped: &mut Vec<(K, Vec<(u32, Arc<R>)>)>, limit: Option<usize>)
where
    K: Clone + Send + Sync,
    R: kissbot_api::memory::MemoryRecord + Send + Sync + 'static,
{
    if let Some(limit) = limit {
        for (_, records) in grouped.iter_mut() {
            records.sort_by(|a, b| {
                a.1.time().cmp(b.1.time()).then(a.1.sn().cmp(&b.1.sn()))
            });
            if records.len() > limit {
                records.drain(..records.len() - limit);
            }
        }
    }
}
```

`query_think_records` / `query_tool_call_records` / `query_tool_result_records` 各自在 `query_all` 后调用 `take_recent(&mut result, query.limit)`（`QueryRequest.limit`）。

- [ ] **Step 5: kissbot-memory-store/src/api.rs——透传（无改动）**

`QueryChannelRequest`/`QueryRequest` 已含 limit，反序列化自动带入。确认编译：`cd kissbot-memory-store && cargo build`。

- [ ] **Step 6: 运行测试**

Run: `cd kissbot-memory && cargo test`
Expected: 新增目录聚合测试 PASS，现有测试 PASS（构造补了 limit 字段）
Run: `cd kissbot-api && cargo test`
Expected: PASS
Run: `cd kissbot-agent && cargo build`
Expected: 编译通过（QueryChannelRequest 构造处补 limit: None——`memory_reader.rs` 有构造，Task 9 会重构，此处先补字段）

- [ ] **Step 7: Commit**

```bash
git add kissbot-api/src/memory.rs kissbot-memory/src/index.rs kissbot-memory/Cargo.toml kissbot-agent/src/memory_reader.rs
git commit -m "feat(memory): 查询 API 增加可选 limit（最近 N 条）——QueryChannelRequest/QueryRequest 加 limit 字段；channel 查询支持目录聚合（messenger/user/group 空串时枚举 role 目录下 channel 文件合并排序取最近 N）；精确 key 路径同样截取"
```

---

### Task 9: 记忆打包（memory_reader 重构 + 两查询比较）

**Files:**
- Modify: `kissbot-agent/src/memory_reader.rs`

**Interfaces:**
- Consumes: `QueryChannelRequest.limit`（Task 8）、`EffectiveContextConfig { memory_time_secs, memory_count }`、`Message`
- Produces: `memory_reader::{MemoryMsg { user_name, content, time }, window_wins(Option<&str>, Option<&str>) -> bool, pack_memory_messages(&[MemoryMsg]) -> Option<Message>}`；`MemoryReader::read_recent_for_context(agent_id, role_name, cfg) -> Result<Vec<MemoryMsg>>`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_wins_compares_first_record_time() {
        // 窗口首条更早 → true（窗口更大，用窗口）
        assert!(window_wins(Some("2026-08-05 09:00:00"), Some("2026-08-05 10:00:00")));
        // recent 首条更早 → false（recent 更大，用 recent）
        assert!(!window_wins(Some("2026-08-05 10:00:00"), Some("2026-08-05 09:00:00")));
        // 相等 → 窗口
        assert!(window_wins(Some("2026-08-05 10:00:00"), Some("2026-08-05 10:00:00")));
        // 一侧为空：窗口空 → 用 recent；recent 空 → 用窗口；都空 → 窗口（外部处理为空）
        assert!(!window_wins(None, Some("2026-08-05 10:00:00")));
        assert!(window_wins(Some("2026-08-05 10:00:00"), None));
        assert!(window_wins(None, None));
    }

    #[test]
    fn pack_memory_messages_builds_user_message() {
        let msgs = vec![
            MemoryMsg { user_name: "u1".into(), content: "你好".into(), time: "t1".into() },
            MemoryMsg { user_name: String::new(), content: "无名字".into(), time: "t2".into() },
        ];
        let m = pack_memory_messages(&msgs).expect("非空应打包");
        assert!(matches!(&m, Message::User { content } if content.as_str() == "u1: 你好\n无名字"));
    }

    #[test]
    fn pack_memory_messages_empty_returns_none() {
        assert!(pack_memory_messages(&[]).is_none());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test window_wins_compares_first_record_time`
Expected: 编译失败——`window_wins` 未定义

- [ ] **Step 3: 重构 memory_reader.rs**

整体替换为：

```rust
use serde_json::json;

use crate::config_manager::{ConfigManager, EffectiveContextConfig};
use crate::types::{Message, Mode, Result, Error};

/// 记忆消息（channel record 的最小视图：name + content，id 类不保留）
#[derive(Debug, Clone)]
pub struct MemoryMsg {
    pub user_name: String,
    pub content: String,
    pub time: String,
}

/// 两查询比较决策：窗口首条更早 → true（窗口更大，用窗口）；recent 首条更早 → false（用 recent）
/// None 视为最晚（空集合：另一侧胜出）；两侧都空返回 true（外部处理为空）
pub fn window_wins(window_first: Option<&str>, recent_first: Option<&str>) -> bool {
    match (window_first, recent_first) {
        (Some(w), Some(r)) => w <= r,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => true,
    }
}

/// 打包记忆消息为一条 user 消息（content 逐行 "name: text"）；空返回 None
pub fn pack_memory_messages(msgs: &[MemoryMsg]) -> Option<Message> {
    if msgs.is_empty() {
        return None;
    }
    let content = msgs.iter().map(|m| {
        if m.user_name.is_empty() { m.content.clone() } else { format!("{}: {}", m.user_name, m.content) }
    }).collect::<Vec<_>>().join("\n");
    Some(Message::User { content: Arc::new(content) })
}

pub struct MemoryReader {
    client: reqwest::Client,
}

impl MemoryReader {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    /// 时间窗查询（messenger/user/group 空 = 目录聚合）；返回升序记录
    async fn query_channel_range(
        &self,
        agent_id: &str,
        role_name: &str,
        start_time: &str,
        end_time: &str,
    ) -> Result<Vec<MemoryMsg>> {
        self.query_channel(agent_id, role_name, start_time, end_time, None).await
    }

    /// 最近 N 条
    async fn query_channel_recent(
        &self,
        agent_id: &str,
        role_name: &str,
        limit: usize,
    ) -> Result<Vec<MemoryMsg>> {
        self.query_channel(agent_id, role_name, "2000-01-01 00:00:00", "2099-12-31 23:59:59", Some(limit)).await
    }

    async fn query_channel(
        &self,
        agent_id: &str,
        role_name: &str,
        start_time: &str,
        end_time: &str,
        limit: Option<usize>,
    ) -> Result<Vec<MemoryMsg>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/channel", store_url.trim_end_matches('/'));
        let mut body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
            "messenger_id": "",
            "user_id": "",
            "group_id": "",
            "start_time": start_time,
            "end_time": end_time,
        });
        if let Some(l) = limit {
            body["limit"] = json!(l);
        }
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("记忆读取返回 {}", resp.status())));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(self.parse_channel_records(&data["data"]))
    }

    /// 解析 ApiResponse.data：Vec<(key, Vec<(sn, ChannelRecord)>)>
    fn parse_channel_records(&self, data: &serde_json::Value) -> Vec<MemoryMsg> {
        let mut out = Vec::new();
        if let Some(groups) = data.as_array() {
            for group in groups {
                if let Some(records) = group[1].as_array() {
                    for entry in records {
                        let rec = &entry[1];
                        let time = rec["time"].as_str().unwrap_or("").to_string();
                        let user_name = rec["user_name"].as_str().unwrap_or("").to_string();
                        let content = extract_record_text(&rec["content"]);
                        if content.is_empty() {
                            continue;  // 非文本记录跳过
                        }
                        out.push(MemoryMsg { user_name, content, time });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.time.cmp(&b.time));
        out
    }

    /// 两查询（时间窗 + 最近 N）比较首条时间取更早者，返回升序结果
    pub async fn read_recent_for_context(
        &self,
        agent_id: &str,
        role_name: &str,
        cfg: &EffectiveContextConfig,
    ) -> Result<Vec<MemoryMsg>> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let start = chrono::Local::now()
            .checked_sub_signed(chrono::Duration::seconds(cfg.memory_time_secs as i64))
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "2000-01-01 00:00:00".to_string());

        let window = self.query_channel_range(agent_id, role_name, &start, &now).await?;
        let recent = self.query_channel_recent(agent_id, role_name, cfg.memory_count).await?;

        let window_first = window.first().map(|m| m.time.as_str());
        let recent_first = recent.first().map(|m| m.time.as_str());
        Ok(if window_wins(window_first, recent_first) { window } else { recent })
    }
}

/// 从 Content 枚举 JSON（{"Text": "...", "Multi": [...]}）提取文本
fn extract_record_text(content: &serde_json::Value) -> String {
    match content.get("Text") {
        Some(t) => t.as_str().unwrap_or("").to_string(),
        None => match content.get("Multi") {
            Some(arr) => arr.as_array().map(|items| items.iter()
                .filter_map(|c| c.get("Text").and_then(|t| t.as_str()).map(String::from))
                .collect::<Vec<_>>().join("\n")).unwrap_or_default(),
            None => String::new(),
        },
    }
}
```

保留 `read_memory_struct_index` 与 `list_events`（原样），删除 `read_history` 与旧 `records_to_messages`。`read_memory_struct_index`/`list_events` 引用的 `ConfigManager` 参数保留。删除 `MAX_RECENT_RECORDS` 常量。

`coordinator.rs` 的 `build_initial_context` role 分支仍调用 `read_history`——改为调用 `self.memory_reader.read_recent_for_context(...)` + `pack_memory_messages`：

```rust
Mode::Role => {
    let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
    if let Ok(msgs) = self.memory_reader
        .read_recent_for_context(session.agent_id.as_str(), session.role_name.as_str(), &cfg)
        .await
    {
        if let Some(packed) = memory_reader::pack_memory_messages(&msgs) {
            session.context.lock().await.push(packed);
        }
    }
}
```

（memory_reader 模块函数需 pub；coordinator 顶部 import 调整。）

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 记忆打包三个测试 PASS；全部现有测试通过

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/memory_reader.rs kissbot-agent/src/coordinator.rs
git commit -m "feat(agent): 记忆打包——MemoryReader 重构（目录聚合查询 + 时间窗/最近N 两查询比较首条时间取更早者），打包为一条 user 消息（name: text 行）；read_history 删除，build_initial_context role 分支改走记忆打包"
```

---

### Task 10: 重置/压缩完整流程（event 压缩 + role 归档重建）

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: `EffectiveContextConfig.compress_prompt`、`ModelClient::call`、`ContextCache`、`HistoryArchive`、`MemoryReader::read_recent_for_context`/`pack_memory_messages`
- Produces: `AgentCoordinator::{reset_context（按 mode 归档+清空+重建）, compress_context(session)（event 超长：归档→LLM 总结→重写 system+user(压缩指令)+assistant(总结)）}`；`run_agentic_loop` 溢出分支按 mode 调 reset 或 compress

- [ ] **Step 1: 实现 compress_context 与 reset 分支**

`reset_context` 替换为：

```rust
/// 上下文重置（新 session_key 或超长）：按模式归档当前缓存 → 清空 → 重建
/// event：归档（超长时调用方先 compress，此处仅归档+清空+重建空白缓存）
/// role：归档 + 记忆打包重建
async fn reset_context(&self, session: &Arc<Session>) {
    let key = self.session_key_of_session(session);
    let path = self.cache.path_for(&key);
    if path.exists() {
        let _ = self.history.archive(&key, &path).await;
    }
    let _ = self.cache.clear(&key).await;
    // 合批缓冲清空 + 代数递增（失效旧计时任务）
    {
        let mut b = session.batch.lock().await;
        b.clear();
        session.batch_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    session.context.lock().await.clear();
    self.build_initial_context(session).await;
    info!("会话上下文已重置: role={} mode={:?}", session.role_name, session.mode);
}
```

`build_initial_context` 的 role 分支用 Task 9 的记忆打包，并补「重新进入时归档旧缓存」（role 重新进入/重建路径：缓存若有内容先归档再清空，防重复累积）：

```rust
Mode::Role => {
    let key = self.session_key_of_session(session);
    let path = self.cache.path_for(&key);
    if path.exists() {
        // 重新进入既有 role 会话：旧上下文先归档为历史（重建后缓存将被清空重写）
        let _ = self.history.archive(&key, &path).await;
        let _ = self.cache.clear(&key).await;
    }
    let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
    if let Ok(msgs) = self.memory_reader
        .read_recent_for_context(session.agent_id.as_str(), session.role_name.as_str(), &cfg)
        .await
    {
        if let Some(packed) = memory_reader::pack_memory_messages(&msgs) {
            session.context.lock().await.push(packed);
        }
    }
}
```

> 说明：`reset_context` 已先归档+清空，再调 `build_initial_context`，此时缓存不存在不会重复归档；而重新进入路径（`ensure_session` → `build_initial_context`）缓存存在，由本分支归档。

新增压缩方法：

```rust
/// event 模式超长压缩：归档当前缓存 → LLM 总结（compress_prompt + 当前上下文）→
/// 重写缓存为 system + user(压缩指令) + assistant(总结)，等待后续 channel 消息
async fn compress_context(&self, session: &Arc<Session>) {
    let key = self.session_key_of_session(session);
    let path = self.cache.path_for(&key);
    if path.exists() {
        let _ = self.history.archive(&key, &path).await;
    }
    let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
    // 1. 取当前完整上下文（含 system），末尾追加压缩指令 user 消息
    let messages = {
        let mut ctx = session.context.lock().await;
        let mut msgs = ctx.build();
        msgs.push(Message::User { content: Arc::new(cfg.compress_prompt.clone()) });
        msgs
    };
    // 2. 调会话模型总结（Task 13 后 call 增加 tools 参数，同步改为三参）
    let summary = {
        let model = session.model.load_full();
        let Some(pm) = model.as_ref() else { return; };
        let mc = self.model_client.lock().await;
        mc.call(pm, &messages).await.map(|r| r.content).unwrap_or_default()
    };
    if summary.is_empty() {
        warn!("上下文压缩总结为空，保留原上下文");
        return;
    }
    // 3. 重建：清空内存（system 保留）→ user(压缩指令) + assistant(总结) → 缓存重写
    {
        let mut ctx = session.context.lock().await;
        ctx.clear();
        ctx.push(Message::User { content: Arc::new(cfg.compress_prompt.clone()) });
        ctx.push(Message::Assistant { content: Arc::new(summary), reasoning_content: None, tool_calls: None });
    }
    let _ = self.cache.clear(&key).await;
    let msgs = { session.context.lock().await.build() };
    // 缓存不含 system：只存 user+assistant（恢复时 set_system 在前）
    let store: Vec<Message> = msgs.into_iter().filter(|m| !matches!(m, Message::System { .. })).collect();
    self.cache.append(&key, &store).await;
    info!("会话上下文已压缩: role={} mode={:?}", session.role_name, session.mode);
}
```

`run_agentic_loop` 溢出分支改为按 mode：

```rust
if overflow {
    warn!("会话上下文超长，触发重置: role={} mode={:?}", session.role_name, session.mode);
    match &*session.mode {
        Mode::Event(_) => self.compress_context(session).await,
        Mode::Role => self.reset_context(session).await,
    }
}
```

- [ ] **Step 2: 写压缩上下文构造的单元测试（coordinator.rs tests 追加）**

压缩上下文构造逻辑抽为纯函数便于测试（放 coordinator.rs）：

```rust
/// 压缩后上下文（不含 system）：user(压缩指令) + assistant(总结)
fn compressed_messages(cfg: &EffectiveContextConfig, summary: &str) -> Vec<Message> {
    vec![
        Message::User { content: Arc::new(cfg.compress_prompt.clone()) },
        Message::Assistant { content: Arc::new(summary.to_string()), reasoning_content: None, tool_calls: None },
    ]
}

#[test]
fn compress_builds_prompt_summary_sequence() {
    let cfg = EffectiveContextConfig {
        channel_batch_interval_secs: 3,
        memory_time_secs: 3600,
        memory_count: 50,
        compress_prompt: "总结以上对话".into(),
        stations: std::collections::HashSet::new(),
    };
    let msgs = compressed_messages(&cfg, "总结内容");
    assert_eq!(msgs.len(), 2);
    assert!(matches!(&msgs[0], Message::User { content } if content.as_str() == "总结以上对话"));
    assert!(matches!(&msgs[1], Message::Assistant { content, .. } if content.as_str() == "总结内容"));
}
```

（`EffectiveContextConfig` 需可从 coordinator 测试访问——`use crate::context_config::EffectiveContextConfig;`）

- [ ] **Step 3: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过（新增 compress 测试 PASS）

- [ ] **Step 4: Commit**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "feat(agent): 重置/压缩完整流程——reset_context 按模式归档+清空合批+重建（event 缓存恢复/role 记忆打包）；event 超长走 compress_context（归档→LLM 总结→重写 system+user压缩指令+assistant总结，缓存只存非 system 消息）"
```

---

### Task 11: Station 框架（Tool trait + StationRuntime + Read 工具）

**Files:**
- Create: `kissbot-agent/src/station.rs`
- Modify: `kissbot-agent/src/main.rs`

**Interfaces:**
- Produces: `station::{Tool (async_trait: call(&self, Value) -> Result<Value>), ReadTool { new(cwd: PathBuf), resolve_safe_path(&self, &str) -> Result<PathBuf> }, StationRuntime { new(config: Arc<StationConfig>), register_local(name: &str, Arc<dyn Tool>), call_tool(&self, &str, Value) -> Result<Value> }}`
- Consumes: `StationConfig`（Task 12 前先用最小字段构造/或仅用 base_url/timeout——为解耦，`StationRuntime::new` 只接收 `base_url: String, timeout_secs: u64`，Task 12 再改为接收 Arc<StationConfig>）

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_tool_rejects_escape_paths() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadTool::new(dir.path().to_path_buf());
        // 子目录内绝对路径 OK
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.txt"), "hi").unwrap();
        assert!(tool.resolve_safe_path(sub.join("a.txt").to_str().unwrap()).is_ok());
        // 相对路径基于 cwd 解析
        assert!(tool.resolve_safe_path("sub/a.txt").is_ok());
        // 越界：上级目录
        assert!(tool.resolve_safe_path("../outside.txt").is_err(), ".. 穿透应拒绝");
        // 越界：cwd 之外的绝对路径
        let outside = tempfile::tempdir().unwrap();
        assert!(tool.resolve_safe_path(outside.path().to_str().unwrap()).is_err(), "cwd 外应拒绝");
        // 不存在的路径：父目录在 cwd 内仍应通过校验（读取时自然报不存在）
        assert!(tool.resolve_safe_path("sub/missing.txt").is_ok());
    }

    #[tokio::test]
    async fn read_tool_reads_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "文件内容").unwrap();
        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool.call(serde_json::json!({ "path": "a.txt" })).await.unwrap();
        assert_eq!(result, serde_json::json!({ "content": "文件内容" }));
    }

    #[tokio::test]
    async fn station_runtime_local_call() {
        let runtime = StationRuntime::new(String::new(), 5);
        runtime.register_local("echo", Arc::new(EchoTool));
        let out = runtime.call_tool("echo", serde_json::json!({"v": 1})).await.unwrap();
        assert_eq!(out["v"], 1);
        // 未注册工具报错
        assert!(runtime.call_tool("nope", serde_json::json!({})).await.is_err());
        // base_url 非空 → REST 骨架（未实现错误）
        let remote = StationRuntime::new("http://127.0.0.1:1".to_string(), 5);
        assert!(remote.call_tool("any", serde_json::json!({})).await.is_err());
    }
}

/// 测试用 mock tool
struct EchoTool;
#[async_trait]
impl Tool for EchoTool {
    async fn call(&self, params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(params)
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test read_tool_rejects_escape_paths`
Expected: 编译失败——station 模块不存在

- [ ] **Step 3: 实现 station.rs**

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;

use crate::types::{Error, Result};

/// 工具统一接口：统一参数（serde_json::Value）与返回值（serde_json::Value）
#[async_trait]
pub trait Tool: Send + Sync {
    async fn call(&self, params: Value) -> Result<Value>;
}

// ========== 内置示例工具：Read（读文本文件，路径校验防穿透） ==========

pub struct ReadTool {
    cwd: PathBuf,
}

/// Read 工具返回内容的最大字节数（截断防大文件）
const READ_MAX_BYTES: usize = 64 * 1024;

impl ReadTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    /// 路径校验：相对路径基于 cwd 解析 → canonicalize（消解 .. 与符号链接）→
    /// 校验规范绝对路径等于 cwd 或在 cwd 子目录内（在规范化后的绝对路径上判断，防穿透）
    pub fn resolve_safe_path(&self, raw: &str) -> Result<PathBuf> {
        let path = Path::new(raw);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        // 目标不存在时 canonicalize 失败：先规范化父目录再拼接文件名
        let canon = std::fs::canonicalize(&absolute).unwrap_or_else(|_| {
            absolute.parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .map(|p| p.join(absolute.file_name().unwrap_or_default()))
                .unwrap_or(absolute.clone())
        });
        let canon_cwd = std::fs::canonicalize(&self.cwd)
            .unwrap_or_else(|_| self.cwd.clone());
        if !canon.starts_with(&canon_cwd) {
            return Err(Error::InternalError(format!("路径越界: {}", raw)));
        }
        Ok(canon)
    }
}

#[async_trait]
impl Tool for ReadTool {
    async fn call(&self, params: Value) -> Result<Value> {
        let raw = params.get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| Error::InternalError("缺少参数 path".to_string()))?;
        let safe = self.resolve_safe_path(raw)?;
        let content = tokio::fs::read(&safe).await
            .map_err(|e| Error::IoError(format!("读取文件失败 {}: {}", safe.display(), e)))?;
        let text = String::from_utf8_lossy(&content[..content.len().min(READ_MAX_BYTES)]).to_string();
        Ok(Value::String(text))
    }
}

// ========== Station 运行态 ==========

/// Station 运行态：base_url 非空 = REST 调用（本轮骨架）；为空 = 本地调用（查 local_tools 执行）
pub struct StationRuntime {
    base_url: String,
    timeout_secs: u64,
    local_tools: DashMap<String, Arc<dyn Tool>>,
    client: reqwest::Client,
}

impl StationRuntime {
    pub fn new(base_url: String, timeout_secs: u64) -> Self {
        Self {
            base_url,
            timeout_secs,
            local_tools: DashMap::new(),
            client: reqwest::Client::new(),
        }
    }

    /// 注册本地工具实现（base_url 为空的 station 用）
    pub fn register_local(&self, name: &str, tool: Arc<dyn Tool>) {
        self.local_tools.insert(name.to_string(), tool);
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.local_tools.contains_key(name) || !self.base_url.is_empty()
    }

    /// 执行工具：base_url 空 → 本地查表执行；非空 → REST（本轮骨架，返回未实现错误）
    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value> {
        if self.base_url.is_empty() {
            let tool = self.local_tools.get(name)
                .ok_or_else(|| Error::InternalError(format!("本地工具不存在: {}", name)))?;
            return tool.call(params).await;
        }
        // REST 分支：本轮不实现（后续接入远程 station 后端）
        Err(Error::InternalError(format!("远程 Station 调用未实现（本轮仅本地模式）: {}", name)))
    }
}
```

`main.rs` 加 `mod station;`。

> 注：`StationRuntime::new` 先用 `(base_url, timeout_secs)`，Task 12 改收 `Arc<StationConfig>`（含 tools 配置）。

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-agent && cargo test station`
Expected: 3 个测试 PASS

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/station.rs kissbot-agent/src/main.rs
git commit -m "feat(agent): Station 框架——Tool trait（统一 Value 参数/返回值）、Read 内置工具（绝对路径规范化后 cwd 前缀校验防穿透、64KB 截断）、StationRuntime（base_url 空=本地查表执行/非空=REST 骨架未实现）"
```

---

### Task 12: StationConfig 增加 tools 配置

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`
- Modify: `kissbot-agent/src/station.rs`（StationRuntime 改收 Arc<StationConfig>）

**Interfaces:**
- Produces: `config_manager::{StationConfig { station_id, base_url, timeout_secs, tools: Arc<ArcSwapHashMap<String, ToolConfig>> }, ToolConfig { name: Arc<String>, description: Arc<String>, parameters: Arc<Value> }}`；`ConfigManager::stations()` 已有（返回 Arc<StationConfig> 快照）；`StationRuntime::new(config: Arc<StationConfig>)`

- [ ] **Step 1: 写失败测试（config_manager.rs tests 追加）**

```rust
#[test]
fn station_config_tools_roundtrip() {
    let sc = StationConfig {
        station_id: Arc::new("local".into()),
        base_url: Arc::new(String::new()),
        timeout_secs: 5,
        tools: Arc::new(ArcSwapHashMap::new()),
    };
    let json = serde_json::to_string(&sc).unwrap();
    let back: StationConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(*back.station_id, "local");
    assert!(back.tools.is_empty());

    // ToolConfig 序列化
    let tc = ToolConfig {
        name: Arc::new("read".into()),
        description: Arc::new("读取文本文件".into()),
        parameters: Arc::new(serde_json::json!({ "type": "object", "properties": { "path": { "type": "string" } } })),
    };
    let tj = serde_json::to_value(&tc).unwrap();
    assert_eq!(tj["name"], "read");
    assert_eq!(tj["parameters"]["properties"]["path"]["type"], "string");
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test station_config_tools_roundtrip`
Expected: 编译失败——`StationConfig` 无 `tools`、`ToolConfig` 未定义

- [ ] **Step 3: 实现**

`config_manager.rs`：

```rust
/// 工具配置（StationConfig.tools 的 value；name 与 map key 一致）
/// 字段按编码规范用 Arc<String>/Arc<Value>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub name: Arc<String>,
    pub description: Arc<String>,
    /// JSON Schema（OpenAI tools[].function.parameters）
    pub parameters: Arc<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationConfig {
    pub station_id: Arc<String>,
    /// 非空 = REST 调用；为空 = 本地调用
    pub base_url: Arc<String>,
    pub timeout_secs: u64,
    /// 工具列表（key = 工具名）
    pub tools: Arc<ArcSwapHashMap<String, ToolConfig>>,
}
```

现有 `StationConfig` 构造点（若有测试/代码）补 `tools` 字段。

`station.rs`——`StationRuntime` 改收 `Arc<StationConfig>`：

```rust
use crate::config_manager::StationConfig;

pub struct StationRuntime {
    config: Arc<StationConfig>,
    local_tools: DashMap<String, Arc<dyn Tool>>,
    client: reqwest::Client,
}

impl StationRuntime {
    pub fn new(config: Arc<StationConfig>) -> Self {
        Self { config, local_tools: DashMap::new(), client: reqwest::Client::new() }
    }

    pub fn station_id(&self) -> &str { self.config.station_id.as_str() }

    /// 配置访问器（coordinator 判断 base_url 用）
    pub fn config(&self) -> &StationConfig { &self.config }

    pub fn register_local(&self, name: &str, tool: Arc<dyn Tool>) {
        self.local_tools.insert(name.to_string(), tool);
    }

    /// 该 station 配置的工具名集合（LLM tools 聚合用）
    pub fn configured_tools(&self) -> Vec<ToolConfig> {
        self.config.tools.iter().map(|(_, s)| s.load_full().clone()).collect()
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.local_tools.contains_key(name) || self.config.tools.contains_key(name)
    }

    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value> {
        if self.config.base_url.is_empty() {
            let tool = self.local_tools.get(name)
                .ok_or_else(|| Error::InternalError(format!("本地工具不存在: {}", name)))?;
            return tool.call(params).await;
        }
        Err(Error::InternalError(format!("远程 Station 调用未实现（本轮仅本地模式）: {}", name)))
    }
}
```

`station.rs` 测试更新：`StationRuntime::new` 构造用 `Arc::new(StationConfig { ... })`（tools 空）。`station.rs` 顶部 import `use crate::config_manager::{StationConfig, ToolConfig};`。

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过（station_config_tools_roundtrip + station 三个测试适配后 PASS）

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/
git commit -m "feat(agent): StationConfig 增加 tools（Map<工具名, ToolConfig{name,description,parameters JSON Schema}>）；StationRuntime 改持 Arc<StationConfig>，提供 configured_tools/has_tool，本地/REST 分支按 base_url 判定"
```

---

### Task 13: Agentic Loop 多轮 + tools 聚合 + provider tools wire

**Files:**
- Modify: `kissbot-agent/src/provider.rs`（send 加 tools 参数、openai_body 发 tools 数组）
- Modify: `kissbot-agent/src/model_client.rs`（call 加 tools 参数）
- Modify: `kissbot-agent/src/coordinator.rs`（tools 聚合、多轮循环、工具路由执行、Tool 消息、push_tool_call/result、溢出检查）
- Modify: `kissbot-agent/src/types.rs`（ModelResponse 无变化）

**Interfaces:**
- Consumes: `ToolConfig`、`StationRuntime`、`Message`、`ToolCall`、`push_tool_call/push_tool_result`（memory_store_client 已有）
- Produces: `Provider::send(effective, &[Message], &[ToolConfig])`；`ModelClient::call(pm, &[Message], &[ToolConfig])`；`AgentCoordinator::{station_runtimes: DashMap<String, Arc<StationRuntime>>, tools_for_session(&Session) -> Vec<ToolConfig>, execute_tool_call(&Session, &ToolCall) -> Value, run_agentic_loop 多轮}`

- [ ] **Step 1: provider.rs——send 加 tools 参数 + openai_body 发 tools**

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[Message], tools: &[ToolConfig]) -> Result<ModelResponse>;
    // ...list_models / provider_type 不变
}
```

`openai_body(effective, messages, tools)`：

```rust
fn openai_body(effective: &EffectiveModelConfig, messages: &[Message], tools: &[ToolConfig]) -> serde_json::Value {
    // ...原有 msgs 构造不变...
    let mut body = json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
        "stream": false,
    });
    // tools：非空才发送
    if !tools.is_empty() {
        body["tools"] = json!(tools.iter().map(|t| json!({
            "type": "function",
            "function": { "name": t.name, "description": t.description, "parameters": t.parameters },
        })).collect::<Vec<_>>());
    }
    // ...temperature/thinking/reasoning_effort 不变...
    body
}
```

`OpenAiProvider::send` 与 `AnthropicProvider::send` 签名同步加 `tools: &[ToolConfig]`；`anthropic_body(effective, messages, _tools)` 忽略 tools。

`provider.rs` 顶部 import：`use crate::config_manager::{EffectiveModelConfig, ToolConfig};`（provider.rs 现有 `use crate::config_manager::EffectiveModelConfig;` 改为加 ToolConfig）。

- [ ] **Step 2: provider.rs 测试——tools 数组**

```rust
#[test]
fn openai_body_includes_tools_when_present() {
    let eff = sample_effective();
    let msgs = vec![Message::User { content: Arc::new("查一下".into()) }];
    let tools = vec![ToolConfig {
        name: Arc::new("read".into()),
        description: Arc::new("读取文本文件".into()),
        parameters: Arc::new(serde_json::json!({ "type": "object" })),
    }];
    let body = openai_body(&eff, &msgs, &tools);
    assert_eq!(body["tools"][0]["function"]["name"], "read");
    assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
}

#[test]
fn openai_body_omits_tools_when_empty() {
    let eff = sample_effective();
    let msgs = vec![Message::User { content: Arc::new("你好".into()) }];
    let body = openai_body(&eff, &msgs, &[]);
    assert!(body.get("tools").is_none(), "无工具不应发送 tools 字段");
}

#[test]
fn openai_body_maps_tool_and_assistant_tool_calls() {
    let eff = sample_effective();
    let msgs = vec![
        Message::Assistant {
            content: Arc::new(String::new()),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCall { id: Arc::new("c1".into()), name: Arc::new("read".into()), arguments: Arc::new(serde_json::json!({"path": "/a"})) }]),
        },
        Message::Tool { tool_call_id: Arc::new("c1".into()), name: Arc::new("read".into()), content: Arc::new("内容".into()) },
    ];
    let body = openai_body(&eff, &msgs, &[]);
    assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "c1");
    assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["name"], "read");
    assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["arguments"], r#"{"path":"/a"}"#, "arguments 序列化为 JSON 字符串");
    assert_eq!(body["messages"][1]["role"], "tool");
    assert_eq!(body["messages"][1]["tool_call_id"], "c1");
    // reasoning_content 不发送
    assert!(body["messages"][0].get("reasoning_content").is_none());
}

#[test]
fn parse_openai_response_extracts_tool_calls() {
    let data = serde_json::json!({
        "choices": [{
            "message": { "content": null, "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{\"path\":\"/a\"}" } }] },
            "finish_reason": "tool_calls"
        }]
    });
    let resp = parse_openai_response(&data);
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "c1");
    assert_eq!(resp.tool_calls[0].name, "read");
    assert_eq!(resp.tool_calls[0].arguments["path"], "/a");
    assert_eq!(resp.finish_reason, "tool_calls");
}
```

> 现有 provider 测试里所有 `openai_body(&eff, &msgs)` / `anthropic_body(&eff, &msgs)` 调用点补第三个参数 `&[]`。

- [ ] **Step 3: model_client.rs——call 加 tools**

```rust
use crate::config_manager::{ConfigManager, EffectiveModelConfig, ProviderModel, ToolConfig};

pub async fn call(&self, pm: &ProviderModel, messages: &[Message], tools: &[ToolConfig]) -> Result<ModelResponse> {
    let effective = self.config_manager.resolve_effective_config(pm).await
        .ok_or_else(|| Error::ModelProviderNotSupported(format!(
            "provider/model 不存在: {}/{}", pm.provider, pm.model)))?;
    let provider: Box<dyn Provider> = self.build_provider(&effective)?;
    self.call_with_retry(&effective, provider, messages, tools).await
}

async fn call_with_retry(
    &self,
    effective: &EffectiveModelConfig,
    provider: Box<dyn Provider>,
    messages: &[Message],
    tools: &[ToolConfig],
) -> Result<ModelResponse> {
    // 函数体不变，仅签名与 provider.send(effective, messages, tools) 调用
}
```

- [ ] **Step 4: coordinator.rs——station runtimes + tools 聚合 + 多轮 loop**

`AgentCoordinator` 加字段：

```rust
    /// station_id → StationRuntime（启动时按配置构建；base_url 空的注册内置 Read 工具）
    station_runtimes: Arc<DashMap<String, Arc<StationRuntime>>>,
```

`new()` 初始化（在 channel 绑定循环之前）：

```rust
// 构建 Station 运行态：base_url 为空的本地 station 注册内置 Read 工具
{
    let runtimes = coordinator.station_runtimes.clone();
    for (_, sc) in config.stations().await {
        let runtime = Arc::new(StationRuntime::new(sc));
        if runtime.config().base_url.is_empty() {
            let cwd = std::env::current_dir().unwrap_or_default();
            runtime.register_local("read", Arc::new(station::ReadTool::new(cwd)));
        }
        runtimes.insert(runtime.station_id().to_string(), runtime);
    }
}
```

> `StationRuntime` 需暴露 `config()` 访问器：`pub fn config(&self) -> &StationConfig { &self.config }`。

`coordinator.rs` 顶部 import 加 `use crate::config_manager::ToolConfig;`、`use crate::station::{self, StationRuntime};`。

新增方法：

```rust
/// 会话可用工具：context 配置的 stations ∩ 实际配置的 station → 收集 ToolConfig
async fn tools_for_session(&self, session: &Arc<Session>) -> Vec<ToolConfig> {
    let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
    let mut tools = Vec::new();
    for (station_id, runtime) in self.station_runtimes.iter() {
        if cfg.stations.contains(station_id.as_str()) {
            tools.extend(runtime.configured_tools());
        }
    }
    tools
}

/// 执行单个 tool call：在启用的 station 中查找并调用；找不到返回错误 JSON
async fn execute_tool_call(&self, session: &Arc<Session>, call: &crate::types::ToolCall) -> serde_json::Value {
    let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
    for (station_id, runtime) in self.station_runtimes.iter() {
        if cfg.stations.contains(station_id.as_str()) && runtime.has_tool(call.name.as_str()) {
            match runtime.call_tool(call.name.as_str(), (*call.arguments).clone()).await {
                Ok(v) => return v,
                Err(e) => return serde_json::json!({ "error": e.to_string() }),
            }
        }
    }
    serde_json::json!({ "error": format!("工具不存在: {}", call.name) })
}
```

`run_agentic_loop` 整体替换为多轮版本：

```rust
/// Agentic Loop：多轮工具调用循环（上限 MAX_TOOL_ROUNDS 防死循环）
const MAX_TOOL_ROUNDS: usize = 10;

async fn run_agentic_loop(&self, _channel_id: &str, session: &Arc<Session>, content_text: String, out_channel: &OutChannel) {
    // 无可用模型：静默忽略普通消息（仅管理指令可用）
    if session.model.load().is_none() {
        return;
    }

    let key = self.session_key_of_session(session);

    // 1. 追加用户消息 + 写缓存
    {
        let mut ctx = session.context.lock().await;
        ctx.push(Message::User { content: Arc::new(content_text.clone()) });
    }
    self.cache.append(&key, &[Message::User { content: Arc::new(content_text.clone()) }]).await;

    // 2. tools 聚合（会话 context 配置的启用 station）
    let tools = self.tools_for_session(session).await;

    // 3. 多轮循环
    let mut rounds = 0;
    loop {
        rounds += 1;
        let response = {
            let ctx = session.context.lock().await;
            let messages = ctx.build();
            let model = session.model.load_full();
            let Some(pm) = model.as_ref() else { return; };
            let mc = self.model_client.lock().await;
            mc.call(pm, &messages, &tools).await
        };

        match response {
            Ok(model_resp) if !model_resp.tool_calls.is_empty() && rounds <= MAX_TOOL_ROUNDS => {
                // 4. 追加 assistant(tool_calls) + 写缓存
                {
                    let mut ctx = session.context.lock().await;
                    ctx.push(Message::Assistant {
                        content: Arc::new(String::new()),
                        reasoning_content: None,
                        tool_calls: Some(model_resp.tool_calls.clone()),
                    });
                }
                self.cache.append(&key, &[Message::Assistant {
                    content: Arc::new(String::new()),
                    reasoning_content: None,
                    tool_calls: Some(model_resp.tool_calls.clone()),
                }]).await;

                // 5. 逐个执行 tool call → Tool 消息 + 记忆写入
                let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                let role_name = memory_role(session.role_name.as_str(), &session.mode);
                let agent_id = session.agent_id.clone();
                for call in &model_resp.tool_calls {
                    let result = self.execute_tool_call(session, call).await;
                    let result_text = result.to_string();
                    {
                        let mut ctx = session.context.lock().await;
                        ctx.push(Message::Tool { tool_call_id: call.id.clone(), name: call.name.clone(), content: Arc::new(result_text.clone()) });
                    }
                    self.cache.append(&key, &[Message::Tool { tool_call_id: call.id.clone(), name: call.name.clone(), content: Arc::new(result_text.clone()) }]).await;
                    // 记忆写入（tool-call 与 tool-result）
                    self.memory_store_client.push_tool_call(ToolCallRequest {
                        agent_id: agent_id.clone(),
                        role_name: Arc::new(role_name.clone()),
                        tool_name: call.name.clone(),
                        tool_params: call.arguments.clone(),
                        key: Arc::new(String::new()),
                        time: Arc::new(now.clone()),
                    }).await;
                    self.memory_store_client.push_tool_result(ToolResultRequest {
                        agent_id: agent_id.clone(),
                        role_name: Arc::new(role_name.clone()),
                        tool_result: Arc::new(result.clone()),
                        key: Arc::new(String::new()),
                        time: Arc::new(now.clone()),
                    }).await;
                }
                continue;  // 继续下一轮
            }
            Ok(model_resp) => {
                let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

                // 6. 追加 assistant 回复 + 写缓存
                {
                    let mut ctx = session.context.lock().await;
                    ctx.push(Message::Assistant {
                        content: Arc::new(model_resp.content.clone()),
                        reasoning_content: model_resp.reasoning_content.clone().map(Arc::new),
                        tool_calls: None,
                    });
                }
                self.cache.append(&key, &[Message::Assistant {
                    content: Arc::new(model_resp.content.clone()),
                    reasoning_content: model_resp.reasoning_content.clone().map(Arc::new),
                    tool_calls: None,
                }]).await;

                // 7. think 写入记忆（reasoning_content + thinking 双字段）
                if should_write_think(model_resp.reasoning_content.as_deref(), model_resp.thinking.as_deref()) {
                    let key_uuid = uuid::Uuid::new_v4().to_string();
                    let role_name = memory_role(session.role_name.as_str(), &session.mode);
                    let agent_id = session.agent_id.clone();
                    self.memory_store_client.push_channel_record(ChannelRequest {
                        agent_id: agent_id.clone(),
                        role_name: Arc::new(role_name.clone()),
                        messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
                        user_id: Arc::new(out_channel.user.user_id.clone()),
                        self_user_id: Arc::new(out_channel.user.user_id.clone()),
                        group_id: out_channel.group_id.clone(),
                        is_self: 1,
                        messenger_name: Arc::new(String::new()),
                        user_name: Arc::new(String::new()),
                        group_name: Arc::new(String::new()),
                        content: Content::Think(Arc::new(key_uuid.clone())),
                        time: Arc::new(now.clone()),
                    }).await;
                    self.memory_store_client.push_think(ThinkRequest {
                        agent_id,
                        role_name: Arc::new(role_name),
                        reasoning_content: Arc::new(model_resp.reasoning_content.clone().unwrap_or_default()),
                        thinking: Arc::new(model_resp.thinking.clone().unwrap_or_default()),
                        key: Arc::new(key_uuid),
                        time: Arc::new(now.clone()),
                    }).await;
                }

                // 8. 发送回复到该会话的 out_channel
                self.send_outgoing(out_channel, model_resp.content).await;
                break;
            }
            Err(e) => {
                warn!("模型调用失败: {:?}", e);
                self.send_outgoing(out_channel, format!("❌ 模型调用失败: {}", e)).await;
                break;
            }
        }
    }

    // 9. 检查上下文超长（阈值来自会话模型的 effective.max_context_messages）
    let overflow = {
        let ctx = session.context.lock().await;
        let model = session.model.load_full();
        match model.as_ref() {
            Some(pm) => match self.config.resolve_effective_config(pm).await {
                Some(eff) => ctx.is_overflow(eff.max_context_messages as usize),
                None => false,
            },
            None => false,
        }
    };
    if overflow {
        warn!("会话上下文超长，触发重置: role={} mode={:?}", session.role_name, session.mode);
        match &*session.mode {
            Mode::Event(_) => self.compress_context(session).await,
            Mode::Role => self.reset_context(session).await,
        }
    }
}
```

> `run_agentic_loop` 用到的 import 补充：`kissbot_api::memory::{ChannelRequest, ThinkRequest, ToolCallRequest, ToolResultRequest}`（coordinator.rs 现有 import 检查，缺的补上）；`Content` 已有；`ToolCallRequest`/`ToolResultRequest` 字段以 kissbot-api 为准（memory.rs 定义）。

> `compress_context`（Task 10）中的 `mc.call(pm, &messages)` 同步改为三参 `mc.call(pm, &messages, &[])`（压缩不携带工具）。

- [ ] **Step 5: 多轮 loop 测试策略（单元级覆盖，端到端留手动验证）**

多轮 agentic loop 的测试覆盖策略（不写需要完整 AgentCoordinator 环境的集成测试，避免大量脚手架）：

- **wire 层**：`openai_body_includes_tools_when_present`、`openai_body_maps_tool_and_assistant_tool_calls`、`parse_openai_response_extracts_tool_calls`（Step 2）验证「请求带 tools 数组」「assistant.tool_calls + tool 消息序列化」「响应 tool_calls 解析」——即循环两轮间的消息格式
- **执行层**：`station_runtime_local_call`（Task 11）验证本地工具查表执行与未注册报错；`execute_tool_call` 的路由/错误兜底逻辑与之一致
- **循环控制**：`compressed_messages`（Task 10）+ loop 的「tool_calls 非空继续、为空 break」由 wire 层测试间接保证；`MAX_TOOL_ROUNDS` 上限防死循环为常量守卫
- **端到端**：Task 14 手动验证清单覆盖（真实 LLM + 本地 station + web 通道）

> 若实现时发现可低成本构造（本地 axum mock LLM + 临时 ConfigManager），可补充；否则以上述单元覆盖为准。

- [ ] **Step 6: 运行测试**

Run: `cd kissbot-agent && cargo test`
Expected: 全部通过（openai_body/parse tool_calls 新测试 PASS，现有 provider 测试补 &[] 参数后 PASS）

- [ ] **Step 7: Commit**

```bash
git add kissbot-agent/src/
git commit -m "feat(agent): Agentic Loop 多轮工具循环——Provider/ModelClient 加 tools 参数（openai wire 发 tools 数组、解析 tool_calls）；coordinator 构建 station_runtimes（本地站注册内置 read）、tools_for_session（启用 station 聚合）、execute_tool_call（路由执行/错误兜底）；loop 循环上限 10 轮，每步写缓存，tool-call/result 写记忆，溢出按 mode 压缩/重置"
```

---

### Task 14: 收尾——全量构建、测试、文档与手动验证

**Files:**
- Modify: `docs/design/components-design/kissbot-agent-nexus.md`（关键设计节按新现状更新）
- Modify: `docs/design/components-design/kissbot-agent-station.md`（Station 框架现状）

- [ ] **Step 1: 全量构建 + 测试**

Run: `cd kissbot-agent && cargo build 2>&1 | grep -E "warning: unused|error" ; cargo test`
Run: `cd kissbot-memory && cargo test`
Run: `cd kissbot-api && cargo test`
Run: `cd kissbot-memory-store && cargo build`
Expected: 无 error；未使用的 import/字段按 `#[allow(dead_code)]` 或删除处理（注意：不删除注释）

- [ ] **Step 2: 更新组件设计文档（只说明现状）**

`docs/design/components-design/kissbot-agent-nexus.md`：
- 「4. 上下文构建器」小节更新：上下文为 OpenAI 格式 `Message` 枚举；每会话持有 `SessionContext`（内存）+ 本地缓存（`<data_dir>/context/`）；来源为 channel 合批与记忆打包；重置/压缩流程按模式（event 缓存恢复/压缩、role 记忆打包重建）
- 记忆读取器：role 模式两查询（时间窗 + 最近 N）比较首条时间取更早者，打包为一条 user 消息
- 工具调用分派：多轮 agentic loop、StationRuntime 本地/REST、tools 聚合自启用 station

`docs/design/components-design/kissbot-agent-station.md`：
- 工具注册表：StationConfig.tools（ToolConfig name/description/parameters）；Tool trait 统一参数/返回值
- 运行模式：base_url 非空 REST、为空本地；内置 Read 工具（路径校验防穿透）

- [ ] **Step 3: 手动验证清单（脚本级，agent 本地模式）**

在 `script/` 下提供验证说明（不写自动化脚本，只列步骤）：
1. 配置 `test/workspace/agent-data/nexus.json`：`context` 段加 `""` agent（保留 agent）与 `a1` agent 的 context 配置；stations 段加本地 station（`base_url: ""`、`tools.read` 配 JSON Schema）
2. 启动 memory-store、channel-web、agent（`script/start-*.sh`）
3. 通过 web 发普通消息 → 观察 agent 日志：合批 3s 后进 loop；LLM 返回 tool_call（若模型支持工具）→ 本地执行 read → 第二轮回复
4. 验证 `<data_dir>/context/` 缓存文件随对话增长、`context-history/` 在压缩/重置后出现归档文件

- [ ] **Step 4: Commit**

```bash
git add docs/design/components-design/ script/
git commit -m "docs(agent): 更新 nexus/station 组件设计文档——OpenAI 格式 Message 上下文、缓存/历史归档、合批与记忆打包、多轮 tool loop、StationRuntime 本地/REST、Read 工具路径校验；补充本地模式手动验证步骤"
```

---

## Self-Review 记录

（writing-plans 自审，修正项已同步进正文）

**1. Spec 覆盖检查**（设计文档 9 节 → 任务）：

| Spec 节 | 任务 |
|---|---|
| 1 上下文表示（Message/ToolCall、reasoning_content 不发送、多轮序列） | T1、T2、T13（wire） |
| 2 会话与配置（Session.agent_name、context 段三层继承、max_context_messages 入 provider/model） | T2、T3 |
| 3 缓存与历史（追加不截断/全量回读/归档=复制文件/只写不读） | T4、T5、T6 |
| 4 来源（channel 合批 3s debounce/记忆两查询比较打包/memory-store limit 扩展） | T7、T8、T9 |
| 5 重置/压缩（event 恢复/压缩、role 归档+记忆重建、启动同重置） | T6、T9、T10 |
| 6 Station 框架（tools Map/Tool trait/StationRuntime 本地+REST 骨架/Read 校验/启用 stations） | T11、T12、T13 |
| 7 Agentic Loop（多轮上限/每步写缓存/溢出按 mode） | T6、T13 |
| 8 Provider wire（OpenAI tools/tool_calls/tool；Anthropic content-only） | T2、T13 |
| 9 测试范围（单测+本地模式；REST 不实现不测试） | 各任务内 + T14 |

**2. 占位符扫描**：无 TBD/TODO；Task 13 Step 5 曾含半截集成测试，已替换为明确的单元覆盖策略说明。

**3. 类型一致性修正**（已改入正文）：
- `merge_context_config` 简化（去掉 empty_agent hack，agent 为 None 直接全局默认；role 只可能来自 agent.roles）
- Task 8 测试改用直接写 JSONL 文件（kissbot-memory 无法访问 memory-store 的 RecordManager）+ Once 初始化避免 env 竞态；`take_recent` 改为 K/R 双泛型
- Task 7 `enqueue_batch` 收 `user_name` 参数（去掉 messenger_id/user_id 残留）
- Task 10 `compress_context` 在 Task 10 时点用两参 `mc.call`，Task 13 改三参（两处均已注明同步改）
- Task 10 补 role 重新进入的归档缺口：`build_initial_context` role 分支缓存存在先归档再重建（reset 已清空不重复）
- Task 12 `StationRuntime` 增加 `config()` 访问器（Task 13 coordinator 判断 base_url 用）
- **Pre-flight（用户定夺后已全量同步）**：`Message`/`ToolCall`/`ToolConfig` 字段改 `Arc<String>`/`Arc<Value>`（构造 `Arc::new`、读取 `.as_str()`/`(*x).clone()`/`.map(Arc::new)`），全计划代码片段已逐处更新（Task 1/2/4/6/9/10/12/13）
