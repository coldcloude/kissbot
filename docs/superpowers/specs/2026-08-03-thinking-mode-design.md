# 思考模式（Thinking Mode）支持设计

## 目标

1. provider / model 配置中 `temperature` 改为可选，未配置时**不传**该参数
2. 新增可选 `thinking`（字符串）与 `reasoning_effort`（字符串）参数，未配置时**不传**对应参数
3. 依据 DeepSeek 思考模式文档，实现 openai、anthropic 两种 provider_type 的思考模式传参
4. 响应解析：提取 `reasoning_content`（openai 格式）或 `type="thinking"` block（anthropic 格式），或 content 开头 `<think></think>` 标签内的思考内容；思考内容存入 Think 记忆

## 配置层（kissbot-agent/src/config_manager.rs）

### ProviderConfig

`default_temperature: f32` 改为 `Option<f32>`；新增两个可选默认值：

```rust
pub struct ProviderConfig {
    // ...
    pub default_temperature: Option<f32>,          // 原 f32 → Option
    pub default_thinking: Option<String>,          // 新增：原样进 {"thinking":{"type":...}}
    pub default_reasoning_effort: Option<String>,  // 新增
    // ...
}
```

### ModelConfig

`temperature` 已是 `Option<f32>`，新增两个继承参数：

```rust
pub struct ModelConfig {
    // ...
    pub temperature: Option<f32>,          // 不变
    pub thinking: Option<String>,          // 新增
    pub reasoning_effort: Option<String>,  // 新增
    // ...
}
```

### EffectiveModelConfig

`temperature: f32` 改为 `Option<f32>`；新增两个字段：

```rust
pub struct EffectiveModelConfig {
    // ...
    pub temperature: Option<f32>,          // 原 f32 → Option
    pub thinking: Option<String>,          // 新增
    pub reasoning_effort: Option<String>,  // 新增
    // ...
}
```

`resolve_effective_config` 合并沿用现有模式：`model_cfg.xxx.clone().or(provider.default_xxx)`。

### serde 兼容

- 旧 `nexus.json` 的 `default_temperature: 0.7` 自动反序列化为 `Some(0.7)`，无需迁移
- 新字段缺省即 None；序列化用 `#[serde(skip_serializing_if = "Option::is_none")]` 保持配置整洁

## Provider 传参（kissbot-agent/src/provider.rs）

三个参数全部**有值才传**，无值不出现 key。

### openai_body（/chat/completions）

```json
{
  "model": "...",
  "messages": [...],
  "max_tokens": 2048,
  "temperature": 0.3,                    // 可选：temperature 有值时
  "thinking": { "type": "enabled" },     // 可选：thinking 有值时（原样）
  "reasoning_effort": "high",            // 可选：reasoning_effort 有值时（原样）
  "stream": false
}
```

### anthropic_body（/v1/messages）

```json
{
  "model": "...",
  "messages": [...],
  "max_tokens": 2048,
  "temperature": 0.3,                    // 可选：temperature 有值时
  "thinking": { "type": "enabled" },     // 可选：thinking 有值时（原样）
  "output_config": { "effort": "high" }, // 可选：reasoning_effort 有值时
  "system": "..."                        // 仅 system 消息非空时（原逻辑）
}
```

映射规则：
- `thinking`：两种格式均原样进 `{"thinking": {"type": <值>}}`
- `reasoning_effort`：openai 进顶层 `"reasoning_effort"`；anthropic 进 `"output_config": {"effort": <值>}`
- 不使用 DeepSeek 文档中 anthropic 格式的 `reasoning: {"effort": ...}` 参数

## 响应解析

### ModelResponse（kissbot-agent/src/types.rs）

新增字段：

```rust
pub struct ModelResponse {
    pub content: String,
    pub reasoning_content: Option<String>,   // 新增：思考内容（无则为 None）
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
}
```

### parse_openai_response

1. `reasoning_content = choices[0].message.reasoning_content`（DeepSeek 思考模式返回值，可缺失）
2. `content = choices[0].message.content`
3. 剥离 content 开头的 `<think>...</think>`（**总是执行**，避免标签泄漏给用户/上下文）
4. 若 reasoning_content 仍为空，用剥离出的 `<think>` 内容兜底作为思考内容

### parse_anthropic_response

1. 遍历 `content` blocks，找 `type == "thinking"` 的块 → 取 `thinking` 字段作为思考内容
2. 文本取第一个 `type == "text"` 块的 `text`（原逻辑）
3. 同上：剥离 `<think>` 标签（总是执行）；思考内容为空时用标签内容兜底

### 公共剥离逻辑（纯函数，便于单测）

```rust
/// 匹配 content 开头的 <think>...</think>，剥离并返回 (剥离后内容, Option<思考内容>)
fn strip_think_tag(content: &str) -> (String, Option<String>)
```

效果：`ModelResponse.content` 是剥离后的最终回复；`reasoning_content` 是思考内容。

## 记忆层（kissbot-agent/src/coordinator.rs）

`run_agentic_loop` 模型调用成功后：

```rust
Ok(model_resp) => {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    // 上下文：推入剥离后的最终回复（原逻辑，content 已由 parse 层剥离）
    ctx.push_assistant(model_resp.content.clone(), now.clone());

    // Think 记忆：只存思考内容；无思考内容时不写
    if let Some(reasoning) = &model_resp.reasoning_content {
        let _ = self.memory_writer.push(WriteTask::Think {
            agent_id: session.agent_id.to_string(),
            role_name: Some(role_name),
            content: reasoning.clone(),
            time: now,
        });
    }

    // 发送剥离后的最终回复（原逻辑）
    self.reply(channel_id, &group_id, model_resp.content).await;
    // 上下文超长检查不变
}
```

要点：
- Think 记忆只存思考内容（reasoning_content 或 `<think>` 提取值）
- 无思考内容时不写 Think
- 最终回复 = 剥离后的 content，发送 / 上下文 / channel 记忆逻辑不变

## 模板修改

三处 nexus.json 模板：去掉 `default_temperature`，新增 `default_thinking: "enabled"`、`default_reasoning_effort: "high"`：

- `script/template/nexus.json`
- `test/workspace-template/agent-data/nexus.json`
- `test/workspace/agent-data/nexus.json`

## 测试

- `sample_effective` 改为含 Option 字段的构造
- `openai_body` / `anthropic_body`：覆盖参数组合——temperature / thinking / reasoning_effort 有值传、无值不传（JSON 断言无对应 key）
- `parse_openai_response`：reasoning_content 提取；缺失时 `<think>` 回退 + 剥离
- `parse_anthropic_response`：`type=="thinking"` block 提取；`<think>` 回退 + 剥离
- `strip_think_tag` 单测：有/无标签、非开头标签、空内容
- `resolve_effective_config`：Option 合并（model 覆盖 provider 默认）

## 不修改

- docs/spec 文档（用户明确不改）
- 配置 UI（kissbot-agent-config App.tsx 是 stub）
- memory-store 读回逻辑
