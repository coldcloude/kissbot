# AgentContextConfig 按字段默认实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `AgentContextConfig` 的 5 个扁平 `default_*` 字段改为按字段可缺省：用嵌套 `default_context_config: ContextConfig`（原 `RoleContextConfig` 改名）承载，未配字段逐字段回落全局默认常量（`DEFAULT_*`）。

**Architecture:** `RoleContextConfig` 改名为 `ContextConfig`（字段不变、加 `#[derive(Default)]`），复用作"agent 默认值容器"与"role 覆盖"两种角色；`AgentContextConfig` 变为 `{ default_context_config: ContextConfig, roles: Arc<ArcSwapHashMap<String, ContextConfig>> }`；`merge_context_config` 逐字段 `role.field.or(agent.default.field).unwrap_or(DEFAULT_*)` 三层回落；`EffectiveContextConfig` / `context_config()` 签名不变，消费方零改动。配置格式从扁平 `default_*` 迁移为嵌套 `default_context_config`（模板同步，存量手动迁移）。

**Tech Stack:** Rust（serde 属性：`#[serde(default)]` / `skip_serializing_if`），kissbot-agent crate。

## Global Constraints

- 改造范围：`kissbot-agent/src/config_manager.rs`、3 个 nexus.json 模板（`script/template/nexus.json`、`test/workspace-template/agent-data/nexus.json`、`test/workspace/agent-data/nexus.json`）、`script/README.md`
- 不要删除代码中的注释（项目 CLAUDE.md 规则）；被改注释同步更新保持准确
- `RoleContextConfig` 改名 `ContextConfig`；`ContextConfig` 加 `#[derive(Default)]`
- `AgentContextConfig`：`#[serde(default)] pub default_context_config: ContextConfig` + `roles: Arc<ArcSwapHashMap<String, ContextConfig>>`（roles 不 Option）
- merge：`role.and_then(|r| r.x).or(d.x).unwrap_or(DEFAULT_X)`（d = `&a.default_context_config`）
- 模板 context 段改嵌套格式；旧扁平格式字段在模板/源码零残留
- 每任务结束：`cargo check` 无警告、`cargo test` 全绿、git commit（中文 comment）
- 工作目录：cargo 命令在 `/home/admin/project/kissbot/kissbot-agent`；模板文件在 `/home/admin/project/kissbot` 下

---

### Task 1: config_manager.rs 结构变更 + 逐字段合并 + 测试

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`

**Interfaces:**
- Consumes: 无（本次无跨任务依赖，单文件独立完成）
- Produces: `ContextConfig`（改名 + Default）；`AgentContextConfig { default_context_config, roles }`；`merge_context_config(agent: Option<&AgentContextConfig>, role: Option<&ContextConfig>) -> EffectiveContextConfig`（逐字段回落）；`context_config()` 与 `EffectiveContextConfig` 签名不变

- [ ] **Step 1: 改结构定义**

`config_manager.rs` 中替换 `RoleContextConfig` 定义（约 44-57 行）为 `ContextConfig`（加 Default）：

```rust
/// context 配置（可选覆盖字段；未配字段回落上一级：role → agent 默认 → 全局常量）
/// 复用作 agent 默认值容器（AgentContextConfig.default_context_config）与 role 覆盖（roles map 值）
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
```

替换 `AgentContextConfig` 定义（约 31-42 行）为嵌套结构：

```rust
/// agent 级 context 配置（key = agent_id，覆盖全局默认；未配字段回落全局默认常量）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContextConfig {
    /// agent 默认 context 配置（未配字段回落全局默认常量；缺省 = 全 None = 全局默认）
    #[serde(default)]
    pub default_context_config: ContextConfig,
    /// key = role_name（role 覆盖 agent 默认）
    pub roles: Arc<ArcSwapHashMap<String, ContextConfig>>,
}
```

- [ ] **Step 2: 改 merge_context_config**

替换 `merge_context_config`（约 68-96 行）为逐字段回落：

```rust
/// 三层逐字段合并：全局默认 ← agent 默认 ← role 覆盖（role Some 覆盖 agent；未配回落全局常量）。
/// role 只可能来自 agent.roles（role 无 agent 时不可达），故 agent 为 None 时直接返回全局默认。
/// 注：ContextConfig 全 Option——agent.default_context_config 与 role 各自缺省字段均回落全局常量。
pub fn merge_context_config(
    agent: Option<&AgentContextConfig>,
    role: Option<&ContextConfig>,
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
    let d = &a.default_context_config;
    EffectiveContextConfig {
        channel_batch_interval_secs: role.and_then(|r| r.channel_batch_interval_secs)
            .or(d.channel_batch_interval_secs)
            .unwrap_or(DEFAULT_CHANNEL_BATCH_INTERVAL_SECS),
        memory_time_secs: role.and_then(|r| r.memory_time_secs)
            .or(d.memory_time_secs)
            .unwrap_or(DEFAULT_MEMORY_TIME_SECS),
        memory_count: role.and_then(|r| r.memory_count)
            .or(d.memory_count)
            .unwrap_or(DEFAULT_MEMORY_COUNT),
        compress_prompt: role.and_then(|r| r.compress_prompt.as_ref().map(|s| s.to_string()))
            .or_else(|| d.compress_prompt.as_ref().map(|s| s.to_string()))
            .unwrap_or_else(|| DEFAULT_COMPRESS_PROMPT.to_string()),
        stations: role.and_then(|r| r.stations.clone())
            .map(|s| (*s).clone())
            .or_else(|| d.stations.clone().map(|s| (*s).clone()))
            .unwrap_or_default(),
    }
}
```

- [ ] **Step 3: 更新既有测试构造**

`config_manager.rs` tests mod（约 1180-1245 行）三个测试（`merge_agent_then_role_override` / `role_stations_override_agent` / `agent_role_config_serde_roundtrip`）：

- agent 构造改为嵌套（例）：

```rust
        let agent = AgentContextConfig {
            default_context_config: ContextConfig {
                channel_batch_interval_secs: Some(5),
                memory_time_secs: Some(7200),
                memory_count: Some(100),
                compress_prompt: Some(Arc::new("agent模板".into())),
                stations: Some(Arc::new(["s1".into()].into_iter().collect())),
            },
            roles: Arc::new(ArcSwapHashMap::new()),
        };
```

- role 构造：`RoleContextConfig { ... }` → `ContextConfig { ... }`（字段名与类型不变，仅类型名替换）
- `agent_role_config_serde_roundtrip` 断言：`assert_eq!(back.default_context_config.memory_count, Some(50));`
- 断言值不变（agent Some + role 覆盖语义不变；`merge_agent_then_role_override` 期望 `memory_time_secs == 7200` 等原值仍成立）

- [ ] **Step 4: 新增缺省回落测试**

在 tests mod 的 context 测试区追加两个测试：

```rust
    #[test]
    fn merge_agent_partial_defaults_fall_back_to_globals() {
        // agent 默认仅配部分字段：未配字段回落全局常量；role 覆盖仍生效
        let agent = AgentContextConfig {
            default_context_config: ContextConfig {
                channel_batch_interval_secs: Some(5),
                memory_time_secs: None,
                memory_count: None,
                compress_prompt: None,
                stations: None,
            },
            roles: Arc::new(ArcSwapHashMap::new()),
        };
        let role = ContextConfig {
            channel_batch_interval_secs: None,
            memory_time_secs: Some(7200),
            memory_count: None,
            compress_prompt: None,
            stations: None,
        };
        let eff = merge_context_config(Some(&agent), Some(&role));
        assert_eq!(eff.channel_batch_interval_secs, 5, "agent 配了用 agent");
        assert_eq!(eff.memory_time_secs, 7200, "role 覆盖 agent");
        assert_eq!(eff.memory_count, DEFAULT_MEMORY_COUNT, "未配回落全局");
        assert_eq!(eff.compress_prompt, DEFAULT_COMPRESS_PROMPT);
        assert!(eff.stations.is_empty());
    }

    #[test]
    fn merge_agent_all_default_and_role_overrides() {
        // agent 全缺省（ContextConfig::default()）+ role 覆盖
        let agent = AgentContextConfig {
            default_context_config: ContextConfig::default(),
            roles: Arc::new(ArcSwapHashMap::new()),
        };
        let role = ContextConfig {
            channel_batch_interval_secs: Some(7),
            memory_time_secs: None,
            memory_count: None,
            compress_prompt: None,
            stations: None,
        };
        let eff = merge_context_config(Some(&agent), Some(&role));
        assert_eq!(eff.channel_batch_interval_secs, 7, "role 覆盖生效");
        assert_eq!(eff.memory_time_secs, DEFAULT_MEMORY_TIME_SECS, "agent 全缺省回落全局");
        assert_eq!(eff.memory_count, DEFAULT_MEMORY_COUNT);
    }
```

- [ ] **Step 5: 验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test`
Expected: check 无警告；`test result: ok. 118 passed`（116 + 2 新增）；`rg "RoleContextConfig" src/` 无残留

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/config_manager.rs
git commit -m "refactor(agent): AgentContextConfig 按字段默认——RoleContextConfig 改名 ContextConfig（+Default），AgentContextConfig 改嵌套 default_context_config，merge 逐字段三层回落，新增缺省回落测试"
```

---

### Task 2: nexus.json 模板与 README 迁移嵌套格式

**Files:**
- Modify: `script/template/nexus.json`
- Modify: `test/workspace-template/agent-data/nexus.json`
- Modify: `test/workspace/agent-data/nexus.json`
- Modify: `script/README.md`

**Interfaces:**
- Consumes: Task 1 的 `AgentContextConfig { default_context_config, roles }` 格式
- Produces: 新嵌套格式的模板/文档（与 Task 1 反序列化契约一致）

- [ ] **Step 1: 迁移 3 个 nexus.json 的 context 段**

每个文件的 `context` 段内，每个 agent（`""`、`"a1"` 等）由扁平字段改为嵌套（字段名去 `default_` 前缀；缺省项可省，模板保留全量示例）：

```json
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
```

- 先查看各文件 context 段实际内容（agent key、字段值可能不同），按原值迁移
- 旧扁平字段名（`default_channel_batch_interval_secs` 等）全部移除

- [ ] **Step 2: 迁移 script/README.md 示例**

`script/README.md`（约 52-69 行）的 context 段示例同步为嵌套格式（同 Step 1 结构）。

- [ ] **Step 3: 验证**

Run: `cd /home/admin/project/kissbot && for f in script/template/nexus.json test/workspace-template/agent-data/nexus.json test/workspace/agent-data/nexus.json; do python3 -m json.tool "$f" > /dev/null && echo "$f OK"; done`
Expected: 三个文件均 JSON 合法（json.tool 无报错）
Run: `rg "default_channel_batch_interval_secs|default_memory_time_secs|default_memory_count|default_compress_prompt" script/ test/workspace-template/ --type json --type md 2>/dev/null || rg "default_channel_batch_interval_secs" script/ test/workspace-template/`
Expected: 模板与 README 中无旧扁平字段名残留（注：`default_stations` 是 AgentContextConfig 旧字段名，同样应无残留）

- [ ] **Step 4: Commit**

```bash
git add script/template/nexus.json test/workspace-template/agent-data/nexus.json test/workspace/agent-data/nexus.json script/README.md
git commit -m "refactor(agent): context 配置模板迁移嵌套格式——default_context_config 承载 agent 默认（字段去 default_ 前缀），README 示例同步"
```

---

### Task 3: 全量扫尾验证

**Files:**
- Verify only（发现问题才改）

- [ ] **Step 1: 残留检查**

Run: `cd /home/admin/project/kissbot && rg "RoleContextConfig" kissbot-agent/src script test/workspace-template 2>/dev/null; rg "default_channel_batch_interval_secs|default_memory_time_secs|default_memory_count|default_compress_prompt|default_stations" kissbot-agent/src script/template test/workspace-template test/workspace 2>/dev/null`
Expected: 源码与模板零残留（docs/superpowers 归档文档允许保留历史措辞，不检查）

- [ ] **Step 2: 全量验证**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo check && cargo test 2>&1 | tail -3`
Expected: check 无警告；`test result: ok. 118 passed`

- [ ] **Step 3: 提交（如有残留修复）**

```bash
git add -A
git commit -m "refactor(agent): context 配置按字段默认扫尾（残留清理）"
```

---

## 自审

**1. Spec 覆盖：**
- 决策 1/2（改名 + Default）→ Task 1 Step 1 ✓
- 决策 3（嵌套 default_context_config + roles 不 Option）→ Task 1 Step 1 ✓
- 决策 4（逐字段回落）→ Task 1 Step 2 ✓
- 决策 5（模板迁移）→ Task 2 ✓
- 决策 6（测试）→ Task 1 Step 3/4 ✓
- 验证节 → Task 3 ✓

**2. 占位符扫描：** 无 TBD/TODO；每步含完整代码或精确指令。

**3. 类型一致性：**
- `ContextConfig` 全 Option 字段，`ContextConfig::default()` = 全 None ✓
- `merge_context_config` 签名 `role: Option<&ContextConfig>` 与 `roles: Arc<ArcSwapHashMap<String, ContextConfig>>` 一致 ✓
- 测试构造 `default_context_config: ContextConfig { ... }`（Some 值）与结构定义一致 ✓
- 模板字段名（`channel_batch_interval_secs` 等无前缀）与 `ContextConfig` serde 字段一致 ✓
- `context_config()`（545 行）调 `merge_context_config(agent.as_deref(), role.as_deref())` 不变（role 类型换名后自动适配）✓
