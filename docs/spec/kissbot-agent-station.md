# kissbot-agent-station 技术规格

Station — Agent 组件的 Tool 执行主机模块。本文档为 [kissbot-agent-station 模块设计](../design/components-design/kissbot-agent-station.md) 的技术细节约定：station.json 配置格式、group 全局唯一命名空间、递归平铺语义、工具名整树唯一、工具路由、MCP 端点协议、group 白名单与会话对应关系。MCP 协议与 HTTP 传输细节见 [kissbot-mcp 技术规格](kissbot-mcp.md)。

## 配置格式

Station 配置由 ConfigManager 管理，持久化到 `<data_dir>/station.json`（JSON 格式，UTF-8，\n 换行）。

### StationRepo（顶层）

```json
{
  "station_id": "station-self",
  "groups": {
    "filesystem": {}
  },
  "sub_stations": {
    "station-a": {
      "station_id": "station-a",
      "base_url": "http://127.0.0.1:9001",
      "timeout_secs": 30,
      "default_group": "station-a-tools"
    },
    "plain-mcp": {
      "station_id": "plain-mcp",
      "base_url": "http://127.0.0.1:9002",
      "timeout_secs": 30,
      "default_group": "plain-tools"
    }
  }
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| station_id | string | 本站点唯一标识（必填；被其他 station 作为 sub 调用时用于祖先链防环） |
| groups | map<group 名, GroupConfig> | 本级 group 集合；key = group 名 |
| sub_stations | map<station_id, SubStationConfig> | 直接子 Station 集合；key = station_id |

`station_id` 为必填；`groups` 与 `sub_stations` 带 `#[serde(default)]`，旧配置缺省时反序列化为空 map。

### GroupConfig

当前无字段：配置只声明本站点启用哪些 group，group 的工具元数据与实现由内置注册表按 group 名填充（注册的工具必须带原生实现）。

### 对外工具定义

Station 对外返回的工具定义即 MCP `Tool`，并用 `_meta["kissbot/group"]` 携带所属 group；字段定义见 [kissbot-mcp 技术规格](kissbot-mcp.md) 的 `tools/list`。

### SubStationConfig

| 字段 | 类型 | 说明 |
|------|------|------|
| station_id | string | 子 Station 唯一标识（与 map key 一致）；祖先链与默认 group 回落都用它 |
| base_url | string | 子 Station 基地址（MCP 端点为 `base_url` + `/mcp`） |
| timeout_secs | u64 | 请求超时（秒），用于 MCP 客户端构造 |
| default_group | string? | 子 Station 的默认 group；缺省（空串）时 `effective_group()` 回落为 station_id |

只存直接子连接信息；子 Station 内部结构（groups / 孙子）由子进程自己管理，父通过 MCP 查询。
子 Station 可以是 kissbot Station，也可以是任意第三方 MCP 服务端；两者由 kissbot 扩展能力声明区分（见「第三方 MCP 服务端」）。

## group 全局唯一命名空间

- **group 名**在整棵 Station 树（本级 group + 所有子 Station 递归）上构成全局唯一命名空间，子 Station 的 group 不能与父或兄弟重名
- 本级约束：`Station::from_repo` 构建时以本级 group 名填充；跨进程部分由部署保证（见「工具名整树唯一」）
- 内置注册表占用 `filesystem` 命名空间：配置显式声明 `groups["filesystem"]` 才注册内置 read 工具实现；未声明则不注册

## 递归平铺语义

`Station::list_tools(filter, ancestors)`：`filter: Option<&HashSet<String>>`（group 名白名单）；`ancestors` 为根到当前父节点的 station_id 链。

| filter | 语义 |
|--------|------|
| None | 返回全部 |
| Some(空集) | 返回空 |
| Some(白名单) | 只返回命中白名单的 group 的工具 |

- 本级：遍历本级 group，白名单外的跳过，命中且非空的收集工具定义与 group 名
- 直接子：经 `McpClient` 实时拉取并按对端能力处理（见「第三方 MCP 服务端」）；成功时合并工具定义并更新工具路由（见「工具路由」），失败 → warn 跳过、保留该子上一次查询的路由记录
- 祖先链：所有子请求都附带 `ancestors + 本站 station_id`（纯提示信息，规范要求忽略未知 `_meta` 键）
- 自环检测：任一层发现自己的 `station_id` 已在 `ancestors` 中，返回 JSON-RPC 错误 `STATION_CYCLE`（`1001`）
- 结果返回工具定义数组，每个工具都在 `_meta["kissbot/group"]` 携带所属 group：本级工具用其所属 group，子站工具优先用对端携带的 group、缺省回落该子站默认 group
- 聚合为空 → 返回空工具列表；LLM 请求不携带 tools 字段（兼容无工具场景）
- 顺序确定性：结果按工具名排序、group 名排序去重（同一集合多次请求返回相同顺序，便于客户端缓存与提示词缓存）

## 第三方 MCP 服务端

子 Station 可以是任意第三方 MCP 服务端，不要求它理解 kissbot 的 `group` 与 `ancestors` 扩展。对端是否支持由 `server/discover` 一次性判定并缓存：

- 探测：`SubStation` 首次需要判定时调 `server/discover`，读取 `capabilities.kissbot`；成功即缓存（含「未声明」的否定结果，第三方通用服务端只探一次），失败不缓存、下次重试
- 支持（`capabilities.kissbot.group` 为 true）：请求下传 group 白名单，工具 group 以对端工具定义携带的 `kissbot/group` 为准
- 不支持：不下传 group（对方会忽略），其工具整体归入 `SubStationConfig.effective_group()`（`default_group`，缺省回落 station_id）；本级白名单不含该默认 group 时，该子工具整体被本地过滤掉
- 工具定义未携带 `kissbot/group`：归入 `effective_group()`
- 祖先链对所有子都附带：对不支持者只是被忽略的未知 `_meta` 键；对同样实现 kissbot 扩展但未声明的同级服务端，附带可保留防环能力

## 工具名整树唯一

- 工具名在整棵 Station 树（本级所有 group + 所有子 Station 递归）上全局唯一
- **本级硬约束**：`Station::from_repo` 构建时对全部 group 的工具做去重校验，重名启动失败返回「工具名冲突」（内置注册表自身保证不重名，该校验是守卫）
- **跨进程冲突处理**：`list_tools` 合并子 Station 工具时发现工具名已存在（与本级或先到子重名）→ warn 日志，保留先到者，后到者剔除（不进路由表、不进返回列表）

## 工具路由（tool_routes）

- **路由表位置**：`Station.tool_routes: DashMap<String, String>`（工具名 → 直接子 station_id）；仅用于 `call_tool` 路由，不用于元数据查询；本级工具不进路由表（`call_tool` 先查本级实现表）
- **更新时机**：`list_tools` 拉取子 Station 成功时更新（快照语义——先清该子上一次查询的路由记录，再逐个插入；该子拉取失败时保留上一次查询的记录）
- **冲突处理**：合并时工具名已存在（与本级或先到子重名）→ warn 日志，保留先到者，后到者不进路由表、不进返回列表
- **路由查询**：`call_tool` 本级未命中时按工具名查路由表，命中即调该子；不遍历全部子、不远程获取列表；路由未命中返回「工具不存在」

## MCP 端点

station 自身作为 MCP 服务端，监听公共配置 `agent.station_host` / `agent.station_port` 的 `/mcp` 端点；协议（方法、消息结构、请求头、状态码）见 [kissbot-mcp 技术规格](kissbot-mcp.md)。认证使用 `security.api_key`（`X-Api-Key` 头，由 kissbot-security 的 AuthLayer 叠加）。

对外方法：

| 方法 | 请求要点 | 响应要点 |
|------|----------|----------|
| server/discover | 仅 `_meta` | `supportedVersions`（`["2026-07-28"]`）、`capabilities.tools`、`instructions`、`_meta.serverInfo`（name = `kissbot-station`） |
| tools/list | `_meta["kissbot/groups"]`（组白名单）、`_meta["kissbot/ancestors"]` | `tools`（MCP 工具定义，各工具 `_meta["kissbot/group"]` 携带所属 group） |
| tools/call | `name`、`arguments`、`_meta["kissbot/ancestors"]` | `content` / `structuredContent` / `isError`；自环返回 JSON-RPC 错误 `1001` |

- 工具级失败（不存在 / 参数错误 / 执行报错 / 子调用失败）返回 HTTP 200 + `isError=true`，固定错误结构落在 `structuredContent`，文案同时进 `content`
- 协议级失败（自环）返回 JSON-RPC 错误 `1001`，HTTP 200
- 认证失败 401；传输 / 请求头 / `_meta` 校验失败 400；未知方法 404

### 固定错误结构（`structuredContent`）

`isError=true` 时 `structuredContent` 为固定结构 `{ code, message, source }`；kissbot 实现固定 `source = kissbot`。
`structuredContent` 仅用于 Station 内部处理（如按 `code` 识别循环引用；station 之间逐跳转发以支持这种判定），不进入 Nexus，也不作为工具结果回传 LLM。
解析失败、结构不符或 `source` 非 kissbot 时，消费方按原样透传，不按错误码解释。

### 工具错误码（`structuredContent.code`）

| 码 | 常量 | 说明 |
|----|------|------|
| 0x0000_0001 | TERR_TOOL_NOT_FOUND | 工具不存在 |
| 0x0000_0003 | TERR_TOOL_WRONG_ARGUMENTS | 参数错误 |
| 0x0000_0004 | TERR_TOOL_CYCLE | 级联自环（子站以工具级错误上报时本级识别并上抛） |
| 0x0001_0000 | TERR_TOOL_INTERNAL_ERROR | 本级工具执行失败 |
| 0x0002_0000 | TERR_TOOL_EXTERNAL_ERROR | 子 Station 调用失败（文案含底层原因） |

本级自环以协议级 JSON-RPC 错误 `STATION_CYCLE`（`1001`）返回；子站若以工具级错误码 `TERR_TOOL_CYCLE` 上报自环，本级识别后同样以 `STATION_CYCLE` 上抛。

## 内置注册表

| group | 工具 | 说明 |
|-------|------|------|
| filesystem | read | 读取文本文件；路径经绝对路径规范化后校验位于进程工作目录内（防路径穿透）；返回内容限长 64KB |

配置显式声明 `groups["filesystem"]` 才注册；工具的元数据（含参数 schema）与原生实现由内置注册表成对提供，配置不声明工具。

## group 白名单与会话对应关系

- **会话 group 白名单（GroupSetConfig.group_set）**：`Arc<HashSet<String>>`，会话启用的 group 名集合；默认空集 = 无工具
- 三层继承合并（默认 ← agent 级 ← role 级）后的 `group_set` 即为会话 group 白名单（与其它会话配置一样按 agent → role → session 逐层合成）
- `Nexus::tools_for_session(session_key)`：读会话的 group 白名单 → 为空直接返回空 → `Station::list_tools(Some(&group_set), &[])` 平铺查询；结果由 `From<&kissbot_mcp::Tool> for ToolConfig` 转为 LLM 侧工具定义（`parameters` 取 `inputSchema`），随 LLM 请求的 tools 字段发送
- 工具执行：`AgentPipeline::call_tool`（记忆写入 + 调用）→ `StationToolCaller` 适配层 → `Station::call_tool(name, arguments, &[])`；`CallToolResult` 还原为 `ToolData`——只承载 MCP `content` 字段转成的 `Value`（内容块数组，完整保留 text / image / audio / resource_link / resource 各类型）：成功进 `result`，失败进 `error`（`result` 留空）；`structuredContent` 不进入 Nexus
- 工具结果进入 LLM 上下文前由输出处理器渲染：成功取 `result`、失败取 `error`，文本块取原文、字符串原样、其余块给紧凑占位（二进制内容不进入上下文）
- `ToolCall` / `ToolData` 属于 Nexus 交互层，Station 内部不使用；Station 内部只以 MCP 工具类型与工具名 + 参数交互
- 工具名整树唯一与 group 白名单共同保证：任一工具在整棵 Station 树唯一，白名单按 group 名粗粒度放行

## 启动顺序

ConfigManager → Station → Nexus：

```
ConfigManager::new()   // 加载 KISSBOT_CONFIG agent 段，引导 nexus.json/station.json，注册单例
Station::new()         // 读 station.json 构建全局 Station 单例，注册单例
Nexus::new()           // 装配运行时组件，注册单例；run() 中执行连接与启动动作
StationHttpServer::start()  // 后台启动 station MCP 服务端（agent.station_host:agent.station_port 的 /mcp 端点）
```
