# kissbot-agent-station 模块设计

## 概述
Station — Agent 组件的 Tool 执行主机模块。智能体的"行动"部分，专注于执行具体的工具操作。Station 不直接与 LLM 通信，只提供工具注册、接收 tool call、执行工具、将结果返回给 nexus 的能力。

Station 采用**嵌套结构**：
- **全局单例**：每个 agent 进程内只有一个全局 Station，启动时从配置构建
- **两级节点**：Station 内部由两层组成——本地 **Toolkit 集合**（含工具实现）与直接**子 Station 集合**（仅连接信息，HTTP 通信）；Toolkit 中无子 Station
- **子 Station 仅 HTTP**：子 Station 只能通过 HTTP 与父通信；父只存直接子连接信息，孙子由子 Station 进程自己递归，父不管
- **station 自身也是服务端**：每个 station 监听独立 `agent.station_host:agent.station_port`，可被其他 station 当作 sub 调用
- **配置落盘**：Station 配置由 ConfigManager 承载，可修改并持久化

Station 可以运行在多种形态的设备上：
- **通用服务器**：运行标准工具集（文件操作、命令执行等）
- **网络设备**：读写网络配置、获取监控数据等
- **智能家电**：执行物理世界操作（开关、调节等）
- **机器人**：执行物理动作（移动、抓取等）

## 核心功能

1. **工具注册与管理**：Station 配置声明 toolkit 集合（toolkit 名 → 工具列表），本地 Toolkit 在启动时注册工具实现（内置注册表 + 配置声明），工具元数据含名称、描述、参数格式说明
2. **工具元数据平铺递归**：实时拉取本地 Toolkit 与直接子 Station 的工具元数据，供 nexus 收集发给 LLM 的工具定义；子 Station 经 HTTP 实时拉取，拉取成功时更新路由缓存（快照语义）
3. **toolkit 白名单过滤**：工具元数据查询支持按 toolkit 名集合过滤——不传 = 全部、空集 = 空、白名单 = 命中白名单的 toolkit
4. **工具名整树全局唯一**：工具名在整棵 Station 树（本地 toolkit + 所有子 Station 递归）上全局唯一；本地为硬约束（构建时冲突启动失败），跨进程冲突在路由缓存合并时处理——保留先到者，后到者剔除并记录告警
5. **工具执行**：接收来自 nexus 的 tool call，按工具名查找并执行工具，返回结果

## 内部模块

### 1. Tool 接口 - 工具接口
- 统一参数与返回值
- 异步执行，由实现者提供具体实现

### 2. Station - 全局单例（每 agent 一个）
- 进程内唯一，启动时从配置构建并注册单例，静态访问
- 持有本站点 station_id、本地 Toolkit 集合（key = toolkit 名）、直接子 Station 集合（key = station_id）与远程工具路由表（工具名 → 直接子 station_id，仅工具调用路由用，不用于元数据查询）
- 构建流程：内置注册表填充声明 toolkit 的实现 → 配置声明的 tools/mcps 补充元数据 → 工具名唯一性校验（本地硬约束）→ 子 Station 仅存连接信息；路由表初始为空
- 平铺查询：本地 toolkit 白名单过滤 + 直接子 HTTP 实时拉取并更新路由缓存（快照语义：先清该子旧记录再插入，跨进程冲突保留先到者并记录告警；子查询失败告警跳过、旧缓存保留）；请求带祖先链，检测到自身在祖先链中返回自环错误；MCP 查询本地过滤 + 直接子实时拉取（不建缓存表，MCP 与 Tool 为嵌套关系，实现 MCP 时再设计）
- 工具调用：本地实现表（工具名全局唯一，跨 toolkit 查找）命中执行 → 未命中查路由缓存路由到对应子 HTTP 调用（不遍历全部子、不远程获取列表）→ 路由未命中报"工具不存在"

### 3. Toolkit - 本地工具集（无子 Station）
- 持有工具实现表（key = 工具名；条目含元数据 + 可选本地实现，无本地实现时调用返回未实现）与 MCP 占位表（key = mcp 名）
- Toolkit 中无子 Station（子 Station 只挂在 Station 级）
- 提供本 toolkit 的工具与 MCP 元数据列表

### 4. SubStation - 子 Station 运行态（仅 HTTP）
- 只存直接子连接信息（station_id / 地址 / 超时）与 HTTP 客户端
- 子 Station 内部结构（toolkits / 孙子）由子进程自己管理，父通过 HTTP 查询；孙子由子进程递归，父不管

### 5. 内置注册表 - 内置 toolkit（filesystem）
- 本轮仅 filesystem toolkit：read 工具（读取文本文件）
- 配置显式声明对应 toolkit 名时才注册内置实现；未声明则不注册（内置工具也须显式配置 toolkit 才可用）
- 路径安全校验：读取路径须位于当前工作目录内，越界拒绝，防路径穿透；返回内容限长

### 6. StationClient - 子 Station HTTP 客户端
- 真实 HTTP 客户端：工具列表 / MCP 列表 / 工具调用，参数含可选 toolkit 白名单过滤与祖先链
- 子 Station 只能 HTTP 通信，不可本地调用；协议见 [kissbot-agent-station 技术规格](../../spec/kissbot-agent-station.md)

### 7. MCP 占位
- 配置的 MCP 段存 MCP 元数据（名称/描述），本轮仅建结构，不实现调用
- MCP 平铺查询为占位接口（无生产消费方），本地返回配置、直接子 HTTP 实时拉取；不建缓存表（MCP 与 Tool 为嵌套关系，缓存设计留待实现 MCP 时设计）

## 配置格式

Station 配置由 ConfigManager 管理并持久化，包含：
- **station_id**：本站点唯一标识，祖先链防环使用
- **toolkits**：toolkit 名 → toolkit 配置；toolkit 名全局唯一命名空间
- **内置注册表填充**：声明 filesystem toolkit 时由内置注册表注册 read 工具（见第 5 节），配置中 tools 无需（也不应）声明 read——显式声明同名 read 会在注册步骤触发"工具名冲突"导致启动失败
- **ToolkitConfig**：tools = 工具名 → 工具配置（名称/描述/参数格式说明）+ mcps = mcp 名 → MCP 元数据（占位，本轮仅建结构不实现调用，见第 7 节）
- **sub_stations**：station_id → 子 Station 连接配置（station_id/地址/超时），只存直接子连接信息

详细字段说明见 [kissbot-agent-station 技术规格](../../spec/kissbot-agent-station.md)。

## 功能流程

### 工具元数据查询（实时拉取 + 路由缓存更新）
Nexus 的 agentic loop 需要工具定义时 → 读取会话 context 配置的 toolkit 白名单 → 平铺查询：
1. 遍历本地 Toolkit：白名单外的 toolkit 跳过，命中的收集工具元数据（同时收集本地工具名集合，供冲突校验用——本地优先，子工具与本地重名剔除）
2. 遍历直接子 Station：经 HTTP 实时拉取 → 成功则更新路由缓存（快照语义：先清该子旧记录再逐个插入；跨进程工具名冲突保留先到者，后到者剔除并记录告警，且不进返回列表）；查询失败 → 告警跳过、旧缓存保留
3. 聚合结果返回 nexus 随 LLM 请求发送；聚合为空则请求不携带工具定义（兼容无工具场景）

### 工具调用流程
Nexus 的 agentic loop 解析 LLM 返回的 tool call → 按工具名调用：
1. 本地实现表查找（工具名全局唯一，跨 toolkit 至多命中一个）：命中执行本地实现（仅元数据注册无本地实现 → 返回未实现）；未命中 → 查远程工具路由表
2. 查路由缓存表（工具名 → 直接子 station_id）：命中 → 经 HTTP 调该子（父不管孙子，子收到请求后自己递归自己的子）；不遍历全部子、不远程获取列表
3. 路由未命中 → 返回"工具不存在"

所有记忆操作（包括 tool call 和 tool result 的记录）由 nexus 统一完成。

### Station 启动流程
```
kissbot-agent 启动
  → ConfigManager（加载 agent 配置，引导 nexus/station 持久化配置）
  → Station（读配置 → 内置注册表填充 + 配置声明补充 + 工具名唯一性校验 + 子 Station 连接信息 → 注册全局单例）
  → Nexus（装配 + 注册单例）
  → StationHttpServer（后台启动 station 对外 HTTP 服务）
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
- **配置热更新监听**：ConfigManager 提供配置变更监听能力，接入 Station/Nexus 为遗留事项
