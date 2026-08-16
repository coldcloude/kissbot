# kissbot-agent-station 模块设计

## 概述
Station — Agent 组件的 Tool 执行主机模块。智能体的"行动"部分，专注于执行具体的工具操作。Station 不直接与 LLM 通信，只提供工具注册、接收 tool call、执行工具、将结果返回给 nexus 的能力。

Station 采用**嵌套结构**：
- **全局单例**：每个 agent 进程内只有一个全局 Station（`Station::get()` / `Station::new()`，OnceLock 静态访问），启动时从 station.json 构建
- **两级节点**：Station 内部由两层组成——本地 **Toolkit 集合**（含工具实现）与直接**子 Station 集合**（仅连接信息，HTTP 通信）；Toolkit 中无子 Station
- **子 Station 仅 HTTP**：子 Station 只能通过 HTTP 与父通信（本轮为骨架，未实现）；父只存直接子连接信息，孙子由子 Station 进程自己递归，父不管
- **配置落盘**：Station 配置由 ConfigManager 承载，持久化到 `<data_dir>/station.json`（`StationRepo { toolkits, sub_stations }`）

Station 可以运行在多种形态的设备上：
- **通用服务器**：运行标准工具集（文件操作、命令执行等）
- **网络设备**：读写网络配置、获取监控数据等
- **智能家电**：执行物理世界操作（开关、调节等）
- **机器人**：执行物理动作（移动、抓取等）

## 核心功能

1. **工具注册与管理**：Station 配置声明 toolkit 集合（toolkit 名 → 工具列表），本地 Toolkit 在启动时注册工具实现（内置注册表 + 配置声明），工具元数据含名称、描述、参数 JSON Schema
2. **工具元数据平铺递归**：`tools(filter)` 实时拉取本地 Toolkit 与直接子 Station 的工具元数据，供 nexus 收集发给 LLM 的工具定义；子 Station 经 HTTP 实时拉取（骨架期返回空集合，非报错无 warn 噪声），拉取成功时更新 tool_routes 路由缓存（快照语义）
3. **toolkit 白名单过滤**：`tools(filter)` 的 filter（toolkit 名集合）语义：`None` = 全部、`Some(空集)` = 空、`Some(白名单)` = 命中白名单的 toolkit
4. **工具名整树全局唯一**：工具名在整棵 Station 树（本地 toolkit + 所有子 Station 递归）上全局唯一；本地为硬约束（构建时冲突启动失败），跨进程冲突在路由缓存合并时处理——保留先到者，后到者剔除 + warn
5. **工具执行**：接收来自 nexus 的 tool call，按工具名查找并执行工具，返回结果

## 内部模块

### 1. Tool trait - 工具接口
- 统一参数（serde_json::Value）与返回值（serde_json::Value）
- 异步执行，实现者实现 `call` 方法

### 2. Station - 全局单例（每 agent 一个）
- 进程内唯一（OnceLock），`new()` 从 ConfigManager 读 station.json 构建并注册单例，`get()` 静态访问
- 持有本地 Toolkit 集合（`DashMap<String, Arc<Toolkit>>`，key = toolkit 名）、直接子 Station 集合（`DashMap<String, Arc<SubStation>>`，key = station_id）与远程工具路由表（`tool_routes: DashMap<String, String>`，工具名 → 直接子 station_id，仅 call_tool 路由用，不用于元数据查询）
- 构建流程：内置注册表填充声明 toolkit 的实现 → 配置声明的 tools/mcps 补充元数据 → 工具名唯一性校验（本地硬约束）→ 子 Station 仅存连接信息；tool_routes 初始为空
- 平铺查询：`tools(filter)` 本地 toolkit 白名单过滤 + 直接子实时拉取（骨架期返回空集合，非报错无 warn 噪声）并更新 tool_routes 缓存（快照语义：先清该子旧记录再插入，跨进程冲突保留先到者 + warn；子查询失败 warn 跳过、旧缓存保留）；`mcps(filter)` 本地过滤 + 直接子实时拉取（不建缓存表，MCP 与 Tool 嵌套关系，实现 MCP 时再设计）
- 工具调用：`call_tool(name, params)` 本地实现表（工具名全局唯一，跨 toolkit 查找）命中执行 → 未命中查 tool_routes 缓存路由到对应子 HTTP 调用（不再遍历全部子、不再远程获取列表）→ 路由未命中报"工具不存在"

### 3. Toolkit - 本地工具集（无子 Station）
- 持有工具实现表（`DashMap<String, Arc<ToolkitEntry>>`，key = 工具名；条目含元数据 + 可选本地实现，None = 仅元数据注册、调用返回未实现）与 MCP 占位表（`DashMap<String, Arc<McpConfig>>`，key = mcp 名）
- Toolkit 中无子 Station（子 Station 只挂在 Station 级）
- 提供 `configured_tools()` / `configured_mcps()`（该 toolkit 的元数据列表）

### 4. SubStation - 子 Station 运行态（仅 HTTP）
- 只存直接子连接信息（station_id / base_url / timeout_secs）与 HTTP 客户端（StationClient）
- 子 Station 内部结构（toolkits / 孙子）由子进程自己管理，父通过 HTTP 查询（骨架期 list_tools/list_mcps 返回空集合、call_tool 返回未实现错误）；孙子由子进程递归，父不管

### 5. 内置注册表 - 内置 toolkit（filesystem）
- 本轮仅 filesystem toolkit：read 工具（读取文本文件，参数 path）
- 配置显式声明对应 toolkit 名（`toolkits["filesystem"]`）时才注册内置实现；未声明则不注册（内置工具也须显式配置 toolkit 才可用）
- 路径安全校验：参数 path 先基于当前工作目录解析为绝对路径，再规范化（消解 `..` 与符号链接），校验其位于当前工作目录或其子目录内，越界拒绝，防路径穿透；返回内容限长（64KB）

### 6. StationClient - 子 Station HTTP 客户端（骨架）
- 请求/响应结构意图已定义（list_tools / list_mcps / call_tool，参数含可选 toolkit 白名单过滤）；骨架期 list_tools/list_mcps 返回空集合（非报错，无 warn 噪声）、call_tool 返回未实现错误
- 子 Station 只能 HTTP 通信，不可本地调用；后续实现见 [kissbot-agent-station 技术规格](../../spec/kissbot-agent-station.md) 协议骨架章节

### 7. MCP 占位
- ToolkitConfig.mcps 存 MCP 元数据（McpConfig：name/description），本轮仅建结构，不实现调用
- `mcps(filter)` 平铺查询为占位接口（无生产消费方），本地返回配置、直接子实时拉取（骨架期返回空集合）；不建缓存表（MCP 与 Tool 为嵌套关系，缓存设计留待实现 MCP 时设计）

## 配置格式

Station 配置由 ConfigManager 管理，持久化到 `<data_dir>/station.json`：

```json
{
  "toolkits": {
    "filesystem": {
      "tools": {},
      "mcps": {
        "mcp1": { "name": "mcp1", "description": "占位" }
      }
    }
  },
  "sub_stations": {
    "station-a": {
      "station_id": "station-a",
      "base_url": "http://127.0.0.1:9001",
      "timeout_secs": 30
    }
  }
}
```

- **toolkits**：map<toolkit 名, ToolkitConfig>；toolkit 名全局唯一命名空间
- **内置注册表填充**：声明 `toolkits["filesystem"]` 时由内置注册表注册 read 工具（见第 5 节），配置中 tools 无需（也不应）声明 read——显式声明同名 read 会在 `Station::from_repo` 注册步骤 2 触发"工具名冲突: read（toolkit 内全局唯一）"导致启动失败
- **ToolkitConfig**：tools = map<工具名, ToolConfig>（name/description/parameters JSON Schema）+ mcps = map<mcp 名, McpConfig>（占位，本轮仅建结构不实现调用，见第 7 节）
- **sub_stations**：map<station_id, SubStationConfig>（station_id/base_url/timeout_secs），只存直接子连接信息
- 兼容旧配置：toolkits/sub_stations 带 `#[serde(default)]`，旧 station.json 空对象 `{}` 可加载

详细字段说明见 [kissbot-agent-station 技术规格](../../spec/kissbot-agent-station.md)。

## 功能流程

### 工具元数据查询（实时拉取 + 路由缓存更新）
Nexus 的 agentic loop 需要工具定义时 → `tools_for_session` 读会话 context 配置的 toolkit 白名单 → `Station::tools(Some(&cfg.toolkits))`：
1. 遍历本地 Toolkit：白名单外的 toolkit 跳过，命中的收集 `configured_tools()`（同时收集本地工具名集合，供冲突校验用——本地优先，子工具与本地重名剔除）
2. 遍历直接子 Station：经 StationClient.list_tools 实时拉取（骨架期返回空集合，非报错无 warn 噪声）→ 成功则更新 tool_routes 缓存（快照语义：先清该子旧记录再逐个插入；跨进程工具名冲突保留先到者，后到者剔除 + warn，且不进返回列表）；查询失败（HTTP 实现后网络错误）→ warn 跳过、旧缓存保留
3. 聚合结果返回 nexus 随 LLM 请求发送；聚合为空则请求不携带 tools 字段（兼容无工具场景）

### 工具调用流程
Nexus 的 agentic loop 解析 LLM 返回的 tool call → `execute_tool_call(call)` → `Station::call_tool(name, params)`：
1. 本地实现表查找（工具名全局唯一，跨 toolkit 至多命中一个）：命中执行本地实现（仅元数据注册无本地实现 → 返回未实现）；未命中 → 查远程工具路由表
2. 查 tool_routes 缓存表（工具名 → 直接子 station_id）：命中 → 经 StationClient.call_tool 调该子（骨架期返回未实现错误；父不管孙子，子收到请求后自己递归自己的子）；不再遍历全部子、不再远程获取列表
3. 路由未命中 → 返回"工具不存在"

所有记忆操作（包括 tool call 和 tool result 的记录）由 nexus 统一完成。

### Station 启动流程
```
kissbot-agent 启动
  → ConfigManager::new()（加载 KISSBOT_CONFIG agent 段，按 data_dir 引导 nexus.json/station.json）
  → Station::new()（读 station.json → 内置注册表填充 + 配置声明补充 + 工具名唯一性校验 + 子 Station 连接信息 → 注册全局单例）
  → Nexus::new()（装配 + 注册单例）
```

## 常见 Station 类型

### 工程工具站
注册文件操作和命令执行等工程工具。绑定本地工作区目录，提供文件系统和 shell 操作。

### 网络工具站
注册网络搜索和网页抓取等工具。提供网络信息获取能力。

### 设备工具站
运行在网络设备、智能家电、机器人等物理设备上。提供设备相关的控制、读写、监控等工具。

## 本轮范围外

- **MCP 真实实现**：mcps 仅占位结构，无调用实现；MCP 缓存表不建（MCP 与 Tool 为嵌套关系，缓存设计留待实现 MCP 时设计）
- **子 Station HTTP 协议实现**：StationClient 骨架已就位（list_tools/list_mcps 返回空集合、call_tool 返回未实现），HTTP 请求/响应后续实现
- **配置热更新监听**：ConfigManager 已预留 add_listener/notify_listeners，接入 Station/Nexus 为遗留事项
