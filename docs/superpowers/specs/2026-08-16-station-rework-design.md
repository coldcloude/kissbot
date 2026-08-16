# Station 系统重做设计

日期：2026-08-16
状态：设计已确认，待实现

## 背景与目标

agent 组件按设计应分为 Nexus（LLM 通信枢纽）与 Station（Tool 执行主机）两部分。现状代码中 `AgentCoordinator` 实际是 Nexus 的协调器，Station 为扁平的 `StationRuntime` 集合（base_url 空 = 本地、非空 = REST 骨架）。本次重做：

1. **重命名**：`AgentCoordinator` → `Nexus`，`coordinator.rs` → `nexus.rs`，对齐设计文档。
2. **Station 嵌套化**：每个 agent 只有一个全局 Station（静态访问单例）。Station 内部有 Toolkit 集合与子 Station 集合；子 Station 只能通过 HTTP 通信（本轮骨架），父只存直接子连接信息，孙子由子 Station 自己递归。
3. **Toolkit 级别**：每个 Tool 或 MCP 必须属于一个 Toolkit；Toolkit 中无子 Station。toolkit 名全局唯一命名空间（不同 Station 含子 Station 不能重名）。
4. **递归平铺**：Station 获取 Tool/MCP 时递归拉取子 Station 并平铺，像本地一样；可携带 toolkit 名白名单过滤，不带则返回全部。
5. **会话级 Toolkit 配置**：每个 Session 有自己启用的 toolkit 名集合（替代原启用 station_id 集合）。
6. **MCP 本轮不实现**，仅建必要 struct 占位。

## 命名与文件变更

| 现在 | 改为 |
|------|------|
| `coordinator.rs` + `AgentCoordinator` | `nexus.rs` + `Nexus`（单例模式不变：OnceLock + `Nexus::get()`） |
| `station.rs` + `StationRuntime` | `station.rs` 重写为 `Station`（全局单例）+ `Toolkit` + `SubStation` |
| `station_client.rs` + `StationClient` | 改造为子 Station HTTP 客户端（骨架） |
| `station_router.rs` + `StationRouter` | 删除（路由功能并入 Station） |
| `config_manager.rs` + `StationConfig` | 删除；`NexusRepo.stations` 段删除 |
| `config_manager.rs` + `ContextConfig.stations` | 改名 `toolkits`（启用 toolkit 名白名单集合） |

引用方同步更新：`main.rs`、`session_manager.rs`、`command_router.rs`。

## 数据结构

### 配置层（station.json，`StationRepo` 从空占位变为真正结构）

```json
{
  "toolkits": {
    "filesystem": {
      "tools": { "read": { "name": "read", "description": "...", "parameters": {...} } },
      "mcps": {}
    }
  },
  "sub_stations": {
    "station-a": { "station_id": "station-a", "base_url": "http://host:9001", "timeout_secs": 30 }
  }
}
```

```rust
// config_manager.rs
struct StationRepo {
    toolkits: Arc<ArcSwapHashMap<String, ToolkitConfig>>,
    sub_stations: Arc<ArcSwapHashMap<String, SubStationConfig>>,
}
struct ToolkitConfig {
    tools: Arc<ArcSwapHashMap<String, ToolConfig>>,   // key = 工具名
    mcps: Arc<ArcSwapHashMap<String, McpConfig>>,     // key = mcp 名
}
struct McpConfig {           // 占位，最小字段
    name: Arc<String>,
    description: Arc<String>,
}
struct SubStationConfig {    // 只存直接子连接信息；孙子由子进程自己递归
    station_id: Arc<String>,
    base_url: Arc<String>,
    timeout_secs: u64,
}
```

`ToolConfig` 保持不变（name/description/parameters）。`NexusRepo.stations` 字段删除。

### 运行态（station.rs）

```rust
struct Station {            // 全局单例 Station::get()，每个 agent 一个
    toolkits: DashMap<String, Arc<Toolkit>>,        // 本地 toolkit（含实现）
    sub_stations: DashMap<String, Arc<SubStation>>, // 直接子
}
struct Toolkit {
    name: String,
    tools: DashMap<String, Arc<dyn Tool>>,   // 实现表
    mcps: DashMap<String, Arc<McpConfig>>,   // 占位，无实现
}
struct SubStation {
    config: Arc<SubStationConfig>,
    client: StationClient,   // HTTP 客户端骨架
}
```

`Tool` trait 与 `ReadTool` 保持现状（统一 `serde_json::Value` 参数/返回）。

### 内置注册表

toolkit 名 `filesystem` → 内置实现（ReadTool + 其 ToolConfig 元数据）。配置显式声明 `filesystem` toolkit 时，启动时注册 read 的元数据与实现；未声明则不注册。内置工具也须显式配置 toolkit 才可用。

## 数据流

### 元数据查询（agentic loop 组装 tools 前）

`Station::tools(filter: Option<&[&str]>)` 语义：`None` = 不带过滤，返回全部 toolkit（用户原话：不带则返回全部）；`Some(列表)` = 白名单。

```
Nexus::tools_for_session(session)
  → ContextConfig.toolkits（白名单；None/空集合 = 无工具，与现状 stations 语义一致，不调 Station）
  → 非空 → Station::get().tools(Some(&toolkits))
      ├─ 本地：filter 命中 toolkit 名 → 收集该 toolkit 全部 ToolConfig
      └─ 直接子：逐个 HTTP 查询（带同一 filter；骨架期返回空集合）→ 更新工具路由缓存（见下）
  → 合并平铺（跨进程工具名冲突：保留先到者，后到者剔除 + warn）
```

注：`tools(None)` 返回全部 与 `tools(Some(&[]))` 返回空 是两种不同语义，勿混淆。

### 远程工具路由缓存（tool call 专用）

- **缓存位置**：`Station.tool_routes: DashMap<String, String>`（工具名 → 直接子 station_id），仅用于 `call_tool` 路由；本地工具不走缓存表（`call_tool` 先查本地实现表）
- **更新时机**：`tools()` 拉取子 Station 成功时更新（快照语义——先移除该子旧路由记录，再逐个插入；该子拉取失败保留旧缓存）
- **冲突处理**：合并时发现工具名已存在（与先到子/其他子重名）→ warn 日志，保留先到者，后到者不进缓存表、不进返回列表
- **MCP 不建缓存表**（MCP 与 Tool 有嵌套关系，实现 MCP 时再设计）；`mcps()` 实时拉取平铺返回
- **骨架期行为**：子 `list_tools` 返回空集合 → 每次 `tools()` 清空该子路由（快照）→ `tool_routes` 恒空 → 远程工具不可用（与现状等价，无 warn 噪声）

### 工具调用

```
Nexus::execute_tool_call → Station::get().call_tool(name, params)
  ├─ 本地 toolkit 实现表查 name（跨 toolkit 合并查，工具名全局唯一）→ 命中执行
  └─ 未命中 → 查 tool_routes 缓存表（工具名 → 直接子 station_id）→ 命中 → 该子 HTTP /tool/call
     （父不管孙子：子收到请求后自己递归自己的子；不再遍历全部子、不再远程获取列表）
  → 未命中 → Err(工具不存在)
```

### 子 Station HTTP 协议（骨架期：查询返回空集合，调用返回未实现）

- 查询工具元数据：`POST /tools`，请求体可带 `toolkits: Vec<String>`（白名单过滤），响应平铺 `ToolConfig` 列表；骨架期返回空列表（非报错）
- 查询 MCP 元数据：`POST /mcps`，同模式；骨架期返回空列表
- 调用工具：`POST /tool/call`，请求体 `{ name, params }`，响应执行结果；骨架期返回未实现错误
- MCP 相关接口占位（不实现）
- 子 Station 收到查询/调用请求后自己递归自己的子（孙子）

## Nexus 侧改造

- `Nexus` 删除 `station_runtimes` 字段及其构建逻辑，改为依赖全局 `Station` 单例
- `tools_for_session`：保留为薄封装——取 `ContextConfig.toolkits` → 调 `Station::get().tools(Some(&toolkits))`；空集合返回空（请求不携带 tools 字段）
- `execute_tool_call`：改为 `Station::get().call_tool(name, params)`，错误 JSON 包装逻辑不变
- 启动顺序：`ConfigManager::new()` → `Station::new()`（读 station.json 构建并注册单例）→ `Nexus::new()`；内置注册表按配置声明的 toolkit 注册实现

## 错误处理与唯一性

- **本地工具名冲突**（同一 Station 内跨 toolkit）：配置加载/注册时报 `InternalError`，启动失败——工具名必须整树唯一，本地先硬约束
- **平铺查询冲突**（跨进程）：合并子 Station 返回时发现工具名已存在 → warn 日志，保留先到者，后到者剔除（不进缓存表、不进返回列表）
- **调用未命中**：本地实现表与 tool_routes 缓存表都无 → `Err(工具不存在)`（与现状同语义，错误 JSON 由 Nexus 包装）
- **子 Station 查询失败**（HTTP 实现后网络错误）：记 warn 日志、跳过该子（不阻塞整体），旧缓存保留；骨架期查询恒返回空集合，不触发此分支

## 测试

- `station.rs` 单测：
  - 本地 toolkits 工具名冲突 → 构建报错
  - `tools(filter)`：无 filter 返回全部；白名单 filter 只返回命中的 toolkit；空 filter（无工具）
  - `call_tool`：本地实现命中执行；未注册报"工具不存在"；注入 tool_routes 后路由到对应子（骨架 Err → warn）
  - 内置 filesystem toolkit：声明后 read 工具可用（ToolConfig + 实现）
  - 合并逻辑 `merge_sub_tools`：正常插入；冲突保留先到者（后到者剔除 + warn）
- `config_manager.rs` 单测：`StationRepo` 新形状 serde 往返；`McpConfig` 占位序列化；`ContextConfig.toolkits` 改名后旧 `stations` 字段被忽略（静默回退空工具集）
- 现有测试更新：`AgentCoordinator` → `Nexus` 引用、`station_config` helper 形状

## 文档与配置模板同步

- `docs/design/components-design/kissbot-agent-station.md`：重写为嵌套 Station 结构（Toolkit / 子 Station / MCP 占位 / 递归平铺）
- `docs/design/system-design-agent.md`：Station 模块描述同步
- `docs/spec/`：更新或新增 station 技术规格（toolkit 全局唯一命名空间、子 Station HTTP 协议骨架、toolkits 白名单配置）
- `docs/plan/components-plan/kissbot-agent-station.md`：实现计划更新
- `docs/roadmap/pending.md`：登记遗留事项（MCP 实现、子 Station HTTP 协议实现、跨进程工具名唯一性校验）
- 配置模板：`script/template/station.json` 填入 filesystem toolkit 示例；`script/template/nexus.json` 删 `stations` 段、context 示例改 `toolkits`
- 本项目 spec 存放于 `docs/superpowers/specs/`（与项目正式技术规格 `docs/spec/` 分离）

## 范围外（本轮不做，仅登记）

- MCP 真实实现（仅 `McpConfig` 占位）
- 子 Station HTTP 协议实现（请求/响应结构定义，调用返回未实现）
- 配置热更新监听（`ConfigChangeListener`/`notify_listeners` 维持现状，station 变更重启生效）
