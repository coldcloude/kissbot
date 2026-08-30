# kissbot-agent-station 技术规格

Station — Agent 组件的 Tool 执行主机模块。本文档为 [kissbot-agent-station 模块设计](../design/components-design/kissbot-agent-station.md) 的技术细节约定：station.json 配置格式、toolkit 全局唯一命名空间、递归平铺语义、工具名整树唯一、远程工具路由缓存、子 Station HTTP 协议、ContextConfig.toolkits 白名单与 Session 对应关系。

## 配置格式

Station 配置由 ConfigManager 管理，持久化到 `<data_dir>/station.json`（JSON 格式，UTF-8，\n 换行）。

### StationRepo（顶层）

```json
{
  "station_id": "station-self",
  "toolkits": {
    "filesystem": {
      "tools": {},
      "mcps": {}
    },
    "my-toolkit": {
      "tools": {
        "my-tool": {
          "name": "my-tool",
          "description": "自定义工具描述",
          "parameters": { "type": "object", "properties": {} }
        }
      },
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

| 字段 | 类型 | 说明 |
|------|------|------|
| station_id | string | 本站点唯一标识（必填；被其他 station 作为 sub 调用时用于祖先链防环） |
| toolkits | map<toolkit 名, ToolkitConfig> | 本地 toolkit 集合；key = toolkit 名 |
| sub_stations | map<station_id, SubStationConfig> | 直接子 Station 集合；key = station_id |

`station_id` 为必填；`toolkits` 与 `sub_stations` 带 `#[serde(default)]`，旧配置缺省时反序列化为空 map。

### ToolkitConfig

| 字段 | 类型 | 说明 |
|------|------|------|
| tools | map<工具名, ToolConfig> | 工具元数据（key = 工具名）；Toolkit 中无子 Station |
| mcps | map<mcp 名, McpConfig> | MCP 元数据（key = mcp 名；本轮占位，无实现） |

### ToolConfig

| 字段 | 类型 | 说明 |
|------|------|------|
| name | string | 工具名（与 map key 一致） |
| description | string | 工具描述 |
| parameters | object | 参数 JSON Schema（OpenAI tools[].function.parameters） |

### McpConfig

| 字段 | 类型 | 说明 |
|------|------|------|
| name | string | MCP 名（与 map key 一致） |
| description | string | MCP 描述（占位） |

本轮仅建结构，不实现调用。

### SubStationConfig

| 字段 | 类型 | 说明 |
|------|------|------|
| station_id | string | 子 Station 唯一标识（与 map key 一致） |
| base_url | string | 子 Station HTTP 地址 |
| timeout_secs | u64 | 请求超时（秒），用于 StationClient::new(timeout_secs) |

只存直接子连接信息；子 Station 内部结构（toolkits / 孙子）由子进程自己管理，父通过 HTTP 查询。

## toolkit 全局唯一命名空间

- **toolkit 名**在整棵 Station 树（本地 toolkit + 所有子 Station 递归）上构成全局唯一命名空间，子 Station 的 toolkit 不能与父或兄弟重名
- 本地约束：`Station::from_repo` 构建时以本地 toolkit 名填充；跨进程部分由部署保证（见"工具名整树唯一"）
- 内置注册表占用 `filesystem` 命名空间：配置显式声明 `toolkits["filesystem"]` 才注册内置 read 工具实现；未声明则不注册

## 递归平铺语义

`Station::tools(filter, ancestors)` / `Station::mcps(filter, ancestors)`：`filter: Option<&HashSet<String>>`（toolkit 名白名单）；`ancestors` 为根到当前父节点的 station_id 链。

| filter | 语义 |
|--------|------|
| None | 返回全部 |
| Some(空集) | 返回空 |
| Some(白名单) | 只返回命中白名单的 toolkit 的元数据 |

- 本地：遍历本地 toolkit，白名单外的跳过，命中的收集 `configured_tools()`/`configured_mcps()`
- 直接子（tools）：经 `StationClient.list_tools(filter, ancestors + 本站 station_id)` 实时拉取；成功时更新 tool_routes 路由缓存（快照语义：先清该子旧记录再逐个插入，跨进程冲突保留先到者 + warn），查询失败 → warn 跳过、旧缓存保留
- 直接子（mcps）：经 `StationClient.list_mcps(filter, ancestors + 本站 station_id)` 实时拉取；不建缓存表（MCP 与 Tool 嵌套关系，实现 MCP 时再设计）
- 自环检测：任一层发现自己的 `station_id` 已在 `ancestors` 中，返回 `StationCycle` 错误
- 聚合为空 → 请求不携带 tools 字段（兼容无工具场景）

## 工具名整树唯一

- 工具名在整棵 Station 树（本地所有 toolkit + 所有子 Station 递归）上全局唯一
- **本地硬约束**：`Station::from_repo` 构建时对每个 toolkit 的工具做去重校验，重名（含与内置实现同名）启动失败返回"工具名冲突"
- **跨进程冲突处理**：`tools()` 拉取子 Station 合并进 tool_routes 缓存时发现工具名已存在（与本地或先到子重名）→ warn 日志，保留先到者，后到者剔除（不进缓存表、不进返回列表）

## 远程工具路由缓存（tool_routes）

- **缓存位置**：`Station.tool_routes: DashMap<String, String>`（工具名 → 直接子 station_id）；仅用于 `call_tool` 路由，不用于元数据查询；本地工具不走缓存表（`call_tool` 先查本地实现表）
- **更新时机**：`tools()` 拉取子 Station 成功时更新（快照语义——先清该子旧路由记录，再逐个插入；该子拉取失败保留旧缓存）
- **冲突处理**：合并时工具名已存在（与本地或先到子重名）→ warn 日志，保留先到者，后到者不进缓存表、不进返回列表
- **MCP 不建缓存表**：MCP 与 Tool 为嵌套关系，缓存设计留待实现 MCP 时再设计；`mcps()` 实时拉取平铺返回

## 子 Station HTTP 协议

子 Station 只能通过 HTTP 与父 Station 通信，不可本地调用。station 自身服务端监听公共配置 `agent.station_host` / `agent.station_port`，认证使用 `security.api_key`（`X-Api-Key` header）。

所有接口统一使用 `ApiResponse<T>`：

- 成功时 `data` 直接是结果，不再包一层；
- `list_tools` / `list_mcps` 业务错误返回非 200；
- `call_tool` 的工具调用失败返回 HTTP 200 + `success=false`；
- 自环检测返回 400；
- 认证失败返回 401。

### list_tools

`POST /station/tools`

查询子 Station 平铺后的工具元数据（可选 toolkit 白名单过滤）。

```jsonc
// 请求
{
  "filter": ["toolkit-a"],   // 可选；None = 全部，空数组 = 空
  "ancestors": ["root", "parent"]  // 根到当前父节点；缺省为空数组
}
// 成功响应 200
{
  "success": true,
  "data": [ { "name": "read", "description": "...", "parameters": { } } ],
  "error": null
}
```

### list_mcps

`POST /station/mcps`

查询子 Station 平铺后的 MCP 元数据（可选 toolkit 白名单过滤）。

```jsonc
// 请求
{
  "filter": ["toolkit-a"],
  "ancestors": ["root", "parent"]
}
// 成功响应 200
{
  "success": true,
  "data": [ { "name": "mcp1", "description": "..." } ],
  "error": null
}
```

### call_tool

`POST /station/call-tool`

调用子 Station 上的工具。

```jsonc
// 请求
{
  "tool_name": "read",
  "parameters": { "path": "/tmp/a.txt" },
  "ancestors": ["root", "parent"]
}
// 成功响应 200
{
  "success": true,
  "data": "文件内容...",
  "error": null
}
// 工具执行失败：HTTP 200 + success=false
```

任意接口若发现自己的 `station_id` 已在 `ancestors` 中，返回 400 + `ApiResponse::error("station cycle detected")`。

## ContextConfig.toolkits 白名单与 Session 对应关系

- **ContextConfig.toolkits**：`Option<Arc<HashSet<String>>>`，会话启用的 toolkit 名白名单，替代原 stations 字段；`None`/空集 = 无工具
- 三层继承合并（全局默认 ← agent 默认 ← role 覆盖）后的 `EffectiveContextConfig.toolkits: HashSet<String>` 即为会话工具白名单
- `Nexus::tools_for_session(session)`：读 session 的 (agent_id, role_name) 合并 context 配置 → `toolkits` 为空直接返回空 → `Station::get().tools(Some(&cfg.toolkits), &[])` 平铺查询
- `Nexus::execute_tool_call(call)`：`Station::get().call_tool(name, args, &[])`，错误返回 `{ "error": "..." }` JSON
- 工具名整树唯一与 toolkit 白名单共同保证：任一工具在整棵 Station 树唯一，白名单按 toolkit 名粗粒度放行

## 启动顺序

ConfigManager → Station → Nexus：

```
ConfigManager::new()   // 加载 KISSBOT_CONFIG agent 段，引导 nexus.json/station.json，注册单例
Station::new()         // 读 station.json 构建全局 Station，注册单例
Nexus::new()           // 装配运行时组件，注册单例；run() 中执行连接与启动动作
StationHttpServer::start()  // 后台启动 station 对外 HTTP 服务（agent.station_host:agent.station_port）
```
