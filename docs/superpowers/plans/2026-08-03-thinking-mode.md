# 思考模式（Thinking Mode）支持 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** provider/model 配置中 temperature 可选化，新增 thinking / reasoning_effort 可选参数（openai/anthropic 两格式传参），响应解析思考内容（reasoning_content / thinking block / `<think>` 标签）存入 Think 记忆。

**Architecture:** 配置层（config_manager.rs）三个结构体字段 Option 化与新增，合并逻辑沿用现有 `model_cfg.xxx.or(provider.default_xxx)` 模式；provider 层（provider.rs）body 生成改为"有值才传"，响应解析新增思考内容提取与 `<think>` 标签剥离（纯函数 strip_think_tag 便于单测）；coordinator 层 Think 记忆改为只存思考内容（方案 A）。

**Tech Stack:** Rust（tokio / serde / serde_json / reqwest / axum），cargo test 验证。

## Global Constraints

- **不删除代码中的注释**（项目约定）
- **不修改 docs/spec 文档**（用户明确要求）
- **不修改配置 UI**（kissbot-agent-config App.tsx 是 stub）与 **memory-store 读回逻辑**
- 字段名固定为 `reasoning_content`（不是 reasoning）
- `thinking` 是 `String`，原样进 `{"thinking": {"type": <值>}}`（如 "enabled"/"disabled"）
- anthropic 格式**不用** `reasoning: {"effort": ...}` 参数；reasoning_effort 在 anthropic 进 `output_config: {"effort": <值>}`
- 三个可选参数（temperature / thinking / reasoning_effort）**有值才传**，无值不出现 key
- `<think></think>` 标签**总是剥离**（只要位于 content 开头，允许前导空白），仅作为思考内容兜底
- Think 记忆**只存思考内容**，无思考内容时不写（方案 A）
- 文本文件 UTF-8、LF；commit comment 用中文，且应包含本次提交所有改动内容

---

### Task 1: 配置层 Option 化（config_manager.rs + 关联构造点）

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`（结构定义 + resolve_effective_config + tests）
- Modify: `kissbot-agent/src/http_server.rs:171-184`（test_provider 构造）
- Modify: `kissbot-agent/src/provider.rs:221-234`（sample_effective 构造）

**Interfaces:**
- Produces: `EffectiveModelConfig { temperature: Option<f32>, thinking: Option<String>, reasoning_effort: Option<String>, ... }`；`ProviderConfig.default_temperature: Option<f32>`、`default_thinking: Option<String>`、`default_reasoning_effort: Option<String>`；`ModelConfig.thinking: Option<String>`、`reasoning_effort: Option<String>`

- [ ] **Step 1: 更新测试代码（构造点 + 断言 + 新增合并/兼容测试）**

在 `kissbot-agent/src/config_manager.rs` 的 tests 模块：

`sample_provider` 改为：
```rust
    fn sample_provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: Arc::new(name.into()),
            provider_type: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "sk-test".into(),
            default_context_length: 65536,
            default_max_tokens: 4096,
            default_temperature: Some(0.7),
            default_timeout_secs: 60,
            default_retry_count: 3,
            default_thinking: None,
            default_reasoning_effort: None,
            models: Arc::new(ArcSwapHashMap::new()),
        }
    }
```

三处 `ModelConfig { ... }` 构造（行 ~697、~739、~852，`context_length: ...` 之后）各追加两个字段：
```rust
                thinking: None,
                reasoning_effort: None,
```

`resolve_effective_config_merges_provider_and_model`（行 ~697-731）：model 的 ModelConfig 构造改为：
```rust
            map.insert("deepseek-4-flash".to_string(), ArcSwap::new(Arc::new(ModelConfig {
                model: "deepseek-4-flash".into(),
                max_tokens: Some(2048),
                temperature: Some(0.3),
                timeout_secs: Some(30),
                retry_count: Some(2),
                context_length: None,  // 未配 → 继承 provider 默认
                thinking: Some("enabled".into()),
                reasoning_effort: Some("high".into()),
            })));
```
断言改为（`assert_eq!(eff.temperature, 0.3, ...)` 替换）：
```rust
        assert_eq!(eff.temperature, Some(0.3), "model 的 temperature 应生效");
        assert_eq!(eff.thinking.as_deref(), Some("enabled"), "model 的 thinking 应生效");
        assert_eq!(eff.reasoning_effort.as_deref(), Some("high"), "model 的 reasoning_effort 应生效");
```

`resolve_effective_config_inherits_provider_defaults`（行 ~733-764）：将测试开头的 provider 构造与 ModelConfig 构造替换为（含思考默认值设置与新增字段）：
```rust
        let mut provider = sample_provider("deepseek");
        // 设置思考相关默认值（验证未配置时继承 provider 默认）
        provider.default_thinking = Some("disabled".into());
        provider.default_reasoning_effort = Some("low".into());
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("deepseek-4-flash".to_string(), ArcSwap::new(Arc::new(ModelConfig {
                model: "deepseek-4-flash".into(),
                max_tokens: None,
                temperature: None,
                timeout_secs: None,
                retry_count: None,
                context_length: Some(131072),  // 覆盖 context_length
                thinking: None,
                reasoning_effort: None,
            })));
        }
```
断言改为：
```rust
        assert_eq!(eff.temperature, Some(0.7));
```
并在该测试末尾（`assert_eq!(eff.context_length, 131072, "model 覆盖 context_length 应生效");` 之后）追加：
```rust
        assert_eq!(eff.thinking.as_deref(), Some("disabled"), "thinking 未配应继承 provider 默认");
        assert_eq!(eff.reasoning_effort.as_deref(), Some("low"), "reasoning_effort 未配应继承 provider 默认");
```

`resolve_effective_config_missing_returns_none` / `resolve_effective_config_synthesizes_unconfigured_model` / `provider_crud_and_default_set` 中三处 `assert_eq!(eff.temperature, 0.7);` 改为 `assert_eq!(eff.temperature, Some(0.7));`。

新增兼容性测试（放在 `sample_provider` 附近）：
```rust
    #[test]
    fn provider_config_old_shape_migration() {
        // 旧格式：default_temperature 为数值，无 default_thinking/default_reasoning_effort
        let old = r#"{
            "name": "deepseek",
            "provider_type": "openai",
            "base_url": "https://api.deepseek.com",
            "api_key": "sk-test",
            "default_context_length": 65536,
            "default_max_tokens": 4096,
            "default_temperature": 0.7,
            "default_timeout_secs": 60,
            "default_retry_count": 3,
            "models": {}
        }"#;
        let pc: ProviderConfig = serde_json::from_str(old).unwrap();
        assert_eq!(pc.default_temperature, Some(0.7), "旧数值应映射为 Some");
        assert_eq!(pc.default_thinking, None);
        assert_eq!(pc.default_reasoning_effort, None);
    }
```

`kissbot-agent/src/http_server.rs` tests 的 `test_provider` 改为：
```rust
    fn test_provider(name: &str) -> crate::config_manager::ProviderConfig {
        crate::config_manager::ProviderConfig {
            name: Arc::new(name.into()),
            provider_type: "openai".into(),
            base_url: "https://api.example.com".into(),
            api_key: "sk-test".into(),
            default_context_length: 65536,
            default_max_tokens: 4096,
            default_temperature: Some(0.7),
            default_timeout_secs: 60,
            default_retry_count: 3,
            default_thinking: None,
            default_reasoning_effort: None,
            models: Arc::new(ArcSwapHashMap::new()),
        }
    }
```

`kissbot-agent/src/provider.rs` tests 的 `sample_effective` 改为：
```rust
    fn sample_effective() -> EffectiveModelConfig {
        EffectiveModelConfig {
            provider_type: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "sk-test".into(),
            model: "deepseek-4-flash".into(),
            max_tokens: 2048,
            temperature: Some(0.3),
            timeout_secs: 30,
            retry_count: 2,
            context_length: 65536,
            thinking: None,
            reasoning_effort: None,
        }
    }
```

- [ ] **Step 2: 运行测试确认编译失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 编译错误——结构体尚无新字段（缺 `default_thinking` / `thinking` 等字段或类型不匹配）。

- [ ] **Step 3: 实现结构体与合并逻辑**

`ProviderConfig` 定义中 `pub default_temperature: f32,` 替换为：
```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_temperature: Option<f32>,
    pub default_timeout_secs: u64,
    pub default_retry_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_thinking: Option<String>,          // 默认思考模式开关值（原样进 {"thinking":{"type":...}}）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,  // 默认思考力度
```

`EffectiveModelConfig` 定义中 `pub temperature: f32,` 替换为：
```rust
    pub temperature: Option<f32>,
```
并在 `context_length` 之后追加：
```rust
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
```

`ModelConfig` 定义中 `context_length` 字段之后追加：
```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
```

`resolve_effective_config` 中：
```rust
            temperature: model_cfg.as_ref().and_then(|m| m.temperature).unwrap_or(provider.default_temperature),
```
替换为：
```rust
            temperature: model_cfg.as_ref().and_then(|m| m.temperature).or(provider.default_temperature),
```
并在 `context_length` 之后追加：
```rust
            thinking: model_cfg.as_ref().and_then(|m| m.thinking.clone()).or(provider.default_thinking.clone()),
            reasoning_effort: model_cfg.as_ref().and_then(|m| m.reasoning_effort.clone()).or(provider.default_reasoning_effort.clone()),
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 全部 PASS（含新增 provider_config_old_shape_migration）。

- [ ] **Step 5: 提交**

```bash
git add kissbot-agent/src/config_manager.rs kissbot-agent/src/http_server.rs kissbot-agent/src/provider.rs
git commit -m "feat(config): temperature 可选化并新增 thinking/reasoning_effort 配置——ProviderConfig.default_temperature 改 Option<f32>，新增 default_thinking/default_reasoning_effort；ModelConfig 新增 thinking/reasoning_effort 继承参数；EffectiveModelConfig 相应 Option 化；resolve_effective_config 合并逻辑改为 or（model 优先，无则继承 provider 默认）；新增旧格式兼容测试"
```

---

### Task 2: Provider body 可选参数传参（provider.rs）

**Files:**
- Modify: `kissbot-agent/src/provider.rs`（openai_body / anthropic_body + tests）

**Interfaces:**
- Consumes: `EffectiveModelConfig.temperature: Option<f32>`、`thinking: Option<String>`、`reasoning_effort: Option<String>`（Task 1 产出）
- Produces: openai body 含可选 `temperature` / `thinking: {"type": ...}` / `reasoning_effort`；anthropic body 含可选 `temperature` / `thinking: {"type": ...}` / `output_config: {"effort": ...}`

- [ ] **Step 1: 写失败测试**

在 `kissbot-agent/src/provider.rs` tests 模块追加：
```rust
    #[test]
    fn openai_body_omits_optional_params_when_none() {
        let mut eff = sample_effective();
        eff.temperature = None;
        eff.thinking = None;
        eff.reasoning_effort = None;
        let msgs = vec![MessageItem { role: "user".into(), content: "你好".into() }];
        let body = openai_body(&eff, &msgs);
        assert!(body.get("temperature").is_none(), "temperature 未配置不应传");
        assert!(body.get("thinking").is_none(), "thinking 未配置不应传");
        assert!(body.get("reasoning_effort").is_none(), "reasoning_effort 未配置不应传");
        assert_eq!(body["model"], "deepseek-4-flash");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn openai_body_passes_thinking_and_reasoning_effort() {
        let mut eff = sample_effective();
        eff.thinking = Some("enabled".into());
        eff.reasoning_effort = Some("high".into());
        let msgs = vec![MessageItem { role: "user".into(), content: "你好".into() }];
        let body = openai_body(&eff, &msgs);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["temperature"], 0.3_f32 as f64);
    }

    #[test]
    fn anthropic_body_omits_optional_params_when_none() {
        let mut eff = sample_effective();
        eff.temperature = None;
        eff.thinking = None;
        eff.reasoning_effort = None;
        let msgs = vec![MessageItem { role: "user".into(), content: "hi".into() }];
        let body = anthropic_body(&eff, &msgs);
        assert!(body.get("temperature").is_none(), "temperature 未配置不应传");
        assert!(body.get("thinking").is_none(), "thinking 未配置不应传");
        assert!(body.get("output_config").is_none(), "reasoning_effort 未配置不应传 output_config");
    }

    #[test]
    fn anthropic_body_passes_thinking_and_output_config() {
        let mut eff = sample_effective();
        eff.thinking = Some("enabled".into());
        eff.reasoning_effort = Some("high".into());
        let msgs = vec![MessageItem { role: "user".into(), content: "hi".into() }];
        let body = anthropic_body(&eff, &msgs);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["temperature"], 0.3_f32 as f64);
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: `openai_body_omits_optional_params_when_none` 失败（temperature 恒被传入）；`openai_body_passes_thinking_and_reasoning_effort` / `anthropic_body_passes_thinking_and_output_config` 失败（body 中无 thinking/reasoning_effort/output_config key）。

- [ ] **Step 3: 实现 openai_body**

`fn openai_body` 整体替换为：
```rust
fn openai_body(effective: &EffectiveModelConfig, messages: &[MessageItem]) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages.iter().map(|m| {
        json!({ "role": m.role, "content": m.content })
    }).collect();
    let mut body = json!({
        "model": effective.model,
        "messages": msgs,
        "max_tokens": effective.max_tokens,
        "stream": false,
    });
    // 可选参数：有值才传（temperature / thinking / reasoning_effort）
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

- [ ] **Step 4: 实现 anthropic_body**

`fn anthropic_body` 中 `if !system.is_empty() { body["system"] = json!(system); }` 之后追加：
```rust
    // 可选参数：有值才传（temperature / thinking / output_config.effort）
    if let Some(t) = effective.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = &effective.thinking {
        body["thinking"] = json!({ "type": t });
    }
    if let Some(e) = &effective.reasoning_effort {
        body["output_config"] = json!({ "effort": e });
    }
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 全部 PASS（含既有 openai_body_includes_params_and_messages / anthropic_body_separates_system_messages）。

- [ ] **Step 6: 提交**

```bash
git add kissbot-agent/src/provider.rs
git commit -m "feat(provider): 思考模式传参——temperature/thinking/reasoning_effort 改为有值才传；openai 格式 thinking 进 {\"thinking\":{\"type\":...}}、reasoning_effort 进顶层；anthropic 格式 thinking 同 openai、reasoning_effort 进 output_config.effort"
```

---

### Task 3: 响应解析思考内容（types.rs + provider.rs）

**Files:**
- Modify: `kissbot-agent/src/types.rs`（ModelResponse）
- Modify: `kissbot-agent/src/provider.rs`（parse_openai_response / parse_anthropic_response + strip_think_tag + tests）

**Interfaces:**
- Consumes: 无（纯解析函数）
- Produces: `ModelResponse.reasoning_content: Option<String>`；`fn strip_think_tag(content: &str) -> (String, Option<String>)`（剥离后内容, 思考内容）

- [ ] **Step 1: 写失败测试**

在 `kissbot-agent/src/provider.rs` tests 模块追加：
```rust
    #[test]
    fn strip_think_tag_extracts_and_removes_leading_tag() {
        assert_eq!(strip_think_tag("<think>让我想想</think>答案"), ("答案".to_string(), Some("让我想想".to_string())));
    }

    #[test]
    fn strip_think_tag_keeps_non_leading_tag() {
        let content = "答案<think>思考</think>";
        assert_eq!(strip_think_tag(content), (content.to_string(), None));
    }

    #[test]
    fn strip_think_tag_allows_leading_whitespace() {
        assert_eq!(strip_think_tag("\n<think>思考</think>答案"), ("\n答案".to_string(), Some("思考".to_string())));
    }

    #[test]
    fn strip_think_tag_returns_unchanged_when_no_tag() {
        assert_eq!(strip_think_tag("普通文本"), ("普通文本".to_string(), None));
        assert_eq!(strip_think_tag(""), ("".to_string(), None));
    }

    #[test]
    fn strip_think_tag_keeps_unclosed_tag() {
        let content = "<think>未闭合";
        assert_eq!(strip_think_tag(content), (content.to_string(), None));
    }

    #[test]
    fn parse_openai_response_extracts_reasoning_content() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案", "reasoning_content": "思考" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content, "答案");
        assert_eq!(resp.reasoning_content.as_deref(), Some("思考"));
    }

    #[test]
    fn parse_openai_response_falls_back_to_think_tag() {
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "<think>思考</think>答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.content, "答案", "<think> 标签应剥离");
        assert_eq!(resp.reasoning_content.as_deref(), Some("思考"));
    }

    #[test]
    fn parse_anthropic_response_extracts_thinking_block() {
        let data = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "思考过程" },
                { "type": "text", "text": "答复" }
            ],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content, "答复");
        assert_eq!(resp.reasoning_content.as_deref(), Some("思考过程"));
    }

    #[test]
    fn parse_anthropic_response_falls_back_to_think_tag() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "<think>思考</think>答复" }],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.content, "答复");
        assert_eq!(resp.reasoning_content.as_deref(), Some("思考"));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 编译错误——`ModelResponse` 无 `reasoning_content` 字段、无 `strip_think_tag` 函数。

- [ ] **Step 3: 实现 types.rs ModelResponse**

`pub struct ModelResponse` 中 `pub content: String,` 之后追加：
```rust
    /// 思考内容（DeepSeek reasoning_content / anthropic thinking block / <think> 标签兜底；无则为 None）
    pub reasoning_content: Option<String>,
```

- [ ] **Step 4: 实现 strip_think_tag 与 parse 函数**

`fn parse_openai_response` 整体替换为：
```rust
fn parse_openai_response(data: &serde_json::Value) -> ModelResponse {
    let choice = &data["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("").to_string();
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop").to_string();
    // 思考内容：优先 API 的 reasoning_content 字段，缺失时用 <think> 标签兜底；<think> 标签总是剥离
    let api_reasoning = choice["message"]["reasoning_content"].as_str().map(String::from);
    let (content, tag_reasoning) = strip_think_tag(&content);
    let reasoning_content = api_reasoning.or(tag_reasoning);
    ModelResponse { content, reasoning_content, tool_calls: Vec::new(), finish_reason }
}
```

`fn parse_anthropic_response` 整体替换为：
```rust
fn parse_anthropic_response(data: &serde_json::Value) -> ModelResponse {
    // 思考内容：content blocks 中 type=="thinking" 的块（DeepSeek/Anthropic 均返回该结构）
    let mut reasoning_content = None;
    let mut content = String::new();
    if let Some(blocks) = data["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("thinking") if reasoning_content.is_none() => {
                    reasoning_content = block["thinking"].as_str().map(String::from);
                }
                Some("text") if content.is_empty() => {
                    content = block["text"].as_str().unwrap_or("").to_string();
                }
                _ => {}
            }
        }
    }
    let finish_reason = data["stop_reason"].as_str().unwrap_or("end_turn").to_string();
    // <think> 标签总是剥离；思考内容为空时用标签内容兜底
    let (content, tag_reasoning) = strip_think_tag(&content);
    let reasoning_content = reasoning_content.or(tag_reasoning);
    ModelResponse { content, reasoning_content, tool_calls: Vec::new(), finish_reason }
}
```

在 `fn parse_openai_models` 之前追加（放在 Provider 实现区之前均可）：
```rust
/// 匹配 content 开头的 <think>...</think>（允许前导空白），剥离并返回 (剥离后内容, Option<思考内容>)
/// 标签不在开头或未闭合时原样返回
fn strip_think_tag(content: &str) -> (String, Option<String>) {
    let start = content.len() - content.trim_start().len();
    let trimmed = &content[start..];
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            let thinking = rest[..end].to_string();
            let mut stripped = String::with_capacity(content.len());
            stripped.push_str(&content[..start]);
            stripped.push_str(&rest[end + "</think>".len()..]);
            return (stripped, Some(thinking));
        }
    }
    (content.to_string(), None)
}
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 全部 PASS（含既有 parse_openai_response_extracts_content_and_finish_reason / parse_anthropic_response_extracts_text_and_stop_reason）。

- [ ] **Step 6: 提交**

```bash
git add kissbot-agent/src/types.rs kissbot-agent/src/provider.rs
git commit -m "feat(provider): 响应解析思考内容——ModelResponse 新增 reasoning_content 字段；openai 解析 choices[0].message.reasoning_content、anthropic 解析 content blocks 中 type=thinking 的块；新增 strip_think_tag 剥离 content 开头 <think></think>（总是剥离，思考内容缺失时兜底）"
```

---

### Task 4: Think 记忆只存思考内容（coordinator.rs）

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`（run_agentic_loop）

**Interfaces:**
- Consumes: `ModelResponse.reasoning_content: Option<String>`（Task 3 产出）

- [ ] **Step 1: 修改 run_agentic_loop 的 Think 推送**

`kissbot-agent/src/coordinator.rs` 中步骤 4 原代码：
```rust
                // 4. 推送 think 到 MemoryWriter（事件模式编码；取记忆用会话保存的 agent_id）
                let role_name = memory_role(&session.key);
                let _ = self.memory_writer.push(WriteTask::Think {
                    agent_id: session.agent_id.to_string(),
                    role_name: Some(role_name),
                    content: model_resp.content.clone(),
                    time: now,
                });
```
替换为：
```rust
                // 4. 推送 think 到 MemoryWriter（事件模式编码；取记忆用会话保存的 agent_id）
                // Think 记忆只存思考内容（方案 A）：有思考内容才写，无则跳过
                if let Some(reasoning) = &model_resp.reasoning_content {
                    let role_name = memory_role(&session.key);
                    let _ = self.memory_writer.push(WriteTask::Think {
                        agent_id: session.agent_id.to_string(),
                        role_name: Some(role_name),
                        content: reasoning.clone(),
                        time: now,
                    });
                }
```

- [ ] **Step 2: 构建与测试**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 编译通过（`model_resp.content` 仍用于步骤 3 上下文与步骤 5 发送，变量仍被使用），全部测试 PASS。

- [ ] **Step 3: 提交**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "feat(agent): Think 记忆只存思考内容——模型回复后若有 reasoning_content（reasoning_content 字段或 <think> 标签提取值）则写入 Think 记忆，无思考内容时不写；最终回复仍经原 channel 记忆（is_self=1）存储"
```

---

### Task 5: nexus.json 模板修改

**Files:**
- Modify: `script/template/nexus.json`
- Modify: `test/workspace-template/agent-data/nexus.json`
- Modify: `test/workspace/agent-data/nexus.json`

**Interfaces:**
- Consumes: `ProviderConfig` 新字段（Task 1 产出，serde 反序列化兼容）

- [ ] **Step 1: 修改三处模板**

三处 `nexus.json` 的 providers.deepseek 段中，删除 `"default_temperature": 0.7,` 行，并在 `"default_retry_count": 3,` 之后追加：
```json
      "default_thinking": "enabled",
      "default_reasoning_effort": "high",
```
（`script/template/nexus.json` 每行带缩进 6 空格、其余两处缩进 4 空格，按各自文件现有格式调整缩进）

- [ ] **Step 2: 验证 JSON 合法性与反序列化兼容**

Run: `cd /home/admin/project/kissbot && python3 -c "import json;[json.load(open(p)) for p in ['script/template/nexus.json','test/workspace-template/agent-data/nexus.json','test/workspace/agent-data/nexus.json']];print('OK')"`

注：仅用于验证 JSON 语法合法性（项目禁止用 python 改文件，验证不受限）。
Expected: `OK`

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 全部 PASS（serde 反序列化旧/新格式均兼容）。

- [ ] **Step 3: 提交**

```bash
git add script/template/nexus.json test/workspace-template/agent-data/nexus.json test/workspace/agent-data/nexus.json
git commit -m "chore(config): nexus.json 模板改为思考模式默认配置——去掉 default_temperature（可选参数未配置不传），新增 default_thinking=enabled、default_reasoning_effort=high（script/template 与 test 的 workspace/workspace-template 三处）"
```

---

### Task 6: 全量验证

- [ ] **Step 1: 运行 kissbot-agent 全部测试**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 全部 PASS，0 failed。

- [ ] **Step 2: 检查未提交改动**

Run: `cd /home/admin/project/kissbot && git status --short`
Expected: 无未提交改动（或仅剩本次计划外无关文件，需确认）。
