# Provider/Model 配置按字段默认：ModelConfig 提取公共结构 + Provider 嵌套 default_model_config

日期：2026-08-14
状态：设计已确认

## 1. 目标

`ProviderConfig` 的 8 个 `default_*` 字段改为按字段可缺省：未配字段回落全局默认常量。落地方式（与 ContextConfig 同模式）：
`ModelConfig` 保留名并提取为"可继承模型参数"公共结构（纯 Option 字段，移除冗余 `model` 标识字段），
`ProviderConfig` 用单个嵌套字段 `default_model_config: ModelConfig` 替代 8 个扁平 `default_*` 字段，
`models` map 值复用同一 `ModelConfig`。

继承链语义：全局默认 ← provider 默认 ← model 覆盖，逐字段三层回落（model 未配 → provider 默认 → 全局常量）。
`temperature` / `thinking` / `reasoning_effort` 保持 Option 且无全局常量（None = 不发送）。

改造范围：`kissbot-agent/src/config_manager.rs` + 其测试 + `script/template/nexus.json` + `script/README.md`。

## 2. 核心决策

| # | 决策 | 说明 |
|---|------|------|
| 1 | `ModelConfig` 提取为公共结构，**移除 `model` 标识字段** | 全仓无读取方（merge 用 `model_name` 参数、`resolve_effective_config` 用 `pm.model`、前端/其他模块仅用 `EffectiveModelConfig`）；models map key 承载模型名；旧配置含 `model` 字段被 serde 忽略 |
| 2 | `ModelConfig` 加 `#[derive(Default)]` | 全 Option 字段，Default = 全 None；配合 `#[serde(default)]` 使缺省可用 |
| 3 | `ProviderConfig` 用嵌套 `default_model_config: ModelConfig` | `#[serde(default)]`；替代 8 个扁平 `default_*` 字段；`models: Arc<ArcSwapHashMap<String, ModelConfig>>` 不变 |
| 4 | 全局默认常量（取现有模板值） | `DEFAULT_MAX_TOKENS: u32 = 4096`、`DEFAULT_CONTEXT_LENGTH: u32 = 65536`、`DEFAULT_MAX_CONTEXT_MESSAGES: usize = 100`、`DEFAULT_TIMEOUT_SECS: u64 = 60`、`DEFAULT_RETRY_COUNT: u32 = 3`；`temperature`/`thinking`/`reasoning_effort` 无全局常量（None 传播） |
| 5 | merge 逐字段三层回落 | `model.and_then(|m| m.x).or(d.x).unwrap_or(DEFAULT_X)`（d = `&provider.default_model_config`）；`temperature`/`thinking`/`reasoning_effort` 用 `.or()` None 传播；`EffectiveModelConfig` / `resolve_effective_config` / `provider_config_by_name` 签名不变，消费方零改动 |
| 6 | 配置格式迁移：模板/README 同步 | provider 段 `default_*` 扁平字段 → `default_model_config` 嵌套（字段去 `default_` 前缀）；存量手动迁移，不做双格式兼容 |
| 7 | 测试更新 + 新增缺省回落用例 | `sample_provider` helper 改嵌套；ModelConfig 构造删 `model:` 行；新增"provider 部分缺省回落全局""provider 全缺省 + model 覆盖"用例 |

## 3. 改动清单

### 3.1 config_manager.rs

```rust
// ---- 全局默认常量（provider/model 未配字段回落；值 = 原模板必填值） ----
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
pub const DEFAULT_CONTEXT_LENGTH: u32 = 65536;
pub const DEFAULT_MAX_CONTEXT_MESSAGES: usize = 100;
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub const DEFAULT_RETRY_COUNT: u32 = 3;

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

pub struct ProviderConfig {
    pub name: Arc<String>,               // provider 名（providers map 的 key）
    pub provider_type: String,           // "openai" | "anthropic"，决定 Provider 实现
    pub base_url: String,                // URL 前缀，如 https://api.deepseek.com
    pub api_key: String,                 // provider 级密钥
    /// provider 默认模型参数（未配字段回落全局常量；缺省 = 全 None = 全局默认）
    #[serde(default)]
    pub default_model_config: ModelConfig,
    pub models: Arc<ArcSwapHashMap<String, ModelConfig>>,  // key = model 标识
}
```

- `merge_model_config`（约 171-196 行）改为三层回落：

```rust
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

- `EffectiveModelConfig` / `resolve_effective_config` / `provider_config_by_name` / `add_provider` / `remove_provider` 签名不变（`resolve_effective_config` 用 `pm.model` 作 key，天然适配）

### 3.2 测试（config_manager.rs tests mod）

- `sample_provider` helper（约 903 行）：8 个 `default_*` 直接值 → `default_model_config: ModelConfig { max_tokens: Some(4096), context_length: Some(65536), max_context_messages: Some(100), timeout_secs: Some(60), retry_count: Some(3), temperature: Some(0.7), thinking: None, reasoning_effort: None }`
- ModelConfig 构造 3 处（约 960/1011/1130 行）：删 `model: "..."` 行
- `resolve_effective_config_inherits_provider_defaults` 等测试：`provider.default_thinking = ...` → `provider.default_model_config.thinking = ...`（同 reasoning_effort）
- 新增 `merge_model_config_provider_partial_defaults_fall_back_to_globals`：provider 的 `default_model_config` 部分字段 None → 回落 `DEFAULT_*`；model 覆盖仍生效
- 新增 `merge_model_config_provider_all_default_and_model_overrides`：provider 全缺省（`ModelConfig::default()`）+ model 覆盖
- `provider_config_old_shape_migration` 测试：旧格式反序列化（default_temperature 数值、无 thinking）——字段变化后断言适配（旧扁平字段被忽略 → `default_model_config` 缺省回落全局）

### 3.3 模板与文档（迁移）

- `script/template/nexus.json` providers 段：扁平 `default_*` → 嵌套，例：
  ```json
  "providers": {
    "deepseek": {
      "name": "deepseek",
      "provider_type": "openai",
      "base_url": "https://api.deepseek.com",
      "api_key": "...",
      "default_model_config": {
        "max_tokens": 4096,
        "context_length": 65536,
        "max_context_messages": 100,
        "timeout_secs": 60,
        "retry_count": 3,
        "temperature": 0.3,
        "thinking": "enabled",
        "reasoning_effort": "low"
      },
      "models": {}
    }
  }
  ```
  （值按模板实际内容迁移，字段去 `default_` 前缀）
- `script/README.md` provider 示例同步 + 迁移注意补旧扁平 `default_*` 字段说明（同 context 迁移注意模式）

## 4. 数据流 / 错误处理 / 测试

- **读**：`resolve_effective_config()` 现场合成 `EffectiveModelConfig`（逐字段三层回落），消费方（model_client / provider / coordinator）无感知
- **写**：管理 API `add_provider` 接受完整 `ProviderConfig`（`default_model_config` 嵌套）；`provider_config_old_shape_migration` 验证旧格式兼容
- **错误处理**：无新增错误路径（serde default 静默回落）
- **测试**：cargo test 全绿（118 + 2 新增 + 既有适配）

## 5. 已知取舍

- 旧扁平 `default_*` 字段配置反序列化时 `default_model_config` 缺省回落全 None——旧值静默丢失（如 `default_max_tokens: 4096` 不生效）。决策 6 接受（自托管 + 模板同步），README 迁移注意文档化
- `ModelConfig.model` 标识字段移除——若未来有按 value 取模型名的消费需求，改从 map key 取

## 6. 验证

- `cargo check` 通过（无警告）
- `cargo test` 全绿（118 + 2 新增 = 120）
- `rg "default_max_tokens|default_context_length|default_max_context_messages|default_timeout_secs|default_retry_count|default_temperature|default_thinking|default_reasoning_effort"`（作为字段名）在源码/模板零残留（docs/superpowers 归档允许）
