# max_tokens_usage 驱动的上下文重置实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除 `context_length` / `max_context_messages` 配置，新增必填 `max_tokens_usage`，改用模型 API 返回的 `usage.total_tokens` 驱动会话上下文重置（超过 80% 阈值触发）。

**Architecture:** `ModelResponse` 增加 `total_tokens` 字段（openai 解析 `usage.total_tokens`，anthropic 固定 0）；`Session` 记录最近一次模型响应的 total_tokens，`run_agentic_loop` 开头（追加用户消息前）检查是否超过 `effective.max_tokens_usage` 的 80%，超限执行现有重建逻辑（event 压缩 / role 记忆打包）后清零——延迟检查语义，无新消息不重置。

**Tech Stack:** Rust 2024（kissbot-agent 独立 crate）、tokio、serde/serde_json、arc-swap、dashmap、axum（测试）、Playwright（e2e）。

## Global Constraints

- 遵守 `.claude/rules/coding-standards.md`：非枚举/非 Map Key/非 Vec 字段用 `Arc<T>` 包裹；构造一律 `Arc::new(...)`，读取用 `.as_str()` / `(*x).clone()`
- 不要删除代码中的注释（CLAUDE.md）；`is_overflow` 函数与其注释随逻辑废弃一并删除属本次功能变更
- 禁止用 sed/python 等命令修改文件；读写用工具
- 测试运行：各 crate 独立，`cd kissbot-agent && cargo test`（无根 workspace）
- e2e 测试：`cd test && npx playwright test tests/agent-config-api.spec.ts`
- 提交 comment 用中文，包含本次改动全部内容
- 文本文件 UTF-8、`\n` 换行
- 80% 阈值判定：`max_tokens_usage > 0 && last_total_tokens * 10 > (max_tokens_usage as u64) * 8`（整数运算，u64 提升防溢出；`max_tokens_usage = 0` 视为未启用，永不触发）

---

### Task 1: ModelResponse 增加 total_tokens 与 provider 解析

**Files:**
- Modify: `kissbot-agent/src/types.rs`（`ModelResponse` 增加字段）
- Modify: `kissbot-agent/src/provider.rs`（`parse_openai_response` / `parse_anthropic_response` 填充字段）

**Interfaces:**
- Produces: `types::ModelResponse.total_tokens: u64`——Task 2 的 `Session.last_total_tokens` 消费
- 依赖 API 事实（已用 agent-browser 查证）：DeepSeek `/chat/completions` 与 Kimi `/chat/completions` 响应均为 OpenAI 兼容，`usage.total_tokens` 存在（`prompt_tokens + completion_tokens`）

- [ ] **Step 1: types.rs 增加字段**

在 `kissbot-agent/src/types.rs` 的 `ModelResponse` 结构末尾（`finish_reason` 之后）增加：

```rust
    #[allow(dead_code)]
    pub finish_reason: Arc<String>,
    /// 本次请求 token 总占用（usage.total_tokens；openai 解析、anthropic 暂固定 0）
    pub total_tokens: u64,
}
```

（把结尾 `}` 替换为上面两行 + `}`；`total_tokens` 不加 `#[allow(dead_code)]`——Task 2 起被读取）

- [ ] **Step 2: provider.rs 两个解析函数填充字段**

`kissbot-agent/src/provider.rs` `parse_openai_response`：在函数开头取 usage（`let choice` 之前）：

```rust
fn parse_openai_response(data: &serde_json::Value) -> ModelResponse {
    // usage.total_tokens：本次请求 prompt+completion token 总占用（DeepSeek/Kimi 均返回）；缺字段回退 0
    let total_tokens = data["usage"]["total_tokens"].as_u64().unwrap_or(0);
    let choice = &data["choices"][0];
```

并在 `ModelResponse { ... }` 字面量末尾（`finish_reason` 之后）增加：

```rust
        finish_reason: Arc::new(finish_reason),
        total_tokens,
    }
```

`parse_anthropic_response`：在 `ModelResponse { ... }` 字面量末尾增加：

```rust
        finish_reason: Arc::new(finish_reason),
        total_tokens: 0,   // Anthropic API 文档未找到，暂固定 0（永不触发重置）
    }
```

- [ ] **Step 3: 写失败测试**（provider.rs `mod tests` 末尾追加）

```rust
    #[test]
    fn parse_openai_response_extracts_total_tokens() {
        // DeepSeek/Kimi 非流式响应：usage.total_tokens = prompt + completion
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.total_tokens, 15, "应取 usage.total_tokens");
    }

    #[test]
    fn parse_openai_response_missing_usage_defaults_zero() {
        // 无 usage 字段（容错）→ 0
        let data = serde_json::json!({
            "choices": [{ "message": { "content": "答案" }, "finish_reason": "stop" }]
        });
        let resp = parse_openai_response(&data);
        assert_eq!(resp.total_tokens, 0, "缺 usage 回退 0");
    }

    #[test]
    fn parse_anthropic_response_total_tokens_always_zero() {
        let data = serde_json::json!({
            "content": [{ "type": "text", "text": "答复" }],
            "stop_reason": "end_turn"
        });
        let resp = parse_anthropic_response(&data);
        assert_eq!(resp.total_tokens, 0, "anthropic 暂固定 0");
    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test parse_openai_response_extracts_total_tokens && cargo test parse_openai_response_missing_usage_defaults_zero && cargo test parse_anthropic_response_total_tokens_always_zero`
Expected: 3 个测试 PASS（既有测试不受影响：无 usage 的用例 total_tokens 均为 0）

- [ ] **Step 5: 运行全量测试**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 全量 PASS

- [ ] **Step 6: Commit**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/src/types.rs kissbot-agent/src/provider.rs && git commit -m "ModelResponse 增加 total_tokens：openai 解析 usage.total_tokens（DeepSeek/Kimi），anthropic 暂固定 0，缺字段回退 0"
```

---

### Task 2: 配置结构变更 + 下游修复 + 会话延迟检查改造

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`（删常量、改 `ModelConfig` / `EffectiveModelConfig` / `merge_model_config`、更新与新增测试）
- Modify: `kissbot-agent/src/provider.rs`（`sample_effective` 测试夹具）
- Modify: `kissbot-agent/src/http_server.rs`（`test_provider` 测试夹具）
- Modify: `kissbot-agent/src/nexus.rs`（第 29-30 行注释）
- Modify: `kissbot-agent/src/session_manager.rs`（删 `is_overflow`、`Session` 加 `last_total_tokens`、`run_agentic_loop` 改造、新增 `should_reset`、测试夹具与断言）

**Interfaces:**
- Consumes: `ModelResponse.total_tokens: u64`（Task 1）
- Produces: `EffectiveModelConfig.max_tokens_usage: u32`、`session_manager::should_reset(last_total_tokens: u64, max_tokens_usage: u32) -> bool`、`Session.last_total_tokens: AtomicU64`——Task 3 配置模板、Task 4 e2e 消费

- [ ] **Step 1: config_manager.rs 删除两个默认常量**

删除 `kissbot-agent/src/config_manager.rs` 中：

```rust
/// 模型默认上下文长度（token）
pub const DEFAULT_CONTEXT_LENGTH: u32 = 65536;
/// 模型默认上下文消息条数上限（溢出触发重置/压缩）
pub const DEFAULT_MAX_CONTEXT_MESSAGES: usize = 100;
```

（保留 `DEFAULT_MAX_TOKENS` / `DEFAULT_TIMEOUT_SECS` / `DEFAULT_RETRY_COUNT` 不动）

- [ ] **Step 2: config_manager.rs 改 EffectiveModelConfig**

`EffectiveModelConfig` 中删除 `context_length`、`max_context_messages` 两字段（含各自注释），改为：

```rust
    pub max_tokens: u32,
    /// token 占用上限（必填：usage.total_tokens 超过其 80% 触发会话重置）
    pub max_tokens_usage: u32,
    pub temperature: Option<f32>,
```

- [ ] **Step 3: config_manager.rs 改 ModelConfig 与 ProviderConfig**

`ModelConfig`：删除 `context_length`、`max_context_messages` 两个 Option 字段，新增必填 `max_tokens_usage`：

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// token 占用上限（必填非 Option：provider 默认与 model 覆盖均须声明，缺失解析失败——
    /// 破坏性变更；usage.total_tokens 超过其 80% 触发会话重置）
    pub max_tokens_usage: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
```

`ProviderConfig.default_model_config` 去掉 `#[serde(default)]`（段必填，旧配置无该段解析失败）：

```rust
    /// provider 默认模型参数（必填段：max_tokens_usage 必须声明；旧配置无 default_model_config 解析失败）
    pub default_model_config: ModelConfig,
```

- [ ] **Step 4: config_manager.rs 改 merge_model_config**

删除 `context_length` / `max_context_messages` 两行合成，新增：

```rust
        max_tokens: model.and_then(|m| m.max_tokens).or(d.max_tokens).unwrap_or(DEFAULT_MAX_TOKENS),
        // max_tokens_usage 必填非 Option：model 覆盖有值直接用，无该 model 键时用 provider 默认（无全局回落）
        max_tokens_usage: model.map(|m| m.max_tokens_usage).unwrap_or(d.max_tokens_usage),
        temperature: model.and_then(|m| m.temperature).or(d.temperature),
```

- [ ] **Step 5: 更新 config_manager.rs 既有测试**

`sample_provider`（约 945 行）：`default_model_config` 字面量改为：

```rust
            default_model_config: ModelConfig {
                max_tokens: Some(4096),
                max_tokens_usage: 128000,
                timeout_secs: Some(60),
                retry_count: Some(3),
                temperature: Some(0.7),
                thinking: None,
                reasoning_effort: None,
            },
```

`provider_config_old_shape_migration` 测试整体替换为（旧扁平格式无 `default_model_config` 段 → 解析失败，破坏性变更）：

```rust
    #[test]
    fn provider_config_old_shape_no_longer_parses() {
        // 旧扁平 default_* 字段格式无 default_model_config 段 → 解析失败（破坏性变更，配置文件需迁移）
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
            "default_max_context_messages": 100,
            "models": {}
        }"#;
        assert!(serde_json::from_str::<ProviderConfig>(old).is_err(), "旧格式缺 default_model_config 应解析失败");
    }

    #[test]
    fn provider_config_missing_max_tokens_usage_fails() {
        // 必填语义：default_model_config 段内缺 max_tokens_usage → 解析失败
        let json = r#"{
            "name": "deepseek",
            "provider_type": "openai",
            "base_url": "https://api.deepseek.com",
            "api_key": "sk-test",
            "default_model_config": { "max_tokens": 4096 },
            "models": {}
        }"#;
        assert!(serde_json::from_str::<ProviderConfig>(json).is_err(), "缺 max_tokens_usage 应解析失败");
    }
```

`provider_config_nested_roundtrip`：删 `context_length`/`max_context_messages` 两行断言，改为：

```rust
        assert_eq!(back.default_model_config.max_tokens, Some(4096));
        assert_eq!(back.default_model_config.max_tokens_usage, 128000);
        assert_eq!(back.default_model_config.timeout_secs, Some(60));
```

`merge_model_provider_partial_defaults_fall_back_to_globals` 与 `merge_model_provider_all_default_and_model_overrides`：
- 两个 `ModelConfig { ... }` 字面量删除 `context_length: None,`、`max_context_messages: None,` 行，各加 `max_tokens_usage: 128000,`（partial 测试）与 `max_tokens_usage: 262144,`（all-default 测试，验证覆盖）
- 断言删除 `assert_eq!(eff.context_length, DEFAULT_CONTEXT_LENGTH, ...)`、`assert_eq!(eff.max_context_messages, DEFAULT_MAX_CONTEXT_MESSAGES)` 两行
- partial 测试加：`assert_eq!(eff.max_tokens_usage, 128000, "provider 默认值生效");`
- all-default 测试加：`assert_eq!(eff.max_tokens_usage, 262144, "model 覆盖生效");`

`resolve_effective_config_merges_provider_and_model`：`ModelConfig` 字面量删两字段加 `max_tokens_usage: 131072,`；断言删 `max_context_messages` 两行，改加：

```rust
        assert_eq!(eff.max_tokens_usage, 131072, "model 的 max_tokens_usage 应生效");
```

`resolve_effective_config_inherits_provider_defaults`：`ModelConfig` 字面量删 `context_length: Some(131072),`、`max_context_messages: None,` 两行，加 `max_tokens_usage: 131072,`；断言删两行，改加：

```rust
        assert_eq!(eff.max_tokens_usage, 131072, "model 覆盖 max_tokens_usage 应生效");
```

`resolve_effective_config_missing_returns_none` 末尾追加断言：

```rust
        assert_eq!(eff.max_tokens_usage, 128000, "model 未配置时用 provider 默认");
```

（`resolve_effective_config_synthesizes_unconfigured_model`、`provider_crud_and_default_set` 等其它测试若构造 `ModelConfig` 字面量，同样删两字段并补 `max_tokens_usage`——以 `cargo build` 编译错误为准逐一修复）

- [ ] **Step 6: 修复下游编译（provider / http_server / nexus）**

`kissbot-agent/src/provider.rs` `sample_effective`：删除两字段，改为：

```rust
            max_tokens: 2048,
            max_tokens_usage: 128000,
            temperature: Some(0.3),
            timeout_secs: 30,
            retry_count: 2,
            thinking: None,
            reasoning_effort: None,
```

`kissbot-agent/src/http_server.rs` `test_provider`：删除两字段，改为：

```rust
            default_model_config: crate::config_manager::ModelConfig {
                max_tokens: Some(4096),
                max_tokens_usage: 128000,
                timeout_secs: Some(60),
                retry_count: Some(3),
                temperature: Some(0.7),
                thinking: None,
                reasoning_effort: None,
            },
```

`kissbot-agent/src/nexus.rs` 第 29-30 行注释替换为：

```rust
// 上下文重置阈值来自会话模型 effective.max_tokens_usage（provider/model 配置合成）：
// 最近一次模型响应的 usage.total_tokens 超过其 80% 时触发重置，见 run_agentic_loop 检查。
```

- [ ] **Step 7: session_manager.rs 改造**

7a. 删除 `SessionContext::is_overflow`（含注释，约 205-207 行）：

```rust
    /// 检查是否超长（threshold 来自模型 effective 配置的 max_context_messages）
    pub fn is_overflow(&self, max: usize) -> bool {
        self.messages.len() >= max
    }
```

7b. `Session` 结构体（`notify: Arc<Notify>` 之后）新增字段：

```rust
    /// 最近一次模型响应的 token 总占用（usage.total_tokens；0 = 尚无请求或已重建）。
    /// 每次请求成功后更新；run_agentic_loop 开头检查是否触发重置（延迟检查，无新消息不重置）
    last_total_tokens: AtomicU64,
```

7c. `run_agentic_loop` 中原 overflow 检查块（`// 1. 检查上下文超长...` 到 `if overflow {` 行）替换为：

```rust
        // 1. 检查上下文 token 占用超限（阈值来自会话模型的 effective.max_tokens_usage；延迟检查：
        //    上次模型响应的 usage.total_tokens 超过 80% 触发；无新消息不触发）
        let should_reset = {
            let model = self.model.load_full();
            match model.as_ref() {
                Some(pm) => match ConfigManager::get().resolve_effective_config(pm).await {
                    Some(eff) => should_reset(self.last_total_tokens.load(Ordering::Relaxed), eff.max_tokens_usage),
                    None => false,
                },
                None => false,
            }
        };
        if should_reset {
```

（`if overflow {` 改为 `if should_reset {`；块内 `Mode::Event(_) => { ... }` 与 `Mode::Role => { ... }` 原逻辑保持不变；在重建 `match` 结束后、`// 2. tools 聚合` 之前插入清零）：

```rust
            // 重建后清空 usage 记录（新上下文尚未产生 token 占用，避免下次立即再触发）
            self.last_total_tokens.store(0, Ordering::Relaxed);
        }
```

7d. agentic loop 内 `match response { Ok(model_resp) => {` 分支开头（`let now = ...` 之前）插入：

```rust
                Ok(model_resp) => {
                    // 保存本次请求 token 占用（下次请求开头检查是否触发重置；工具轮次每次成功后均更新）
                    self.last_total_tokens.store(model_resp.total_tokens, Ordering::Relaxed);
                    let now = Arc::new(Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
```

7e. 文件末尾 `mod tests` 之前（自由函数区，`write_cache_lines` 之后）新增：

```rust
/// token 占用重置判定：usage 超过 max_tokens_usage 的 80%（整数运算避免浮点；u64 提升防 u32 乘法溢出）
/// max_tokens_usage = 0 视为未启用（显式 0 永不触发，防御性兜底）
fn should_reset(last_total_tokens: u64, max_tokens_usage: u32) -> bool {
    max_tokens_usage > 0 && last_total_tokens * 10 > (max_tokens_usage as u64) * 8
}
```

7f. 测试夹具修复：`test_pair` 与 `session_copies_role_name_and_mode_from_key` 中的 `Session { ... }` 字面量在 `notify` 之后加 `last_total_tokens: AtomicU64::new(0),`

- [ ] **Step 8: 写 should_reset 测试**（`mod tests` 末尾追加）

```rust
    #[test]
    fn should_reset_triggers_only_above_80_percent() {
        assert!(!should_reset(0, 128000), "无 usage 不触发");
        assert!(!should_reset(80, 100), "恰好 80% 不触发（严格大于）");
        assert!(should_reset(81, 100), "超过 80% 触发");
        assert!(!should_reset(102400, 128000), "恰好 80% 不触发");
        assert!(should_reset(102401, 128000), "超过 80% 触发");
    }

    #[test]
    fn should_reset_never_for_zero_budget() {
        assert!(!should_reset(100, 0), "max_tokens_usage=0 视为未启用永不触发");
        assert!(!should_reset(0, 0));
    }
```

- [ ] **Step 9: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test`
Expected: 编译通过，全量 PASS（config / provider / session_manager / http_server 测试全部更新后通过）

- [ ] **Step 10: Commit**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/src/ && git commit -m "上下文重置改为 token 占用驱动：删除 context_length/max_context_messages 配置，新增必填 max_tokens_usage；会话记录最近 usage.total_tokens，下次请求开头超过 80% 触发重建（event 压缩/role 记忆打包）后清零，is_overflow 消息条数判断废弃"
```

---

### Task 3: 配置模板与文档同步

**Files:**
- Modify: `script/template/nexus.json`
- Modify: `script/README.md`
- Modify: `test/workspace-template/agent-data/nexus.json`
- Modify: `test/workspace/agent-data/nexus.json`（git 已跟踪的运行产物，与模板保持一致）

**Interfaces:**
- Consumes: `ModelConfig.max_tokens_usage`（Task 2）——缺字段会导致 agent 启动解析失败，本任务必须完成才能跑 agent

- [ ] **Step 1: 更新 script/template/nexus.json**

`default_model_config` 段（第 21-29 行）删除两字段，改为：

```json
      "default_model_config": {
        "max_tokens": 4096,
        "max_tokens_usage": 128000,
        "timeout_secs": 60,
        "retry_count": 3,
        "thinking": "enabled",
        "reasoning_effort": "low"
      },
```

- [ ] **Step 2: 更新 test/workspace-template/agent-data/nexus.json 与 test/workspace/agent-data/nexus.json**

两处 `default_model_config` 段做同样修改（删除 `context_length`、`max_context_messages`，加 `"max_tokens_usage": 128000`；`test/workspace` 由 `resetWorkspace()` 从模板复制，保持 git 内容一致即可）

- [ ] **Step 3: 更新 script/README.md**

第 28-29 行示例 JSON 中 `default_model_config` 同样改为含 `"max_tokens_usage": 128000`（删除两字段）。

第 66 行说明改写为：

```
- `default_model_config` 承载 provider 默认模型参数（`max_tokens` / `max_tokens_usage` / `timeout_secs` / `retry_count` / `temperature` / `thinking` / `reasoning_effort`，未声明的数值字段回落全局默认 4096 / 60 / 3，`temperature`/`thinking`/`reasoning_effort` 未配时不发送（无全局默认）；`max_tokens_usage` 为必填项（旧配置缺该字段解析失败））；会话记录最近一次模型响应的 `usage.total_tokens`，下次请求开头超过 `max_tokens_usage` 的 80% 时触发重置（event 模式压缩、role 模式归档重建），无新消息不触发
```

第 66 行附近「迁移注意」段落中提及的默认回落值 `4096/65536/100/60/3` 改为 `4096/60/3`，并补充 `max_tokens_usage` 必填迁移说明（旧配置需删除 `context_length`、`max_context_messages` 并新增 `max_tokens_usage`）。

- [ ] **Step 4: 校验 JSON 与一致性**

Run: `cd /home/admin/project/kissbot && python3 -c "import json; [json.load(open(p)) for p in ['script/template/nexus.json','test/workspace-template/agent-data/nexus.json','test/workspace/agent-data/nexus.json']]; print('json ok')"`
Expected: `json ok`（只读校验，不修改文件——项目禁止用 python 改文件，仅校验）

（若环境无 python3，改用 `node -e "JSON.parse(require('fs').readFileSync('script/template/nexus.json')); ..."`）

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot && git add script/template/nexus.json script/README.md test/workspace-template/agent-data/nexus.json test/workspace/agent-data/nexus.json && git commit -m "配置模板与文档同步 max_tokens_usage：nexus.json 模板/测试 workspace/README 移除 context_length、max_context_messages，新增必填 max_tokens_usage（128000），README 说明 80% 阈值延迟重置语义"
```

---

### Task 4: e2e 配置 API 测试适配

**Files:**
- Modify: `test/tests/agent-config-api.spec.ts`

**Interfaces:**
- Consumes: `max_tokens_usage` 必填（Task 2）——TC-02 不带该字段将返回失败

- [ ] **Step 1: 更新 TC-02**

`test/tests/agent-config-api.spec.ts` 第 56 行 `default_model_config` 中 `max_tokens: 8192, context_length: 200000, temperature: 0.7,` 改为 `max_tokens: 8192, max_tokens_usage: 200000, temperature: 0.7,`（删除 `context_length`）。

第 70 行断言 `expect(saved.providers.anthropic.default_model_config.context_length).toBe(200000);` 改为：

```typescript
    expect(saved.providers.anthropic.default_model_config.max_tokens_usage).toBe(200000);
```

- [ ] **Step 2: 运行 e2e 测试**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/agent-config-api.spec.ts`
Expected: 6 个用例全部 PASS（TC-02 验证 max_tokens_usage 添加并落盘）

- [ ] **Step 3: Commit**

```bash
cd /home/admin/project/kissbot && git add test/tests/agent-config-api.spec.ts && git commit -m "e2e 配置 API 测试适配：TC-02 移除 context_length 断言，改为必填 max_tokens_usage 添加与落盘验证"
```

---

## Self-Review 记录

- **Spec 覆盖**：删两配置（Task 2 Step 1-4）✓；必填 max_tokens_usage（Task 2 Step 3 + 解析失败测试）✓；total_tokens 解析 openai/anthropic（Task 1）✓；80% 延迟检查 + 重建后清零 + 每次请求保存（Task 2 Step 7）✓；配置文件/文档/测试同步（Task 3、Task 4）✓；新增单测（Task 1 Step 3、Task 2 Step 8）✓
- **占位扫描**：无占位
- **类型一致性**：`should_reset(u64, u32) -> bool`、`ModelResponse.total_tokens: u64`、`EffectiveModelConfig.max_tokens_usage: u32`、`ModelConfig.max_tokens_usage: u32`（非 Option）、`Session.last_total_tokens: AtomicU64` 在 Task 间一致
