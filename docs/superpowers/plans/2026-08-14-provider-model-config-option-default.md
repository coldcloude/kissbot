# Provider/Model 配置按字段默认实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ProviderConfig` 的 8 个扁平 `default_*` 字段改为按字段可缺省：`ModelConfig` 提取为公共结构（纯 Option 字段、移除冗余 `model` 标识字段、加 `#[derive(Default)]`），`ProviderConfig` 用嵌套 `default_model_config: ModelConfig` 承载，未配字段逐字段回落全局默认常量。

**Architecture:** 与已完成的 ContextConfig 方案同模式：`ModelConfig` 复用作"provider 默认值容器"与"model 覆盖"两种角色；`merge_model_config` 逐字段三层回落 `model.x.or(provider.default.x).unwrap_or(DEFAULT_X)`；temperature/thinking/reasoning_effort 保持 Option 且无全局常量（None 传播）；`EffectiveModelConfig` / `resolve_effective_config` 签名不变，消费方零改动。配置格式从扁平 `default_*` 迁移为嵌套 `default_model_config`（模板同步，存量手动迁移）。

**Tech Stack:** Rust（serde 属性），kissbot-agent crate。

## Global Constraints

- 改造范围：`kissbot-agent/src/config_manager.rs`、`script/template/nexus.json`、`script/README.md`
- 不要删除代码中的注释（项目 CLAUDE.md 规则）；被改注释同步更新保持准确
- `ModelConfig`：纯 Option 字段（8 个）+ `#[derive(Default)]`；**移除 `model: String` 标识字段**（全仓无读取方，models map key 承载）
- `ProviderConfig`：`#[serde(default)] pub default_model_config: ModelConfig` 替代 8 个扁平 `default_*`；`models: Arc<ArcSwapHashMap<String, ModelConfig>>` 不变
- 全局常量（模板现有值）：`DEFAULT_MAX_TOKENS: u32 = 4096`、`DEFAULT_CONTEXT_LENGTH: u32 = 65536`、`DEFAULT_MAX_CONTEXT_MESSAGES: usize = 100`、`DEFAULT_TIMEOUT_SECS: u64 = 60`、`DEFAULT_RETRY_COUNT: u32 = 3`
- merge：`model.and_then(|m| m.x).or(d.x).unwrap_or(DEFAULT_X)`（d = `&provider.default_model_config`）；temperature/thinking/reasoning_effort 用 `.or()` None 传播
- 模板 provider 段改嵌套格式；旧扁平 `default_*` 字段在源码/模板零残留
- 每任务结束：`cargo check` 无警告、`cargo test` 全绿（118 + 2 新增 = 120）、git commit（中文 comment）
- 工作目录：cargo 命令在 `/home/admin/project/kissbot/kissbot-agent`；模板文件在 `/home/admin/project/kissbot` 下

---

### Task 1: config_manager.rs 结构变更 + 全局常量 + 三层合并 + 测试

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`

**Interfaces:**
- Consumes: 无
- Produces: `ModelConfig`（纯 Option 8 字段 + Default + 无 model 字段）；`ProviderConfig.default_model_config: ModelConfig`；`DEFAULT_*` 常量 5 个；`merge_model_config(provider: &ProviderConfig, model: Option<&ModelConfig>, model_name: &str) -> EffectiveModelConfig`（三层回落）；`EffectiveModelConfig` / `resolve_effective_config` / `provider_config_by_name` 签名不变

- [ ] **Step 1: 加全局默认常量**

`config_manager.rs` 全局常量区（约 29-36 行，context 常量之后）追加：

```rust
// ---- 全局默认值（provider/model 未配字段回落；值 = 原模板必填值） ----

/// 模型默认最大输出 token 数
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
/// 模型默认上下文长度（token）
pub const DEFAULT_CONTEXT_LENGTH: u32 = 65536;
/// 模型默认上下文消息条数上限（溢出触发重置/压缩）
pub const DEFAULT_MAX_CONTEXT_MESSAGES: usize = 100;
/// 模型默认请求超时（秒）
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// 模型默认重试次数
pub const DEFAULT_RETRY_COUNT: u32 = 3;
```

- [ ] **Step 2: ModelConfig 提取为公共结构（去 model 字段）**

替换 `ModelConfig` 定义（约 149-167 行，含 `pub model: String` 字段）为：

```rust
/// 可继承模型参数（Option 覆盖字段；未配字段回落上一级：model → provider 默认 → 全局常量）
/// 复用作 provider 默认值容器（ProviderConfig.default_model_config）与 model 覆盖（models map 值）
/// 注：model 标识由 models map key 承载（旧 ModelConfig.model 冗余字段已移除，全仓无读取方）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}
```

- [ ] **Step 3: ProviderConfig 改嵌套 default_model_config**

替换 `ProviderConfig` 定义（约 108-147 行）——删除 8 个扁平 `default_*` 字段（`default_context_length` / `default_max_tokens` / `default_temperature` / `default_timeout_secs` / `default_retry_count` / `default_thinking` / `default_reasoning_effort` / `default_max_context_messages`），加入嵌套字段：

```rust
pub struct ProviderConfig {
    pub name: Arc<String>,               // provider 名（providers map 的 key）
    pub provider_type: String,           // "openai" | "anthropic"，决定 Provider 实现
    pub base_url: String,                // URL 前缀，如 https://api.deepseek.com（原 endpoint）
    pub api_key: String,                 // provider 级密钥
    /// provider 默认模型参数（未配字段回落全局常量；缺省 = 全 None = 全局默认）
    #[serde(default)]
    pub default_model_config: ModelConfig,
    pub models: Arc<ArcSwapHashMap<String, ModelConfig>>,  // key = model 标识
}
```

- [ ] **Step 4: merge_model_config 改三层回落**

替换 `merge_model_config` 函数体（约 171-196 行）为：

```rust
/// 合成 provider 默认 + model 覆盖的有效参数（与 merge_context_config 同模式：
/// 全局默认 ← provider 默认 ← model 覆盖，model 未配字段继承 provider，二者都未配回落全局常量；
/// temperature/thinking/reasoning_effort 无全局默认，None 传播（不发送））
pub fn merge_model_config(
    provider: &ProviderConfig,
    model: Option<&ModelConfig>,
    model_name: &str,
) -> EffectiveModelConfig {
    let d = &provider.default_model_config;
    EffectiveModelConfig {
        provider_type: provider.provider_type.clone(),
        base_url: provider.base_url.clone(),
        api_key: provider.api_key.clone(),
        model: model_name.to_string(),   // 用切换指令的模型名（未配置也有效）
        max_tokens: model.and_then(|m| m.max_tokens).or(d.max_tokens).unwrap_or(DEFAULT_MAX_TOKENS),
        temperature: model.and_then(|m| m.temperature).or(d.temperature),
        timeout_secs: model.and_then(|m| m.timeout_secs).or(d.timeout_secs).unwrap_or(DEFAULT_TIMEOUT_SECS),
        retry_count: model.and_then(|m| m.retry_count).or(d.retry_count).unwrap_or(DEFAULT_RETRY_COUNT),
        context_length: model.and_then(|m| m.context_length).or(d.context_length).unwrap_or(DEFAULT_CONTEXT_LENGTH),
        max_context_messages: model.and_then(|m| m.max_context_messages).or(d.max_context_messages).unwrap_or(DEFAULT_MAX_CONTEXT_MESSAGES),
        thinking: model.and_then(|m| m.thinking.clone()).or(d.thinking.clone()),
        reasoning_effort: model.and_then(|m| m.reasoning_effort.clone()).or(d.reasoning_effort.clone()),
    }
}
```

- [ ] **Step 5: 更新测试 helper 与构造**

`config_manager.rs` tests mod：

a) `sample_provider`（约 903-919 行）改嵌套：

```rust
    fn sample_provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: Arc::new(name.into()),
            provider_type: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "sk-test".into(),
            default_model_config: ModelConfig {
                max_tokens: Some(4096),
                context_length: Some(65536),
                max_context_messages: Some(100),
                timeout_secs: Some(60),
                retry_count: Some(3),
                temperature: Some(0.7),
                thinking: None,
                reasoning_effort: None,
            },
            models: Arc::new(ArcSwapHashMap::new()),
        }
    }
```

b) 3 处 `ModelConfig { model: "deepseek-4-flash".into(), ... }` 构造（约 960/1011/1130 行）删 `model:` 行。

c) `provider_config_old_shape_migration`（约 922-941 行）断言改造（旧扁平字段被 serde 忽略 → default_model_config 缺省回落）：

```rust
    #[test]
    fn provider_config_old_shape_migration() {
        // 旧格式：扁平 default_* 字段（default_temperature 数值、无 thinking/reasoning_effort）
        // 新结构：default_model_config 缺省（serde default）→ 全 None → 回落全局默认
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
        let pc: ProviderConfig = serde_json::from_str(old).unwrap();
        assert_eq!(pc.default_model_config.max_tokens, None, "旧扁平字段被忽略");
        assert_eq!(pc.default_model_config.temperature, None);
        assert_eq!(pc.default_model_config.thinking, None);
        assert_eq!(pc.default_model_config.reasoning_effort, None);
    }
```

d) `resolve_effective_config_inherits_provider_defaults`（约 1006-1007 行）：
`provider.default_thinking = Some("disabled".into());` → `provider.default_model_config.thinking = Some("disabled".into());`
`provider.default_reasoning_effort = Some("low".into());` → `provider.default_model_config.reasoning_effort = Some("low".into());`

e) 其余测试断言不变（model 覆盖值、provider 默认值经 sample_provider 保持，如 `assert_eq!(eff.context_length, 65536, "context_length 未配应继承 provider 默认")` 仍成立）。

- [ ] **Step 6: 新增缺省回落测试**

在 tests mod 追加两个测试（复用 sample_provider 改字段，避免长构造）：

```rust
    #[test]
    fn merge_model_provider_partial_defaults_fall_back_to_globals() {
        // provider 默认仅配部分字段：未配字段回落全局常量；model 覆盖仍生效
        let mut provider = sample_provider("deepseek");
        provider.default_model_config = ModelConfig {
            max_tokens: Some(2048),
            context_length: None,
            max_context_messages: None,
            timeout_secs: None,
            retry_count: None,
            temperature: None,
            thinking: None,
            reasoning_effort: None,
        };
        let model = ModelConfig {
            max_tokens: None,
            context_length: None,
            max_context_messages: None,
            timeout_secs: Some(30),
            retry_count: None,
            temperature: None,
            thinking: None,
            reasoning_effort: None,
        };
        let eff = merge_model_config(&provider, Some(&model), "deepseek-4-flash");
        assert_eq!(eff.max_tokens, 2048, "provider 配了用 provider");
        assert_eq!(eff.context_length, DEFAULT_CONTEXT_LENGTH, "未配回落全局");
        assert_eq!(eff.max_context_messages, DEFAULT_MAX_CONTEXT_MESSAGES);
        assert_eq!(eff.timeout_secs, 30, "model 覆盖 provider");
        assert_eq!(eff.retry_count, DEFAULT_RETRY_COUNT);
        assert_eq!(eff.temperature, None, "无全局默认，None 传播");
    }

    #[test]
    fn merge_model_provider_all_default_and_model_overrides() {
        // provider 全缺省（ModelConfig::default()）+ model 覆盖
        let mut provider = sample_provider("deepseek");
        provider.default_model_config = ModelConfig::default();
        let model = ModelConfig {
            max_tokens: Some(4096),
            context_length: None,
            max_context_messages: None,
            timeout_secs: None,
            retry_count: None,
            temperature: None,
            thinking: None,
            reasoning_effort: None,
        };
        let eff = merge_model_config(&provider, Some(&model), "deepseek-4-flash");
        assert_eq!(eff.max_tokens, 4096, "model 覆盖生效");
        assert_eq!(eff.context_length, DEFAULT_CONTEXT_LENGTH, "provider 全缺省回落全局");
        assert_eq!(eff.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(eff.retry_count, DEFAULT_RETRY_COUNT);
        assert_eq!(eff.max_context_messages, DEFAULT_MAX_CONTEXT_MESSAGES);
    }
```

- [ ] **Step 7: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: check 无警告；`test result: ok. 120 passed`（118 + 2 新增）；`rg "default_max_tokens|default_context_length|default_max_context_messages|default_timeout_secs|default_retry_count" src/` 零残留（temperature/thinking/reasoning_effort 的旧 default_ 名应同样零残留：`rg "default_temperature|default_thinking|default_reasoning_effort" src/`）

- [ ] **Step 8: Commit**

```bash
git add kissbot-agent/src/config_manager.rs
git commit -m "refactor(agent): Provider/Model 配置按字段默认——ModelConfig 提取公共结构（去 model 标识、+Default），ProviderConfig 改嵌套 default_model_config，新增全局默认常量，merge 三层回落，测试适配+新增缺省回落用例"
```

---

### Task 2: nexus.json 模板与 README 迁移嵌套格式

**Files:**
- Modify: `script/template/nexus.json`
- Modify: `script/README.md`

**Interfaces:**
- Consumes: Task 1 的 `ProviderConfig { default_model_config: ModelConfig, models }` 格式
- Produces: 新嵌套格式的模板/文档（与 Task 1 反序列化契约一致）

- [ ] **Step 1: 迁移 script/template/nexus.json 的 providers 段**

`script/template/nexus.json` 的 `providers.deepseek` 段（约 20-29 行）：8 个扁平 `default_*` 改为嵌套 `default_model_config`（字段去 `default_` 前缀；模板无 `default_temperature`，保持省略；`thinking`/`reasoning_effort` 值按模板原样）：

```json
      "default_model_config": {
        "max_tokens": 4096,
        "context_length": 65536,
        "max_context_messages": 100,
        "timeout_secs": 60,
        "retry_count": 3,
        "thinking": "enabled",
        "reasoning_effort": "low"
      },
      "models": {}
```

- 先查看模板实际内容，按原值迁移，字段顺序可保持模板原序

- [ ] **Step 2: 迁移 script/README.md 示例**

`script/README.md` 的 provider 段示例（约 26-31 行）同步为嵌套格式；"迁移注意"节追加一句旧扁平 `default_*` 字段说明（与 context 迁移注意同风格）：

```
`providers.<provider>` 旧版扁平 `default_*` 字段（如 `default_max_tokens`）已不再识别，需迁移为 `default_model_config.<字段>`（字段名去掉 `default_` 前缀，如 `default_model_config.max_tokens`），否则旧值静默回落全局默认（4096/65536/100/60/3）
```

- [ ] **Step 3: 验证**

Run: `cd /home/admin/project/kissbot && python3 -m json.tool script/template/nexus.json > /dev/null && echo OK`
Expected: JSON 合法
Run: `cd /home/admin/project/kissbot && rg "default_max_tokens|default_context_length|default_max_context_messages|default_timeout_secs|default_retry_count|default_temperature|default_thinking|default_reasoning_effort" script/`
Expected: 零残留

- [ ] **Step 4: Commit**

```bash
git add script/template/nexus.json script/README.md
git commit -m "refactor(agent): provider 配置模板迁移嵌套格式——default_model_config 承载 provider 默认（字段去 default_ 前缀），README 示例与迁移注意同步"
```

---

### Task 3: 全量扫尾验证

**Files:**
- Verify only（发现问题才改）

- [ ] **Step 1: 残留检查**

Run: `cd /home/admin/project/kissbot && rg "default_max_tokens|default_context_length|default_max_context_messages|default_timeout_secs|default_retry_count|default_temperature|default_thinking|default_reasoning_effort" kissbot-agent/src script/template test/workspace-template test/workspace 2>/dev/null`
Expected: 源码与模板零残留（docs/superpowers 归档文档允许保留历史措辞，不检查）

- [ ] **Step 2: 全量验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test 2>&1 | tail -3`
Expected: check 无警告；`test result: ok. 120 passed`

- [ ] **Step 3: 提交（如有残留修复）**

```bash
git add -A
git commit -m "refactor(agent): provider/model 配置按字段默认扫尾（残留清理）"
```

---

## 自审

**1. Spec 覆盖：**
- 决策 1（ModelConfig 提取 + 去 model）→ Task 1 Step 2 ✓
- 决策 2（Default）→ Task 1 Step 2 ✓
- 决策 3（嵌套 default_model_config）→ Task 1 Step 3 ✓
- 决策 4（常量）→ Task 1 Step 1 ✓
- 决策 5（merge 三层）→ Task 1 Step 4 ✓
- 决策 6（模板迁移）→ Task 2 ✓
- 决策 7（测试）→ Task 1 Step 5/6 ✓
- 验证节 → Task 3 ✓

**2. 占位符扫描：** 无 TBD/TODO；每步含完整代码或精确指令。

**3. 类型一致性：**
- `ModelConfig` 8 个 Option 字段（无 model）与测试构造/`sample_provider` 一致 ✓
- `ProviderConfig.default_model_config: ModelConfig`（serde default）与模板嵌套字段一致 ✓
- `merge_model_config` 签名（`provider: &ProviderConfig, model: Option<&ModelConfig>`）与 `resolve_effective_config` 调用（`merge_model_config(&provider, model_cfg.as_deref(), &pm.model)`）一致——`model_cfg` 来自 `provider.models.get(&pm.model)` 的 `Arc<ModelConfig>`，类型不变 ✓
- 常量名/值（`DEFAULT_MAX_TOKENS: u32 = 4096` 等）与 merge 引用一致 ✓
- `provider_config_old_shape_migration` 断言 `pc.default_model_config.*` 与结构一致 ✓
