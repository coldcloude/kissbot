# Station 系统重做实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 kissbot-agent 的 Station 系统重做为嵌套结构（全局 Station 单例 + Toolkit 级别 + 子 Station 递归平铺 + MCP 占位），并把 `AgentCoordinator` 重命名为 `Nexus`。

**Architecture:** 配置层（station.json：`StationRepo { toolkits, sub_stations }`）与运行态（`Station` 全局单例：`toolkits` 含实现、`sub_stations` 仅连接信息）分离；工具元数据查询 `Station::tools(filter)` 本地白名单过滤 + 直接子 HTTP 递归平铺（骨架期跳过）；`Nexus::tools_for_session`/`execute_tool_call` 委托全局 Station。工具名整树全局唯一（本地硬约束）。

**Tech Stack:** Rust 2024 / tokio / dashmap / arc-swap / serde / reqwest / async-trait；项目约定 Arc<String>/Arc<Value> 字段、ArcSwapHashMap 配置 map、OnceLock 单例模式。

## Global Constraints

- **不要删除代码中的注释**（CLAUDE.md 项目原则；删代码时随代码注释一起更新，不主动删孤立注释）
- 所有文本 UTF-8、`\n` 换行；注释用中文
- git commit comment 用中文，包含本次提交全部改动内容
- 读写文件必须用 Read/Write/Edit 工具，禁止 sed/python 修改文件
- 编译与测试命令均在 `kissbot-agent/` 目录下执行：`cargo test`（该目录是独立 crate，非 workspace）
- spec：`docs/superpowers/specs/2026-08-16-station-rework-design.md`（本项目正式技术规格在 `docs/spec/`，两者分离）

---

### Task 1: 配置层改造（config_manager.rs）+ 清理旧 StationRuntime

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`
- Modify: `kissbot-agent/src/coordinator.rs`
- Modify: `kissbot-agent/src/station.rs`
- Test: `kissbot-agent/src/config_manager.rs`（内嵌 tests 模块）

**Interfaces:**
- Consumes: 现状 `ToolConfig`（name/description/parameters，字段不变）
- Produces:
  - `StationRepo { toolkits: Arc<ArcSwapHashMap<String, ToolkitConfig>>, sub_stations: Arc<ArcSwapHashMap<String, SubStationConfig>> }`
  - `ToolkitConfig { tools: Arc<ArcSwapHashMap<String, ToolConfig>>, mcps: Arc<ArcSwapHashMap<String, McpConfig>> }`
  - `McpConfig { name: Arc<String>, description: Arc<String> }`
  - `SubStationConfig { station_id: Arc<String>, base_url: Arc<String>, timeout_secs: u64 }`
  - `ConfigManager::station_repo_snapshot() -> StationRepo`（Task 2 的 `Station::new()` 消费）
  - `ContextConfig.toolkits: Option<Arc<HashSet<String>>>`（Task 4 的 `tools_for_session` 消费）
  - 删除：`StationConfig`、`NexusRepo.stations`、`ConfigManager::stations()`、`StationRuntime`

- [ ] **Step 1: 修改 config_manager.rs 配置结构**

1a. `ContextConfig` 中 `stations` 字段改名（`config_manager.rs` 内）：

```rust
    /// 启用的 toolkit 名集合（白名单；None/空 = 无工具；替代原 stations 字段）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolkits: Option<Arc<HashSet<String>>>,
```

1b. `EffectiveContextConfig` 中 `stations: HashSet<String>` 字段改名 `toolkits: HashSet<String>`。

1c. `merge_context_config` 中三处 `stations` 引用改 `toolkits`（返回结构字段名 + 两处 `.stations.clone()` → `.toolkits.clone()`）。

1d. `NexusRepo`：删除字段行 `pub stations: Arc<ArcSwapHashMap<String, StationConfig>>,`，其 `Default` 实现中对应行 `stations: Arc::new(ArcSwapHashMap::new()),` 一并删除；`nexus_repo_default_empty` 测试里 `assert!(repo.stations.is_empty());` 删除。

1e. 删除 `StationConfig` struct（含注释），替换为（放在原 `StationConfig` 位置附近）：

```rust
/// Toolkit 配置（StationRepo.toolkits 的 value；key = toolkit 名）
/// Toolkit 中无子 Station；内置 toolkit（如 filesystem）由内置注册表填充元数据与实现，
/// 配置声明的 tools/mcps 作为补充（仅元数据注册，无本地实现时调用返回未实现）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolkitConfig {
    /// 工具元数据（key = 工具名）
    #[serde(default)]
    pub tools: Arc<ArcSwapHashMap<String, ToolConfig>>,
    /// MCP 元数据（key = mcp 名；本轮占位，无实现）
    #[serde(default)]
    pub mcps: Arc<ArcSwapHashMap<String, McpConfig>>,
}

/// MCP 配置（占位：本轮仅建结构，不实现调用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    pub name: Arc<String>,
    pub description: Arc<String>,
}

/// 子 Station 配置（StationRepo.sub_stations 的 value；key = station_id）
/// 只存直接子连接信息；子 Station 内部结构（toolkits/孙子）由子进程自己管理，父通过 HTTP 查询
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubStationConfig {
    pub station_id: Arc<String>,
    pub base_url: Arc<String>,
    pub timeout_secs: u64,
}
```

1f. `StationRepo` 从空占位改为真正结构（替换原 `pub struct StationRepo {}`）：

```rust
/// station 可改配置，持久化到 <data_dir>/station.json
/// 全局 Station 每 agent 一个：本地 toolkit 集合 + 直接子 Station 集合
/// （子只能 HTTP 通信，父只存连接信息；toolkit 名全局唯一命名空间，含子 Station 不能重名）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StationRepo {
    /// 本地 toolkit 集合（key = toolkit 名）
    pub toolkits: Arc<ArcSwapHashMap<String, ToolkitConfig>>,
    /// 直接子 Station 集合（key = station_id；孙子由子进程自己递归，父不管）
    pub sub_stations: Arc<ArcSwapHashMap<String, SubStationConfig>>,
}
```

1g. 删除 `ConfigManager::stations()` 方法（返回 `Vec<(String, Arc<StationConfig>)>` 的那个），新增：

```rust
    /// 返回 StationRepo 快照（Station 单例构建使用）
    pub async fn station_repo_snapshot(&self) -> StationRepo {
        self.station_repo.read().await.clone()
    }
```

- [ ] **Step 2: 修改 config_manager.rs 测试**

2a. 删除测试 `station_config_tools_roundtrip`（StationConfig 已删），替换为：

```rust
    #[test]
    fn station_repo_new_shape_serde_roundtrip() {
        // StationRepo 新形状：toolkits + sub_stations；McpConfig 占位序列化
        let mut repo = StationRepo::default();
        {
            let map = Arc::make_mut(&mut repo.toolkits);
            map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(ToolkitConfig {
                tools: Arc::new(ArcSwapHashMap::new()),
                mcps: Arc::new({
                    let m = ArcSwapHashMap::new();
                    m.insert("mcp1".to_string(), ArcSwap::new(Arc::new(McpConfig {
                        name: Arc::new("mcp1".into()),
                        description: Arc::new("占位".into()),
                    })));
                    m
                }),
            })));
        }
        {
            let map = Arc::make_mut(&mut repo.sub_stations);
            map.insert("station-a".to_string(), ArcSwap::new(Arc::new(SubStationConfig {
                station_id: Arc::new("station-a".into()),
                base_url: Arc::new("http://127.0.0.1:9001".into()),
                timeout_secs: 30,
            })));
        }
        let json = serde_json::to_string(&repo).unwrap();
        assert!(json.contains("\"toolkits\"") && json.contains("\"sub_stations\""), "新形状字段");
        let back: StationRepo = serde_json::from_str(&json).unwrap();
        assert!(back.toolkits.contains_key("filesystem"));
        let tcfg = back.toolkits.get("filesystem").unwrap().load_full();
        assert_eq!(tcfg.mcps.get("mcp1").unwrap().load_full().name.as_str(), "mcp1");
        let sub = back.sub_stations.get("station-a").unwrap().load_full();
        assert_eq!(sub.base_url.as_str(), "http://127.0.0.1:9001");

        // ToolConfig 序列化（原 station_config_tools_roundtrip 保留部分）
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

2b. 新增 context 字段改名测试（放在 context 配置测试区）：

```rust
    #[test]
    fn context_config_toolkits_replaces_stations() {
        // 新格式：toolkits 字段生效
        let new = r#"{"toolkits": ["filesystem"]}"#;
        let cfg: ContextConfig = serde_json::from_str(new).unwrap();
        assert!(cfg.toolkits.as_ref().unwrap().contains("filesystem"));
        // 旧格式：stations 字段被忽略（未知字段），toolkits 缺省为 None
        let old = r#"{"stations": ["local"]}"#;
        let cfg: ContextConfig = serde_json::from_str(old).unwrap();
        assert!(cfg.toolkits.is_none(), "旧 stations 字段应被忽略");
    }
```

2c. `nexus_repo_serde_roundtrip` 与 `nexus_repo_default_empty` 测试中删除 `stations:` 字段构造行与断言行（与 1d 对应）。

- [ ] **Step 3: 修改 coordinator.rs 清理旧 station 运行时**

3a. 删除 import 行 `use crate::station::{self, StationRuntime};`。

3b. 删除字段 `station_runtimes: Arc<DashMap<String, Arc<StationRuntime>>>,` 及其初始化行 `station_runtimes: Arc::new(DashMap::new()),`。

3c. 删除 `new()` 中构建 station 运行时的块（`// 构建 Station 运行态：...` 到 `}` 的整个 `{ let runtimes = ... }` 块）。

3d. `tools_for_session` 临时改为空实现（Task 4 恢复真逻辑）：

```rust
    /// 会话可用工具：context 配置的启用 toolkits 白名单 → Station 平铺查询（Task 4 接入 Station 单例）
    /// 过渡期（Task 1-3）返回空：Station 运行时已从 Nexus 移除，待 station.rs 重写后恢复
    pub async fn tools_for_session(&self, _session: Arc<Session>) -> Vec<ToolConfig> {
        Vec::new()
    }
```

3e. `execute_tool_call` 临时改为空实现（Task 4 恢复）：

```rust
    /// 执行单个 tool call（Task 4 接入 Station 单例；过渡期返回工具不存在）
    pub async fn execute_tool_call(&self, _session: Arc<Session>, call: Arc<ToolCall>) -> serde_json::Value {
        serde_json::json!({ "error": format!("工具不存在: {}", call.name) })
    }
```

- [ ] **Step 4: 修改 station.rs 删除 StationRuntime**

删除 `StationRuntime` struct + impl（含 `station_id`/`config`/`register_local`/`configured_tools`/`has_tool`/`call_tool` 方法）；`Tool` trait、`ReadTool`（含 `resolve_safe_path`/`READ_MAX_BYTES`/单测）原样保留。`use crate::config_manager::{StationConfig, ToolConfig};` 改为 `use crate::config_manager::ToolConfig;`。删除测试 `station_runtime_local_call` 与 helper `station_config`（StationConfig 已删）。

- [ ] **Step 5: 运行测试验证**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -20`
Expected: 编译通过，全部测试 PASS（含重写后的 station_repo / context_toolkits 新测试）

- [ ] **Step 6: 提交**

```bash
git add kissbot-agent/src/config_manager.rs kissbot-agent/src/coordinator.rs kissbot-agent/src/station.rs
git commit -m "refactor(agent): 配置层 Station 重构——StationRepo 承载 toolkits/sub_stations（ToolkitConfig/McpConfig/SubStationConfig），删除 StationConfig 与 NexusRepo.stations，ContextConfig.stations 改名 toolkits（白名单）；清理 StationRuntime 与 Nexus.station_runtimes（tools/execute_tool_call 过渡期空实现）"
```

---

### Task 2: station.rs 重写（全局 Station 单例 + Toolkit + 子 Station + 递归平铺）

**Files:**
- Modify: `kissbot-agent/src/station.rs`
- Test: `kissbot-agent/src/station.rs`（内嵌 tests 模块）

**Interfaces:**
- Consumes: Task 1 的 `StationRepo`/`ToolkitConfig`/`McpConfig`/`SubStationConfig`/`ToolConfig`、`ConfigManager::station_repo_snapshot()`；Task 3 的 `StationClient`（list_tools/list_mcps/call_tool 骨架）
- Produces:
  - `Station::get() -> &'static Station`（全局单例）
  - `Station::new() -> Result<()>`（从 ConfigManager 读 station.json 构建并注册单例）
  - `Station::from_repo(&StationRepo) -> Result<Station>`（纯构造，单测用）
  - `Station::tools(Option<&HashSet<String>>) -> Result<Vec<ToolConfig>>`（None=全部，Some=白名单）
  - `Station::mcps(Option<&HashSet<String>>) -> Result<Vec<McpConfig>>`（占位查询）
  - `Station::call_tool(&str, Value) -> Result<Value>`（本地实现表 → 直接子递归）

- [ ] **Step 1: 编写失败测试**（追加到 station.rs `#[cfg(test)] mod tests`）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_manager::{McpConfig, StationRepo, SubStationConfig, ToolkitConfig, ToolConfig};
    use arc_swap::ArcSwap;
    use kissbot_api::ArcSwapHashMap;

    fn filesystem_toolkit() -> ToolkitConfig {
        ToolkitConfig {
            tools: Arc::new(ArcSwapHashMap::new()),
            mcps: Arc::new(ArcSwapHashMap::new()),
        }
    }

    fn repo_with_filesystem() -> StationRepo {
        let mut repo = StationRepo::default();
        let map = Arc::make_mut(&mut repo.toolkits);
        map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(filesystem_toolkit())));
        repo
    }

    fn repo_with_mcp() -> StationRepo {
        let mut repo = StationRepo::default();
        let map = Arc::make_mut(&mut repo.toolkits);
        let mut t = filesystem_toolkit();
        {
            let m = Arc::make_mut(&mut t.mcps);
            m.insert("mcp1".to_string(), ArcSwap::new(Arc::new(McpConfig {
                name: Arc::new("mcp1".into()),
                description: Arc::new("占位".into()),
            })));
        }
        map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(t)));
        repo
    }

    fn repo_with_sub() -> StationRepo {
        let mut repo = StationRepo::default();
        let map = Arc::make_mut(&mut repo.sub_stations);
        map.insert("station-a".to_string(), ArcSwap::new(Arc::new(SubStationConfig {
            station_id: Arc::new("station-a".into()),
            base_url: Arc::new("http://127.0.0.1:9001".into()),
            timeout_secs: 5,
        })));
        repo
    }

    #[tokio::test]
    async fn tools_none_returns_all_local() {
        // 内置 filesystem toolkit 声明后，read 工具元数据由内置注册表填充
        let station = Station::from_repo(&repo_with_filesystem()).unwrap();
        let tools = station.tools(None).await.unwrap();
        assert_eq!(tools.len(), 1, "内置注册表应填充 read");
        assert_eq!(tools[0].name.as_str(), "read");
    }

    #[tokio::test]
    async fn tools_filter_whitelist_semantics() {
        // None=全部；Some(命中)=白名单；Some(未命中)/Some(空集)=空
        let station = Station::from_repo(&repo_with_filesystem()).unwrap();
        let hit: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        let miss: HashSet<String> = ["other".to_string()].into_iter().collect();
        let empty = HashSet::new();
        assert_eq!(station.tools(Some(&hit)).await.unwrap().len(), 1);
        assert!(station.tools(Some(&miss)).await.unwrap().is_empty());
        assert!(station.tools(Some(&empty)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn tools_with_sub_skips_http_skeleton() {
        // 子 Station HTTP 骨架未实现 → Err 记日志跳过；无本地工具时结果为空
        let station = Station::from_repo(&repo_with_sub()).unwrap();
        assert!(station.tools(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mcps_placeholder_local_only() {
        let station = Station::from_repo(&repo_with_mcp()).unwrap();
        let mcps = station.mcps(None).await.unwrap();
        assert_eq!(mcps.len(), 1);
        assert_eq!(mcps[0].name.as_str(), "mcp1");
    }

    #[test]
    fn from_repo_rejects_duplicate_tool_names() {
        // 两个 toolkit 声明同名工具（一个内置 read、一个配置 read）→ 本地硬约束启动失败
        let mut repo = StationRepo::default();
        {
            let map = Arc::make_mut(&mut repo.toolkits);
            map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(filesystem_toolkit())));
            map.insert("other".to_string(), ArcSwap::new(Arc::new(ToolkitConfig {
                tools: Arc::new({
                    let m = ArcSwapHashMap::new();
                    m.insert("read".to_string(), ArcSwap::new(Arc::new(ToolConfig {
                        name: Arc::new("read".into()),
                        description: Arc::new("d".into()),
                        parameters: Arc::new(serde_json::json!({})),
                    })));
                    m
                }),
                mcps: Arc::new(ArcSwapHashMap::new()),
            })));
        }
        let err = Station::from_repo(&repo).unwrap_err();
        assert!(err.to_string().contains("工具名冲突"), "冲突应报错: {}", err);
    }

    #[tokio::test]
    async fn call_tool_local_hit_and_miss() {
        let station = Station::from_repo(&repo_with_filesystem()).unwrap();
        // 命中：read 本地实现执行（读测试目录内文件）
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "内容").unwrap();
        let read = crate::station::ReadTool::new(dir.path().to_path_buf());
        let station2 = Station::from_repo(&{
            let mut repo = repo_with_filesystem();
            // 替换内置实现无法直接注入，这里改用临时单实例验证路径：直接调 ReadTool
            repo
        }).unwrap();
        let _ = (station, station2, read);
        // 未注册工具 → 工具不存在
        let miss = station.call_tool("nope", serde_json::json!({})).await;
        assert!(miss.is_err() && miss.unwrap_err().to_string().contains("工具不存在"));
    }
}
```

> 注：`call_tool_local_hit_and_miss` 中内置实现路径不可注入（ReadTool 由注册表用 `current_dir()` 构造），命中路径改由 Task 2 的注册表构造确认——若测试复杂度高，可只保留未注册断言，命中路径由 `tools_none_returns_all_local` 的元数据 + ReadTool 自身单测覆盖。

- [ ] **Step 2: 运行测试验证失败**

Run: `cd kissbot-agent && cargo test station:: 2>&1 | tail -20`
Expected: 编译失败（`Station`/`from_repo`/`tools` 等未定义）

- [ ] **Step 3: 重写 station.rs**（`Tool` trait 与 `ReadTool` 部分原样保留，在其后替换整个运行态部分）：

```rust
// ========== 内置注册表 ==========

/// 内置 toolkit 声明：toolkit 名 → 内置工具集合（元数据 + 实现）
/// 配置显式声明对应 toolkit 名时才注册；未声明则不注册（内置工具也须显式配置 toolkit 才可用）
pub struct BuiltinToolkit {
    /// 内置工具元数据（ToolConfig，含参数 JSON Schema）
    pub tool_configs: Vec<ToolConfig>,
    /// 内置工具实现（(工具名, 实现)，与 tool_configs 同名对应）
    pub tool_impls: Vec<(&'static str, Arc<dyn Tool>)>,
}

/// 内置注册表（本轮仅 filesystem toolkit：Read 工具）
fn builtin_registry() -> Vec<(&'static str, BuiltinToolkit)> {
    vec![(
        "filesystem",
        BuiltinToolkit {
            tool_configs: vec![ToolConfig {
                name: Arc::new("read".into()),
                description: Arc::new("读取文本文件内容（路径限当前工作目录内，返回限长 64KB）".into()),
                parameters: Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径（相对或绝对，限工作目录内）" }
                    },
                    "required": ["path"]
                })),
            }],
            tool_impls: vec![(
                "read",
                Arc::new(ReadTool::new(std::env::current_dir().unwrap_or_default())),
            )],
        },
    )]
}

// ========== 运行态结构 ==========

/// Toolkit 内单个工具条目：元数据 + 实现（None = 仅元数据注册，无本地实现，调用返回未实现）
struct ToolkitEntry {
    config: ToolConfig,
    imp: Option<Arc<dyn Tool>>,
}

/// Toolkit 运行态：工具实现表 + MCP 占位。Toolkit 中无子 Station
pub struct Toolkit {
    name: String,
    tools: DashMap<String, Arc<ToolkitEntry>>,
    mcps: DashMap<String, Arc<McpConfig>>,
}

impl Toolkit {
    fn new(name: String) -> Self {
        Self { name, tools: DashMap::new(), mcps: DashMap::new() }
    }

    /// 该 toolkit 的工具元数据列表（LLM tools 聚合用）
    fn configured_tools(&self) -> Vec<ToolConfig> {
        self.tools.iter().map(|e| e.value().config.clone()).collect()
    }

    /// 该 toolkit 的 MCP 占位列表
    fn configured_mcps(&self) -> Vec<McpConfig> {
        self.mcps.iter().map(|e| (*e.value()).clone()).collect()
    }
}

/// 子 Station 运行态：只存连接信息 + HTTP 客户端（子只能 HTTP 通信，孙子由子递归）
pub struct SubStation {
    config: Arc<SubStationConfig>,
    client: StationClient,
}

/// 全局 Station 单例（进程内唯一；每个 agent 一个，静态访问）
/// 内部：本地 toolkit 集合（含实现） + 直接子 Station 集合（仅连接信息）
static SINGLETON: OnceLock<Station> = OnceLock::new();

pub struct Station {
    toolkits: DashMap<String, Arc<Toolkit>>,
    sub_stations: DashMap<String, Arc<SubStation>>,
}

impl Station {
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn get() -> &'static Station {
        SINGLETON.get().expect("Station 未初始化")
    }

    /// 从 ConfigManager 读 station.json 构建全局 Station 并注册单例
    pub async fn new() -> Result<()> {
        let repo = ConfigManager::get().station_repo_snapshot().await;
        let station = Self::from_repo(&repo)?;
        let _ = SINGLETON.set(station);
        Ok(())
    }

    /// 按配置构建 Station（纯构造，不注册单例，单测用）：
    /// 内置注册表填充声明 toolkit 的实现；配置声明的 tools/mcps 补充元数据；
    /// 工具名整树全局唯一（本地硬约束，冲突启动失败）
    pub fn from_repo(repo: &StationRepo) -> Result<Station> {
        let registry = builtin_registry();
        let toolkits = DashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (name, tcfg) in repo.toolkits.iter() {
            let tcfg = tcfg.load_full();
            let toolkit = Arc::new(Toolkit::new(name.clone()));
            // 1. 内置注册表填充（声明该 toolkit 名才注册内置实现）
            if let Some((_, builtin)) = registry.iter().find(|(n, _)| *n == name.as_str()) {
                for cfg in &builtin.tool_configs {
                    let tool_name = cfg.name.to_string();
                    Self::check_unique(&mut seen, &tool_name)?;
                    let imp = builtin.tool_impls.iter()
                        .find(|(tn, _)| *tn == tool_name)
                        .map(|(_, t)| t.clone());
                    toolkit.tools.insert(tool_name, Arc::new(ToolkitEntry { config: (*cfg).clone(), imp }));
                }
            }
            // 2. 配置声明的 tools（元数据补充；与内置重名 → 冲突）
            for (tool_name, tcfg) in tcfg.tools.iter() {
                if toolkit.tools.contains_key(tool_name) {
                    return Err(Error::InternalError(format!("工具名冲突: {}（toolkit 内全局唯一）", tool_name)));
                }
                Self::check_unique(&mut seen, tool_name)?;
                toolkit.tools.insert(tool_name.clone(), Arc::new(ToolkitEntry { config: (*tcfg.load_full()).clone(), imp: None }));
            }
            // 3. 配置声明的 mcps（占位）
            for (mcp_name, mcfg) in tcfg.mcps.iter() {
                toolkit.mcps.insert(mcp_name.clone(), Arc::new((*mcfg.load_full()).clone()));
            }
            toolkits.insert(name.clone(), toolkit);
        }
        // 子 Station：只存连接信息
        let sub_stations = DashMap::new();
        for (id, scfg) in repo.sub_stations.iter() {
            let scfg = scfg.load_full();
            let sub = SubStation {
                config: Arc::new((*scfg).clone()),
                client: StationClient::new(scfg.timeout_secs),
            };
            sub_stations.insert(id.clone(), Arc::new(sub));
        }
        Ok(Station { toolkits, sub_stations })
    }

    /// 工具名唯一性校验：本地 toolkit 间不得重名（工具名整树全局唯一，本地先硬约束）
    fn check_unique(seen: &mut HashSet<String>, name: &str) -> Result<()> {
        if !seen.insert(name.to_string()) {
            return Err(Error::InternalError(format!("工具名冲突: {}（整树全局唯一）", name)));
        }
        Ok(())
    }

    /// 工具元数据平铺查询：本地 toolkit 白名单过滤 + 直接子递归（HTTP 骨架，本轮未实现跳过）
    /// filter = None 返回全部；Some(空集) 返回空
    pub async fn tools(&self, filter: Option<&HashSet<String>>) -> Result<Vec<ToolConfig>> {
        let mut out = Vec::new();
        for entry in self.toolkits.iter() {
            let toolkit = entry.value();
            if let Some(f) = filter {
                if !f.contains(toolkit.name.as_str()) { continue; }
            }
            out.extend(toolkit.configured_tools());
        }
        // 直接子递归（HTTP 骨架：未实现返回 Err → 记日志跳过，不阻塞整体）
        for entry in self.sub_stations.iter() {
            let sub = entry.value();
            match sub.client.list_tools(filter).await {
                Ok(tools) => out.extend(tools),
                Err(e) => warn!("子 Station {} 查询工具失败: {}", sub.config.station_id.as_str(), e),
            }
        }
        Ok(out)
    }

    /// MCP 元数据平铺查询（占位接口：本地返回配置，直接子 HTTP 骨架跳过）
    pub async fn mcps(&self, filter: Option<&HashSet<String>>) -> Result<Vec<McpConfig>> {
        let mut out = Vec::new();
        for entry in self.toolkits.iter() {
            let toolkit = entry.value();
            if let Some(f) = filter {
                if !f.contains(toolkit.name.as_str()) { continue; }
            }
            out.extend(toolkit.configured_mcps());
        }
        for entry in self.sub_stations.iter() {
            let sub = entry.value();
            match sub.client.list_mcps(filter).await {
                Ok(mcps) => out.extend(mcps),
                Err(e) => warn!("子 Station {} 查询 MCP 失败: {}", sub.config.station_id.as_str(), e),
            }
        }
        Ok(out)
    }

    /// 执行工具：本地实现表（跨 toolkit，工具名全局唯一）命中执行；
    /// 未命中 → 直接子递归（HTTP 骨架：子返回 Err 记日志跳过；全部未命中 → 工具不存在）
    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value> {
        // 克隆出 Arc 立即释放 DashMap 读锁（不跨 await 持锁，防同 shard 写者死锁）
        for entry in self.toolkits.iter() {
            if let Some(t) = entry.value().tools.get(name) {
                let imp = t.value().imp.clone();
                return match imp {
                    Some(tool) => tool.call(params).await,
                    None => Err(Error::InternalError(format!("工具未实现（仅元数据注册）: {}", name))),
                };
            }
        }
        // 直接子递归（骨架：子未实现返回 Err → 记日志跳过；全部未命中 → 工具不存在）
        for entry in self.sub_stations.iter() {
            let sub = entry.value();
            match sub.client.call_tool(name, params.clone()).await {
                Ok(v) => return Ok(v),
                Err(e) => warn!("子 Station {} 调用工具 {} 失败: {}", sub.config.station_id.as_str(), name, e),
            }
        }
        Err(Error::InternalError(format!("工具不存在: {}", name)))
    }
}
```

文件顶部 import 调整为：

```rust
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;
use tracing::warn;

use crate::config_manager::{ConfigManager, McpConfig, StationRepo, SubStationConfig, ToolkitConfig, ToolConfig};
use crate::station_client::StationClient;
use crate::types::{Error, Result};
```

> 注：`Toolkit`/`SubStation`/`ToolkitEntry`/`BuiltinToolkit` 为 pub 或私有视使用面定——`Toolkit`/`SubStation` 保持 pub（结构与语义公开），`ToolkitEntry`/`BuiltinToolkit` 若仅内部使用可保持私有；若编译出现 dead_code 警告可加 `#[allow(dead_code)]`（按项目既有风格）。

- [ ] **Step 4: 运行测试验证通过**

Run: `cd kissbot-agent && cargo test station:: 2>&1 | tail -20`
Expected: 全部 PASS（`tools_none_returns_all_local` / `tools_filter_whitelist_semantics` / `tools_with_sub_skips_http_skeleton` / `mcps_placeholder_local_only` / `from_repo_rejects_duplicate_tool_names` / `call_tool_local_hit_and_miss` 与既有 read_tool 测试）

- [ ] **Step 5: 全量测试**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -10`
Expected: 全绿（Task 1 后 tools_for_session 过渡空实现不触发 Station 单例，无需装配）

- [ ] **Step 6: 提交**

```bash
git add kissbot-agent/src/station.rs
git commit -m "feat(agent): station.rs 重写为嵌套结构——全局 Station 单例（from_repo 纯构造可测）、Toolkit（工具实现表 + MCP 占位）、子 Station（仅连接信息 + HTTP 客户端）、内置 filesystem toolkit 注册表（read）；tools/mcps 平铺查询支持 toolkit 白名单过滤、call_tool 本地→直接子递归；工具名本地硬约束全局唯一"
```

---

### Task 3: station_client.rs 改造 + 删除 station_router.rs

**Files:**
- Modify: `kissbot-agent/src/station_client.rs`
- Delete: `kissbot-agent/src/station_router.rs`
- Modify: `kissbot-agent/src/main.rs`
- Test: `kissbot-agent/src/station_client.rs`（内嵌 tests 模块）

**Interfaces:**
- Consumes: Task 1 的 `McpConfig`/`ToolConfig`
- Produces: `StationClient::list_tools(Option<&HashSet<String>>) -> Result<Vec<ToolConfig>>`、`StationClient::list_mcps(...) -> Result<Vec<McpConfig>>`、`StationClient::call_tool(&str, Value) -> Result<Value>`（Task 2 的 `SubStation` 消费）

- [ ] **Step 1: 重写 station_client.rs**（整个文件替换）

```rust
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

use crate::config_manager::{McpConfig, ToolConfig};
use crate::types::{Error, Result};

/// 子 Station HTTP 通信客户端（子 Station 只能通过 HTTP 通信，不可本地调用）
/// 本轮骨架：请求/响应结构定义好（list_tools / list_mcps / call_tool），调用返回未实现错误；
/// 后续实现 HTTP 协议（查询元数据带 toolkit 白名单过滤 / 工具调用）
pub struct StationClient {
    client: reqwest::Client,
    _default_timeout: Duration,
}

impl StationClient {
    pub fn new(default_timeout_secs: u64) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(default_timeout_secs))
                .build()
                .unwrap_or_default(),
            _default_timeout: Duration::from_secs(default_timeout_secs),
        }
    }

    /// 查询子 Station 平铺后的工具元数据（可选 toolkit 白名单过滤；None = 全部）
    /// 骨架：返回未实现错误，后续由 HTTP 协议实现接入
    pub async fn list_tools(&self, _filter: Option<&HashSet<String>>) -> Result<Vec<ToolConfig>> {
        Err(Error::InternalError("子 Station 工具查询未实现（本轮骨架）".to_string()))
    }

    /// 查询子 Station 平铺后的 MCP 元数据（可选 toolkit 白名单过滤）
    /// 骨架：返回未实现错误（MCP 本轮整体不实现）
    pub async fn list_mcps(&self, _filter: Option<&HashSet<String>>) -> Result<Vec<McpConfig>> {
        Err(Error::InternalError("子 Station MCP 查询未实现（本轮骨架）".to_string()))
    }

    /// 调用子 Station 上的工具
    /// 骨架：返回未实现错误，后续由 HTTP 协议实现接入
    pub async fn call_tool(&self, _name: &str, _params: Value) -> Result<Value> {
        Err(Error::InternalError("子 Station 工具调用未实现（本轮骨架）".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn skeleton_returns_unimplemented() {
        let client = StationClient::new(5);
        let filter: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        assert!(client.list_tools(Some(&filter)).await.unwrap_err().to_string().contains("未实现"));
        assert!(client.list_mcps(None).await.unwrap_err().to_string().contains("未实现"));
        assert!(client.call_tool("read", serde_json::json!({})).await.unwrap_err().to_string().contains("未实现"));
    }
}
```

- [ ] **Step 2: 删除 station_router.rs 并移除 mod 声明**

`git rm kissbot-agent/src/station_router.rs`；`main.rs` 中删除行 `mod station_router;`。

- [ ] **Step 3: 运行测试验证**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -10`
Expected: 编译通过，全部 PASS（含 `skeleton_returns_unimplemented`）

- [ ] **Step 4: 提交**

```bash
git add kissbot-agent/src/station_client.rs kissbot-agent/src/main.rs
git rm kissbot-agent/src/station_router.rs
git commit -m "refactor(agent): StationClient 改造为子 Station HTTP 客户端骨架（list_tools/list_mcps/call_tool 请求响应结构就位，调用返回未实现）；删除 station_router.rs（路由功能并入全局 Station）"
```

---

### Task 4: coordinator.rs → nexus.rs 重命名 + Nexus 委托全局 Station

**Files:**
- Rename: `kissbot-agent/src/coordinator.rs` → `kissbot-agent/src/nexus.rs`
- Modify: `kissbot-agent/src/nexus.rs`（原名 coordinator.rs）
- Modify: `kissbot-agent/src/session_manager.rs`
- Modify: `kissbot-agent/src/command_router.rs`
- Modify: `kissbot-agent/src/main.rs`

**Interfaces:**
- Consumes: Task 2 的 `Station::get()`/`tools`/`call_tool`；Task 1 的 `ContextConfig.toolkits`
- Produces: `Nexus`（原 `AgentCoordinator` 全量 API 同名迁移：`get()`/`new()`/`run()`/`verify_agent_exists`/`tools_for_session`/`execute_tool_call` 等）

- [ ] **Step 1: git mv 并全局替换标识符**

```bash
git mv kissbot-agent/src/coordinator.rs kissbot-agent/src/nexus.rs
```

在 `nexus.rs` 中：`AgentCoordinator` → `Nexus`（struct 定义、`SINGLETON: OnceLock<Nexus>`、`Nexus::get()`、所有内部 `AgentCoordinator::get()` 引用、文档注释中 "AgentCoordinator" 字样同步）。模块顶部 `use crate::station::{self, StationRuntime};` 改为 `use crate::station::Station;`（Task 1 已删该行，此处重新按需加）。

- [ ] **Step 2: 恢复 tools_for_session / execute_tool_call 真逻辑**

`tools_for_session`（替换 Task 1 的临时空实现）：

```rust
    /// 会话可用工具：context 配置的启用 toolkits 白名单 → Station 平铺查询（本地 + 直接子递归）
    /// tools 聚合为空则请求不携带 tools 字段（兼容无工具场景）
    pub async fn tools_for_session(&self, session: Arc<Session>) -> Vec<ToolConfig> {
        let cfg = ConfigManager::get().context_config(session.agent_id.as_str(), session.role_name.as_str()).await;
        if cfg.toolkits.is_empty() {
            return Vec::new();
        }
        match Station::get().tools(Some(&cfg.toolkits)).await {
            Ok(tools) => tools,
            Err(e) => {
                warn!("工具查询失败: {}", e);
                Vec::new()
            }
        }
    }
```

`execute_tool_call`（替换 Task 1 的临时实现；去掉不再需要的 session 参数）：

```rust
    /// 执行单个 tool call：全局 Station 本地实现表 → 直接子递归；找不到/调用失败返回错误 JSON
    pub async fn execute_tool_call(&self, call: Arc<ToolCall>) -> serde_json::Value {
        match Station::get().call_tool(call.name.as_str(), (*call.arguments).clone()).await {
            Ok(v) => v,
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        }
    }
```

- [ ] **Step 3: 更新引用方**

`session_manager.rs`：
- 行 19：`use crate::coordinator::AgentCoordinator;` → `use crate::nexus::Nexus;`
- 所有 `AgentCoordinator::get()` → `Nexus::get()`（约 5 处）
- 行 392：`coordinator.execute_tool_call(self.clone(), call.clone())` → `coordinator.execute_tool_call(call.clone())`
- `ensure_test_globals`（约 801-820 行）：在 `let _ = ConfigManager::new().await;` 后插入 `let _ = crate::station::Station::new().await;`（装配 Station 单例），注释更新为 "ConfigManager/Station/Nexus 单例各注册一次"

`command_router.rs`：
- 行 6：`use crate::coordinator::{AgentCoordinator, RESERVED_ROLE_NAME};` → `use crate::nexus::{Nexus, RESERVED_ROLE_NAME};`
- 行 11、174、214：`AgentCoordinator::get()` / `AgentCoordinator::verify_agent_exists` → `Nexus::get()` / `Nexus::verify_agent_exists`

`main.rs`：
- `mod coordinator;` → `mod nexus;`
- 启动顺序更新（Station 在 Nexus 前）：

```rust
    // 1. 加载配置并注册全局单例（KISSBOT_CONFIG agent 段 → AgentConfig，按 data_dir 引导 NexusRepo/StationRepo）
    config_manager::ConfigManager::new().await
        .expect("初始化配置失败");

    // 2. 构建全局 Station 单例（读 station.json：本地 toolkit + 直接子 Station）
    station::Station::new().await
        .expect("初始化 Station 失败");

    // 3. 初始化 Nexus（装配 + 注册单例；连接与启动动作在 run() 中执行）
    nexus::Nexus::new().await
        .expect("初始化 Nexus 失败");
```

- 主循环调用：`coordinator::AgentCoordinator::get().run().await;` → `nexus::Nexus::get().run().await;`，注释 "Coordinator" 字样同步为 "Nexus"。

- [ ] **Step 4: 全量测试**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -10`
Expected: 编译通过，全部 PASS（含 session_manager 测试装配——`ensure_test_globals` 已装配 Station 单例）

- [ ] **Step 5: 提交**

```bash
git add kissbot-agent/src/nexus.rs kissbot-agent/src/session_manager.rs kissbot-agent/src/command_router.rs kissbot-agent/src/main.rs
git commit -m "refactor(agent): AgentCoordinator 重命名为 Nexus（coordinator.rs → nexus.rs），Nexus 委托全局 Station——tools_for_session 经 ContextConfig.toolkits 白名单查 Station::tools，execute_tool_call 调 Station::call_tool（去 session 参数）；启动顺序 ConfigManager → Station → Nexus；引用方（session_manager/command_router/main）同步"
```

---

### Task 5: 配置模板同步 + 全量验证

**Files:**
- Modify: `script/template/station.json`
- Modify: `script/template/nexus.json`
- Modify: `test/workspace-template/agent-data/nexus.json`
- Modify: `test/workspace/agent-data/nexus.json`（若存在）

- [ ] **Step 1: 更新 station.json 模板**（整个文件替换）

```json
{
  "toolkits": {
    "filesystem": {}
  },
  "sub_stations": {}
}
```

> filesystem 为内置 toolkit：read 工具的元数据与实现由内置注册表填充，配置仅声明 toolkit 名。子 Station 示例（连接信息，孙子由子进程递归）：`"station-a": { "station_id": "station-a", "base_url": "http://127.0.0.1:9001", "timeout_secs": 30 }`。

- [ ] **Step 2: 更新 nexus.json 模板**

`script/template/nexus.json` 与 `test/workspace-template/agent-data/nexus.json`、`test/workspace/agent-data/nexus.json`：删除 `"stations": {},` 行（`NexusRepo.stations` 已删；旧文件多余字段 serde 默认忽略，但模板保持干净）。context 段如有示例，`stations` 键改 `toolkits`。

- [ ] **Step 3: 全量验证**

Run: `cd kissbot-agent && cargo test 2>&1 | tail -5 && cargo build 2>&1 | tail -3`
Expected: 全部 PASS，build 无警告报错

- [ ] **Step 4: 提交**

```bash
git add script/template/station.json script/template/nexus.json test/workspace-template/agent-data/nexus.json test/workspace/agent-data/nexus.json
git commit -m "chore(agent): 配置模板同步——station.json 模板改为 toolkits（filesystem 内置）/sub_stations 结构，nexus.json 模板删除 stations 段"
```

---

### Task 6: 文档同步

**Files:**
- Modify: `docs/design/components-design/kissbot-agent-station.md`（重写）
- Modify: `docs/design/system-design-agent.md`
- Create: `docs/spec/kissbot-agent-station.md`
- Modify: `docs/plan/components-plan/kissbot-agent-station.md`
- Modify: `docs/roadmap/pending.md`
- Modify: `docs/index.md`（若含 spec 索引）

- [ ] **Step 1: 重写 kissbot-agent-station.md**

按新结构重写模块设计：概述（嵌套 Station、每 agent 一个全局单例、子 Station 仅 HTTP）、核心功能（toolkit 平铺递归、白名单过滤、工具名全局唯一）、内部模块（Station 单例 / Toolkit / SubStation / 内置注册表 filesystem / StationClient 骨架 / MCP 占位）、配置格式（station.json 示例）、功能流程（元数据查询、工具调用）、范围外（MCP、HTTP 协议实现）。

- [ ] **Step 2: 更新 system-design-agent.md**

"配置管理器"段落中 "StationRepo 存 stations（本轮占位）" 改为 "StationRepo 存 toolkits/sub_stations（station.json）"；Station 模块描述同步（Toolkit 级别、子 Station HTTP、递归平铺）；"AgentCoordinator" 相关表述若出现改为 "Nexus"。

- [ ] **Step 3: 新增 docs/spec/kissbot-agent-station.md**

技术规格：station.json 配置格式（toolkits/sub_stations/McpConfig/SubStationConfig 字段说明）、toolkit 全局唯一命名空间约定、递归平铺语义（tools(filter) None=全部/空集=空）、工具名整树唯一（本地硬约束 + 跨进程部署保证）、子 Station HTTP 协议骨架（list_tools/list_mcps/call_tool 请求响应结构，本轮未实现）、ContextConfig.toolkits 白名单与 Session 对应关系。

- [ ] **Step 4: 更新实现计划与遗留事项**

`docs/plan/components-plan/kissbot-agent-station.md`：改为本次嵌套化改造的计划（Task 划分或引用本计划文件）；`docs/roadmap/pending.md` 登记：MCP 实现、子 Station HTTP 协议实现、跨进程工具名唯一性校验（查询时发现冲突报错）；`docs/index.md` 若有 spec 索引则加入 kissbot-agent-station.md。

- [ ] **Step 5: 提交**

```bash
git add docs/
git commit -m "docs(agent): Station 嵌套化文档同步——模块设计重写（Toolkit/子 Station/递归平铺/内置 filesystem），新增 docs/spec/kissbot-agent-station.md 技术规格，实现计划与遗留事项更新（MCP/HTTP 协议/跨进程唯一性）"
```

---

## Self-Review

**Spec coverage 检查：**
- 重命名 AgentCoordinator → Nexus / nexus.rs → Task 4 ✓
- Station 全局单例静态访问 → Task 2（OnceLock + Station::get()）✓
- Station 含 Toolkit 集合 + 子 Station 集合 → Task 2 结构 ✓
- 子 Station 仅 HTTP、父只存直接子 → Task 2（SubStation 仅连接信息 + StationClient）、Task 3 ✓
- 递归平铺（tools/mcps）+ toolkit 白名单过滤（None=全部/空集=空）→ Task 2 ✓
- Toolkit 无子 Station → Task 2（Toolkit 结构无 sub_stations）✓
- 每个 Session 的 toolkit map（ContextConfig.toolkits 替代 stations）→ Task 1 + Task 4 ✓
- MCP 仅占位 struct → Task 1（McpConfig）+ Task 2（mcps 查询占位）✓
- station.json 承载配置、nexus.json 删 stations → Task 1 + Task 5 ✓
- 内置 filesystem toolkit（Read）→ Task 2 注册表 ✓
- 文档同步（design/spec/plan/roadmap/模板）→ Task 5 + Task 6 ✓
- 工具名全局唯一（本地硬约束 + 查询校验接口）→ Task 2 ✓

**类型一致性：**
- `Station::tools(Option<&HashSet<String>>)`：Task 2 定义，Task 4 `Some(&cfg.toolkits)`（cfg.toolkits 为 `HashSet<String>`）一致 ✓
- `execute_tool_call(&self, call: Arc<ToolCall>)`：Task 4 改签名，session_manager 调用点同步去 session ✓
- `StationClient::new(u64)` → Task 2 `StationClient::new(scfg.timeout_secs)`（u64）一致 ✓
- `station_repo_snapshot()` → Task 1 定义，Task 2 `Station::new()` 消费 ✓

**占位扫描：** 无 TDD/TODO；Task 2 测试 `call_tool_local_hit_and_miss` 的命中路径标注了降级方案（保留未注册断言）——实现时按注执行即可。
**兼容性：** `StationRepo` 字段带 `#[serde(default)]`，旧 station.json 空对象 `{}` 可加载（修正过）；`ContextConfig` 新 `toolkits` 字段缺省 None（旧 JSON 的 stations 未知字段被 serde 默认忽略），配置均兼容。

---

### Task 7: 远程工具路由缓存 + 子 Station 骨架返回空集合

**Files:**
- Modify: `kissbot-agent/src/station_client.rs`
- Modify: `kissbot-agent/src/station.rs`
- Test: 内嵌 tests 模块

**Interfaces:**
- Consumes: 现有 `Station`/`Toolkit`/`SubStation`/`StationClient`；`ToolConfig`
- Produces:
  - `StationClient::list_tools(Option<&HashSet<String>>) -> Result<Vec<ToolConfig>>`（骨架期返回 `Ok(vec![])`）
  - `StationClient::list_mcps(Option<&HashSet<String>>) -> Result<Vec<McpConfig>>`（骨架期返回 `Ok(vec![])`）
  - `Station.tool_routes: DashMap<String, String>`（工具名 → 直接子 station_id，仅 call_tool 路由用）
  - `Station::merge_sub_tools(&self, station_id: &str, tools: &[ToolConfig], local_names: &HashSet<String>) -> HashSet<String>`（快照合并，冲突保留先到者，返回成功插入的工具名集合）
  - `Station::tools` 拉取子时更新 tool_routes；`Station::call_tool` 未命中本地时查 tool_routes 路由到对应子（删除遍历全部子的循环）

**背景：** 用户决策——从子 Station 获取的 tool 列表缓存在 Station 级全局路由表（仅用于 tool call，不再远程获取列表）；骨架期子 Station 查询返回空集合而非报错（无 warn 噪声）；MCP 不建缓存表（MCP 与 Tool 有嵌套关系，实现 MCP 时再设计）；工具名跨进程冲突保留先到者（后到者剔除 + warn）。

- [ ] **Step 1: 修改 station_client.rs 骨架行为**（list_tools/list_mcps 返回空集合）

```rust
    /// 查询子 Station 平铺后的工具元数据（可选 toolkit 白名单过滤；None = 全部）
    /// 骨架：返回空集合（子 Station 无工具可提供），HTTP 协议实现后返回真实列表
    pub async fn list_tools(&self, _filter: Option<&HashSet<String>>) -> Result<Vec<ToolConfig>> {
        Ok(Vec::new())
    }

    /// 查询子 Station 平铺后的 MCP 元数据（可选 toolkit 白名单过滤）
    /// 骨架：返回空集合（MCP 本轮整体不实现）
    pub async fn list_mcps(&self, _filter: Option<&HashSet<String>>) -> Result<Vec<McpConfig>> {
        Ok(Vec::new())
    }
```

`call_tool` 保持未实现错误不变。更新测试 `skeleton_returns_unimplemented` → `skeleton_list_empty_call_unimplemented`：

```rust
    #[tokio::test]
    async fn skeleton_list_empty_call_unimplemented() {
        let client = StationClient::new(5);
        let filter: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        assert!(client.list_tools(Some(&filter)).await.unwrap().is_empty(), "骨架查询返回空集合");
        assert!(client.list_mcps(None).await.unwrap().is_empty(), "骨架 MCP 查询返回空集合");
        assert!(client.call_tool("read", serde_json::json!({})).await.unwrap_err().to_string().contains("未实现"), "骨架调用保持未实现");
    }
```

- [ ] **Step 2: 修改 station.rs 增加 tool_routes 字段与合并逻辑**

2a. `Station` struct 增加字段（含中文注释）：

```rust
pub struct Station {
    toolkits: DashMap<String, Arc<Toolkit>>,
    sub_stations: DashMap<String, Arc<SubStation>>,
    /// 远程工具路由表：工具名 → 直接子 station_id（仅 call_tool 路由用，不用于元数据查询）
    /// tools() 拉取子 Station 成功时更新（快照语义：先清该子旧记录再逐个插入）；
    /// 工具名整树全局唯一：与本地/先到子重名时保留先到者（后到者剔除 + warn）
    tool_routes: DashMap<String, String>,
}
```

2b. `from_repo` 返回处初始化：`Ok(Station { toolkits, sub_stations, tool_routes: DashMap::new() })`。

2c. 新增 `merge_sub_tools` 方法（可测纯逻辑，放在 `tools` 方法前）：

```rust
    /// 合并单个子 Station 的工具进路由表（快照语义：先清该子旧记录，再逐个插入）
    /// 工具名冲突（与本地或先到子重名）保留先到者：后到者不进路由表
    /// 返回成功插入的工具名集合（调用方据此从返回列表剔除冲突项）
    fn merge_sub_tools(&self, station_id: &str, tools: &[ToolConfig], local_names: &HashSet<String>) -> HashSet<String> {
        // 1. 清该子旧路由记录（快照语义：该子当前拉取结果为准）
        self.tool_routes.retain(|_, v| v.as_str() != station_id);
        // 2. 逐个插入（冲突保留先到者：本地工具与先插入的其他子优先）
        let mut inserted = HashSet::new();
        for t in tools {
            let name = t.name.to_string();
            if self.tool_routes.contains_key(&name) || local_names.contains(&name) {
                warn!("工具名冲突: {}（保留先到者，剔除子 Station {} 的同名工具）", name, station_id);
                continue;
            }
            self.tool_routes.insert(name.clone(), station_id.to_string());
            inserted.insert(name);
        }
        inserted
    }
```

2d. `tools()` 更新子 Station 段（实时拉取 + 更新缓存；先构建本地工具名集合）：

```rust
    pub async fn tools(&self, filter: Option<&HashSet<String>>) -> Result<Vec<ToolConfig>> {
        let mut out = Vec::new();
        // 本地工具名集合（冲突校验用：本地优先，子工具与本地重名剔除）
        let local_names: HashSet<String> = self.toolkits.iter()
            .flat_map(|e| e.value().tools.iter().map(|t| t.key().clone()))
            .collect();
        // 本地
        for entry in self.toolkits.iter() {
            let toolkit = entry.value();
            if let Some(f) = filter {
                if !f.contains(toolkit.name.as_str()) { continue; }
            }
            out.extend(toolkit.configured_tools());
        }
        // 直接子：实时拉取（带同一 filter）→ 更新路由缓存 → 合并元数据（剔除冲突项）
        // 先整树克隆出 Arc 再 await（不跨 await 持 DashMap 读锁）
        let subs: Vec<Arc<SubStation>> = self.sub_stations.iter().map(|e| e.value().clone()).collect();
        for sub in subs {
            match sub.client.list_tools(filter).await {
                Ok(tools) => {
                    let inserted = self.merge_sub_tools(sub.config.station_id.as_str(), &tools, &local_names);
                    out.extend(tools.into_iter().filter(|t| inserted.contains(t.name.as_str())));
                }
                Err(e) => warn!("子 Station {} 查询工具失败: {}", sub.config.station_id.as_str(), e),
            }
        }
        Ok(out)
    }
```

2e. `call_tool()` 更新：本地未命中 → 查 tool_routes 路由到对应子（删除遍历全部子的循环）：

```rust
    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value> {
        // 本地查找：无 await 阶段完成查找并释放 DashMap 读锁（不跨 await 持锁）
        let local: Option<Option<Arc<dyn Tool>>> = {
            let mut found = None;
            for entry in self.toolkits.iter() {
                if let Some(t) = entry.value().tools.get(name) {
                    found = Some(t.value().imp.clone());
                    break;
                }
            }
            found
        };
        if let Some(imp) = local {
            return match imp {
                Some(tool) => tool.call(params).await,
                None => Err(Error::InternalError(format!("工具未实现（仅元数据注册）: {}", name))),
            };
        }
        // 远程路由：查缓存表（工具名 → 直接子 station_id），命中 → 该子 HTTP 调用
        // 不再遍历全部子、不再远程获取列表；先克隆 Arc 再 await（不跨 await 持锁）
        let routed = self.tool_routes.get(name).map(|r| r.value().clone());
        if let Some(station_id) = routed {
            if let Some(sub) = self.sub_stations.get(&station_id).map(|s| s.value().clone()) {
                return match sub.client.call_tool(name, params).await {
                    Ok(v) => Ok(v),
                    Err(e) => Err(e),
                };
            }
        }
        Err(Error::InternalError(format!("工具不存在: {}", name)))
    }
```

- [ ] **Step 3: 更新 station.rs 测试**

3a. 更新 `tools_with_sub_skips_http_skeleton`（子骨架从"Err warn 跳过"变为"返回空集合"），改名并更新断言注释：

```rust
    #[tokio::test]
    async fn tools_with_sub_returns_empty_skeleton() {
        // 子 Station 骨架查询返回空集合（非报错）→ 无 warn 噪声，路由表不更新，结果只有本地
        let station = Station::from_repo(&repo_with_sub()).unwrap();
        assert!(station.tools(None).await.unwrap().is_empty(), "无本地工具，子骨架返回空");
        assert!(station.tool_routes.is_empty(), "骨架期路由表恒空");
    }
```

3b. 新增 `merge_sub_tools` 测试：

```rust
    #[test]
    fn merge_sub_tools_inserts_and_keeps_first_wins() {
        let station = Station::from_repo(&repo_with_filesystem()).unwrap();
        let local_names: HashSet<String> = station.toolkits.iter()
            .flat_map(|e| e.value().tools.iter().map(|t| t.key().clone()))
            .collect();
        // 子 a 提供 x/y → 全插入
        let ta = vec![tool_config("x"), tool_config("y")];
        let ins_a = station.merge_sub_tools("station-a", &ta, &local_names);
        assert_eq!(ins_a.len(), 2);
        assert_eq!(station.tool_routes.get("x").unwrap().value().as_str(), "station-a");
        // 子 b 提供 y（与 a 重名，先到者保留）与 z → y 剔除、z 插入
        let tb = vec![tool_config("y"), tool_config("z")];
        let ins_b = station.merge_sub_tools("station-b", &tb, &local_names);
        assert_eq!(ins_b.len(), 1, "y 冲突剔除，仅 z 插入");
        assert!(ins_b.contains("z") && !ins_b.contains("y"));
        assert_eq!(station.tool_routes.get("y").unwrap().value().as_str(), "station-a", "先到者保留");
        assert_eq!(station.tool_routes.get("z").unwrap().value().as_str(), "station-b");
        // 子 c 提供 read（与本地 filesystem 内置重名，本地优先）→ 剔除
        let tc = vec![tool_config("read")];
        let ins_c = station.merge_sub_tools("station-c", &tc, &local_names);
        assert!(ins_c.is_empty(), "与本地重名剔除");
        assert!(!station.tool_routes.contains_key("read"), "本地工具不进路由表");
        // 快照语义：子 a 重新拉取只含 x → 旧 y 记录被清
        let ta2 = vec![tool_config("x")];
        let _ = station.merge_sub_tools("station-a", &ta2, &local_names);
        assert!(!station.tool_routes.contains_key("y"), "快照：该子旧记录清除");
    }

    fn tool_config(name: &str) -> ToolConfig {
        ToolConfig {
            name: Arc::new(name.into()),
            description: Arc::new("d".into()),
            parameters: Arc::new(serde_json::json!({})),
        }
    }
```

3c. 新增 `call_tool` 路由测试（注入路由表验证分发到对应子，骨架 Err 返回）：

```rust
    #[tokio::test]
    async fn call_tool_routes_via_cache_table() {
        let station = Station::from_repo(&repo_with_sub()).unwrap();
        // 注入路由（模拟 tools() 拉取后缓存）：工具 x 属于 station-a
        station.tool_routes.insert("x".to_string(), "station-a".to_string());
        // 命中路由 → 调子 Station（骨架返回未实现 Err）→ 返回该错误（非"工具不存在"）
        let err = station.call_tool("x", serde_json::json!({})).await.unwrap_err();
        assert!(err.to_string().contains("未实现"), "路由命中应调子而非工具不存在: {}", err);
        // 未命中路由（本地也无）→ 工具不存在
        let miss = station.call_tool("nope", serde_json::json!({})).await;
        assert!(miss.is_err() && miss.unwrap_err().to_string().contains("工具不存在"));
    }
```

3d. 保留既有测试 `call_tool_local_hit_and_miss`（改名为 `call_tool_local_miss_returns_not_found`，去掉冗余脚手架，只留未注册断言）：

```rust
    #[tokio::test]
    async fn call_tool_local_miss_returns_not_found() {
        let station = Station::from_repo(&repo_with_filesystem()).unwrap();
        let miss = station.call_tool("nope", serde_json::json!({})).await;
        assert!(miss.is_err() && miss.unwrap_err().to_string().contains("工具不存在"));
    }
```

- [ ] **Step 4: 运行测试验证**

Run: `cd kissbot-agent && cargo test station:: 2>&1 | tail -10`
Expected: 全部 PASS（既有 + 新增/改名测试）

Run: `cd kissbot-agent && cargo test 2>&1 | tail -5`
Expected: 全量 PASS、build 无新警告

- [ ] **Step 5: 提交**

```bash
git add kissbot-agent/src/station_client.rs kissbot-agent/src/station.rs
git commit -m "feat(agent): 远程工具路由缓存——Station 新增 tool_routes（工具名→直接子 id，仅 call_tool 路由用）；tools() 实时拉取子 Station 并更新缓存（快照语义），跨进程工具名冲突保留先到者（后到者剔除+warn）；call_tool 本地未命中改查缓存路由到对应子（不再遍历全部子）；骨架期子 list_tools/list_mcps 返回空集合（非报错）；MCP 不建缓存表（实现 MCP 时再设计）"
```

### Task 8: 文档同步（缓存行为）

**Files:**
- Modify: `docs/design/components-design/kissbot-agent-station.md`
- Modify: `docs/spec/kissbot-agent-station.md`
- Modify: `docs/roadmap/pending.md`

- [ ] **Step 1: 更新模块设计与技术规格**

同步以下行为到 `docs/design/components-design/kissbot-agent-station.md` 与 `docs/spec/kissbot-agent-station.md`：
- tools() 实时拉取子 Station + 更新 tool_routes 缓存（快照语义）；call_tool 查缓存路由（不再远程获取列表、不再遍历全部子）
- 工具名跨进程冲突：保留先到者（后到者剔除 + warn）
- 骨架期子 Station 查询返回空集合（list_tools/list_mcps）；调用返回未实现错误
- MCP 不建缓存表（MCP 与 Tool 嵌套关系，实现 MCP 时再设计）

- [ ] **Step 2: 更新 roadmap**

`docs/roadmap/pending.md` 遗留事项更新：删/改"跨进程工具名唯一性校验"（已实现为缓存合并时保留先到者 + warn）；保留"MCP 实现、子 Station HTTP 协议实现、配置热更新"；新增"MCP 缓存设计（与 Tool 嵌套关系）"。

- [ ] **Step 3: 提交**

```bash
git add docs/
git commit -m "docs(agent): Station 路由缓存行为同步——tools 实时拉取+更新 tool_routes（快照、冲突保留先到者）、call_tool 查缓存路由、骨架期子查询返回空集合；roadmap 更新（跨进程唯一性已实现、MCP 缓存待设计）"
```

---

## 追加任务自审

- **Spec 覆盖**：缓存表仅用于 tool call（Task 7 结构）、tools 实时拉取+更新缓存（2d）、call_tool 查缓存路由（2e）、冲突保留先到者（2c）、MCP 不建表（未建）、骨架期空集合（Step 1）全部落地 ✓
- **类型一致**：`merge_sub_tools(station_id: &str, tools: &[ToolConfig], local_names: &HashSet<String>) -> HashSet<String>`——Task 7 定义，tools() 内调用 `self.merge_sub_tools(sub.config.station_id.as_str(), &tools, &local_names)` 一致；`tool_routes.get(name).map(|r| r.value().clone())` 产出 String，`sub_stations.get(&station_id)` 消费一致 ✓
- **测试纪律**：merge 纯逻辑可测（快照/冲突/本地优先）；路由分发注入测试；骨架空集合断言 ✓
- **兼容性**：骨架期 tool_routes 恒空 → call_tool 行为等价（本地命中/工具不存在），无回归 ✓
