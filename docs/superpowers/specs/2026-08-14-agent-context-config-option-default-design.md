# AgentContextConfig 按字段默认：ContextConfig 复用 + 嵌套 default_context_config

日期：2026-08-14
状态：设计已确认

## 1. 目标

`AgentContextConfig` 的 5 个 `default_*` 字段改为按字段可缺省：未配字段回落全局默认常量（`DEFAULT_*`）。
落地方式：把"可选覆盖字段"结构（原 `RoleContextConfig`）改名为 `ContextConfig`，复用作 agent 默认值容器——`AgentContextConfig` 用单个嵌套字段 `default_context_config: ContextConfig` 替代 5 个扁平 `default_*` 字段，`roles` 复用同一 `ContextConfig`。

继承链语义不变：全局默认 ← agent 默认 ← role 覆盖，但从"agent 整体缺省才回落全局"细化为"**逐字段**回落"。

改造范围：`kissbot-agent/src/config_manager.rs` + 其测试 + 3 个 nexus.json 模板 + script/README.md 示例。

## 2. 核心决策

| # | 决策 | 说明 |
|---|------|------|
| 1 | `RoleContextConfig` 改名 `ContextConfig` | 结构复用为"agent 默认值容器"与"role 覆盖"两种角色，改名去除 role 专属语义；字段不变（5 个 Option） |
| 2 | `ContextConfig` 加 `#[derive(Default)]` | 全 Option 字段，Default = 全 None；配合 `#[serde(default)]` 使缺省可用 |
| 3 | `AgentContextConfig` 用嵌套 `default_context_config: ContextConfig` | `#[serde(default)]`；替代 5 个扁平 `default_*` 字段；`roles: Arc<ArcSwapHashMap<String, ContextConfig>>` 不变（A 决策：roles 不 Option，空 map = 无覆盖） |
| 4 | 合并逻辑逐字段三层回落 | `role.field.or(agent.default_context_config.field).unwrap_or(DEFAULT_*)`；`agent` 整体缺失返回全局默认（role 不可达）；`EffectiveContextConfig` / `context_config()` 签名不变，coordinator 等消费方零改动 |
| 5 | 配置格式迁移：模板/文档同步 + 存量手动 | 旧扁平格式（`default_channel_batch_interval_secs`）→ 新嵌套格式（`default_context_config: { channel_batch_interval_secs }`）；3 个 nexus.json 模板 + script/README.md 同步；存量 workspace 由 reset 脚本重建（kissbot 自托管，数据量小，不做反序列化兼容） |
| 6 | 测试更新 + 新增缺省回落用例 | 4 个既有测试构造改嵌套；新增"agent 部分字段缺省回落全局""agent 全缺省 + role 覆盖"用例 |

## 3. 改动清单

### 3.1 config_manager.rs

```rust
/// context 配置（可选覆盖字段；未配字段回落上一级：role → agent 默认 → 全局常量）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextConfig {
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

/// agent 级 context 配置（key = agent_id；default_context_config 未配字段回落全局默认常量）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContextConfig {
    /// agent 默认 context 配置（未配字段回落全局默认常量；缺省 = 全 None = 全局默认）
    #[serde(default)]
    pub default_context_config: ContextConfig,
    /// key = role_name（role 覆盖 agent 默认）
    pub roles: Arc<ArcSwapHashMap<String, ContextConfig>>,
}
```

- `merge_context_config`：`role: Option<&ContextConfig>`；agent Some 时 `let d = &a.default_context_config;`，每字段 `role.and_then(|r| r.x).or(d.x).unwrap_or(DEFAULT_X)`
- `context_config()` 与 `EffectiveContextConfig` 不变
- 注释同步（原"三层继承见 merge_context_config"表述更新为逐字段回落）

### 3.2 测试（config_manager.rs tests mod）

- `merge_none_uses_globals`：不变（None, None → 全局默认）
- `merge_agent_then_role_override` / `role_stations_override_agent` / `agent_role_config_serde_roundtrip`：agent 构造改 `default_context_config: ContextConfig { channel_batch_interval_secs: Some(5), ... }`（原 5 个 default_* 直接值 → Some）
- 新增 `merge_agent_partial_defaults_fall_back_to_globals`：agent 的 `default_context_config` 部分字段 None → 回落 `DEFAULT_*`；role 覆盖仍生效
- 新增 agent 全缺省（`default_context_config: ContextConfig::default()`）+ role 覆盖用例

### 3.3 模板与文档（迁移）

- `script/template/nexus.json`、`test/workspace-template/agent-data/nexus.json`、`test/workspace/agent-data/nexus.json`：
  context 段由扁平改嵌套，例：
  ```json
  "context": {
    "": {
      "default_context_config": {
        "channel_batch_interval_secs": 3,
        "memory_time_secs": 3600,
        "memory_count": 50,
        "compress_prompt": "请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。",
        "stations": ["local"]
      },
      "roles": {}
    }
  }
  ```
- `script/README.md` context 示例同步
- 存量 nexus.json（workspace 数据目录）由 reset 脚本重建或手动迁移，不做反序列化兼容（决策 5）

## 4. 数据流 / 错误处理 / 测试

- **读**：`context_config()` 现场合成 `EffectiveContextConfig`（逐字段回落），消费方（coordinator 等）无感知
- **写**：管理 API 无 context 编辑端点（仅 nexus.json 落盘），无回写路径变化
- **错误处理**：无新增错误路径（serde default 静默回落）
- **测试**：cargo test 全绿（4 个更新 + 2 个新增）；模板改动不参与编译

## 5. 已知取舍

- 旧扁平格式配置反序列化时 `default_context_config` 缺省回落全 None——旧字段值静默丢失（如 `default_stations: ["local"]` 不生效）。决策 5 接受（自托管 + reset 脚本重建），不做双格式兼容
- `roles` 保持非 Option（空 map = 无覆盖；无全局默认概念）

## 6. 验证

- `cargo check` 通过（无警告）
- `cargo test` 全绿（116 + 新增 2）
- `rg "RoleContextConfig"` 无残留
- `rg "default_channel_batch_interval_secs|default_memory_time_secs|default_memory_count|default_compress_prompt"`（作为字段名）仅存于文档归档（spec/plan 历史），源码/模板零残留
