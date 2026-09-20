# kissbot-mcp 技术规格

> 说明 kissbot-mcp 的实现范围、消息结构、方法字段、kissbot 扩展、HTTP 传输与客户端/服务端 API 约定。
> 协议依据 MCP 2026-07-28；组件设计见 [kissbot-mcp 组件设计](../design/components-design/kissbot-mcp.md)。

## 实现范围

- 协议版本：`PROTOCOL_VERSION = "2026-07-28"`
- 实现的方法：

| 方法 | 常量 | 说明 |
|------|------|------|
| `server/discover` | `METHOD_DISCOVER` | 查询服务端支持的协议版本、能力与身份 |
| `tools/list` | `METHOD_TOOLS_LIST` | 查询工具定义（支持 kissbot 分组过滤） |
| `tools/call` | `METHOD_TOOLS_CALL` | 调用工具 |

`SUPPORTED_METHODS` 为上述三个方法名的数组（供文档与断言使用）。

## JSON-RPC 消息

消息遵循 JSON-RPC 2.0，并按 MCP 规范作两点约束：请求 id 不得为 null；结果响应必须携带 `resultType`。

```jsonc
// 请求
{ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { } }

// 结果响应
{ "jsonrpc": "2.0", "id": 1, "result": { "resultType": "complete" } }

// 错误响应
{ "jsonrpc": "2.0", "id": 1, "error": { "code": -32602, "message": "…", "data": { } } }
```

| 项 | 约定 |
|----|------|
| `id` | 字符串或整数，不得为 null；无法从请求体读取时错误响应省略该字段 |
| `resultType` | `complete`（请求已完成）或 `input_required`（需要补充输入）；本实现只产出 `complete`，缺省视为 `complete`，未知值由客户端按非法处理 |
| `error.data` | 可选；协议版本协商失败时携带 `supportedVersions` |
| 响应身份 | 结果响应中省略 `error`，错误响应中省略 `result` |

## `_meta`

### 请求 `_meta`（`params._meta`）

| 键 | 必填 | 类型 | 说明 |
|----|------|------|------|
| `io.modelcontextprotocol/protocolVersion` | 是 | string | 本请求使用的协议版本 |
| `io.modelcontextprotocol/clientInfo` | 否 | object | 客户端身份（`name` / `version`） |
| `io.modelcontextprotocol/clientCapabilities` | 是 | object | 客户端能力（可为空对象） |
| `kissbot/ancestors` | 否 | string[] | station 级联祖先链（根 → 当前父节点的 station_id 列表；空列表不序列化） |

### 响应 `_meta`

| 键 | 类型 | 说明 |
|----|------|------|
| `io.modelcontextprotocol/serverInfo` | object | 服务端身份（`name` / `version`），由 `McpServer` 统一填充 |
| `kissbot/group` | string[] | 本次返回工具所属的组名集合（仅 `tools/list`） |

`io.modelcontextprotocol/*` 为 MCP 规范保留键；`kissbot/` 为 kissbot 扩展前缀（非规范保留前缀，键名符合规范的 `_meta` 命名规则）。

## server/discover

请求参数：仅 `_meta`。

响应 `result`：

| 字段 | 类型 | 说明 |
|------|------|------|
| `resultType` | string | 固定 `complete` |
| `supportedVersions` | string[] | 服务端支持的协议版本列表 |
| `capabilities` | object | 能力集合：`tools`（`listChanged` 固定为 `false`）与 `kissbot`（kissbot 扩展能力，见下）；未建模的其他能力项（如 `resources` / `prompts`）解析时忽略、不保留 |
| `instructions` | string? | 自然语言使用说明（服务端未显式配置时由业务处理器给出） |
| `_meta` | object | 含 `serverInfo` |

`capabilities.kissbot`（kissbot 扩展能力声明，第三方通用 MCP 服务端没有该段）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `group` | bool | 支持 `tools/list` 的 `group` 过滤与结果 `kissbot/group` 回带 |
| `ancestors` | bool | 支持请求 `_meta["kissbot/ancestors"]` 级联祖先链与自环检测 |

`DiscoverResult` 提供 `supports_group()` / `supports_ancestors()` / `supports_kissbot()` 读取该声明；
`DiscoverResult::with_kissbot_extensions()` 声明完整扩展，`KissbotCapability::full()` 为其值。
客户端据此在首次接触时一次性判定对端是否支持 kissbot 扩展，无需逐请求试探。

```jsonc
{
  "jsonrpc": "2.0", "id": "discover-1",
  "result": {
    "resultType": "complete",
    "supportedVersions": ["2026-07-28"],
    "capabilities": {
      "tools": { "listChanged": false },
      "kissbot": { "group": true, "ancestors": true }
    },
    "instructions": "…",
    "_meta": { "io.modelcontextprotocol/serverInfo": { "name": "…", "version": "…" } }
  }
}
```

## tools/list

请求参数：

| 字段 | 类型 | 说明 |
|------|------|------|
| `cursor` | string? | 分页游标（本实现不产出 `nextCursor`） |
| `group` | string[]? | kissbot 扩展：组白名单。缺省 = 全部；空数组 = 空结果；非空 = 只返回命中组的工具 |
| `_meta` | object | 请求元数据 |

响应 `result`：

| 字段 | 类型 | 说明 |
|------|------|------|
| `resultType` | string | 固定 `complete` |
| `tools` | Tool[] | 工具定义列表 |
| `nextCursor` | string? | 本实现不产出 |
| `_meta` | object | 含 `serverInfo` 与 `kissbot/group` |

`Tool`：

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | string | 工具名（同一服务端内唯一） |
| `title` | string? | 展示名（可选，缺省不序列化） |
| `description` | string | 工具描述 |
| `inputSchema` | object | 参数 JSON Schema |
| `outputSchema` | object? | 输出 JSON Schema（可选，缺省不序列化） |
| `annotations` | object? | 工具行为注解（可选，缺省不序列化） |

```jsonc
{
  "jsonrpc": "2.0", "id": 1,
  "params": { "group": ["filesystem"], "_meta": { } },
  "result": {
    "resultType": "complete",
    "tools": [
      { "name": "read", "description": "…",
        "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } }
    ],
    "_meta": {
      "io.modelcontextprotocol/serverInfo": { "name": "kissbot-station", "version": "0.1.0" },
      "kissbot/group": ["filesystem"]
    }
  }
}
```

## tools/call

请求参数：

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | string | 工具名 |
| `arguments` | object | 调用参数 |
| `_meta` | object | 请求元数据 |

响应 `result`：

| 字段 | 类型 | 说明 |
|------|------|------|
| `resultType` | string | 固定 `complete` |
| `content` | Content[] | 内容块数组（规范必填，可含多个块） |
| `structuredContent` | any? | 结构化结果（规范可选；为 null 时不序列化） |
| `isError` | bool | 工具执行是否失败（缺省 `false`） |
| `_meta` | object | 含 `serverInfo` |

`Content` 是按 `type` 区分的联合类型（`type` 即枚举变体，内部标签序列化），覆盖规范全部 ContentBlock 变体：

| 变体 | 字段 |
|------|------|
| `text` | `text`；可选 `annotations` / `_meta` |
| `image` | `data`（base64）、`mimeType`；可选 `annotations` / `_meta` |
| `audio` | `data`（base64）、`mimeType`；可选 `annotations` / `_meta` |
| `resource_link` | `uri`、`name`；可选 `title` / `description` / `mimeType` / `icons` / `annotations` / `_meta` |
| `resource` | `resource`（嵌入资源内容：文本 `text` 或二进制 `blob`，均带 `uri`、可选 `mimeType` / `_meta`）；可选 `annotations` / `_meta` |

辅助类型：`Annotations`（`audience` / `priority` / `lastModified`）、`Role`、`Icon`（`src` / `mimeType` / `sizes` / `theme`）、`ResourceContents`（`Text` / `Blob`，按有无 `text` / `blob` 区分）。
读取辅助：`Content::as_text()`、`Content::type_name()`、`CallToolResult::text_content()`。
各变体类型全覆盖；不在规范内的 `type` 值会使反序列化失败。

`content` 由结果生产方（工具实现）决定，MCP 服务只原样回传：不解释块类型、不把结构化数据合成文本、不改写数组。规范中「返回结构化内容时应同时给出序列化 JSON 的文本块」是**对工具**的兼容性建议（针对 `structuredContent`），由工具决定是否在 `content` 里附带这个镜像块，与服务端无关。

结果构造约定（`CallToolResult`）：

| 构造 | `content` | `structuredContent` | `isError` |
|------|-----------|---------------------|-----------|
| `text(s)` | 单个文本块 `s`（原样） | 省略（null） | false |
| `error(code, message)` | 文本块 `message` | 固定错误结构 `{ code, message, source }` | true |

其他形态由生产方按 `CallToolResult::default()` 显式拼装（内容块数组与 `structuredContent` 都由工具给出）。

### 固定错误结构与来源

`isError=true` 时 `structuredContent` 为固定结构：

| 字段 | 类型 | 说明 |
|------|------|------|
| `code` | u32 | 错误码（kissbot 工具错误码由具体层定义） |
| `message` | string | 错误文案 |
| `source` | string | 错误来源标识；kissbot 实现固定为 `kissbot`（常量 `ERROR_SOURCE_KISSBOT`） |

消费方对 `isError` 结果的判定规则：

- 解析失败、结构不符、或 `source` 非 kissbot → 原样透传，不按错误码解释；
- `source` 为 kissbot → 可按 `code` 处理已知语义（如级联自环），未识别的错误码同样透传。

读取辅助：`error_detail()` 解析固定错误结构（不校验来源）；`kissbot_error()` 仅在来源为 kissbot 时给出；`error_message()` 优先取错误结构文案，无则回退首个文本块。

辅助读取：`text_content()` 取首个文本块。

```jsonc
// 文本结果（由 text() 构造）
{ "result": { "resultType": "complete", "content": [{ "type": "text", "text": "文件内容" }],
              "isError": false, "_meta": { } } }
// 结构化结果（镜像块由工具自己放进 content）
{ "result": { "resultType": "complete",
              "content": [{ "type": "text", "text": "{\"temp\":25}" },
                          { "type": "image", "data": "eHl6", "mimeType": "image/png" }],
              "structuredContent": { "temp": 25 }, "isError": false, "_meta": { } } }
// 失败（HTTP 200）
{ "result": { "resultType": "complete", "content": [{ "type": "text", "text": "工具不存在: x" }],
              "structuredContent": { "code": 1, "message": "工具不存在: x", "source": "kissbot" },
              "isError": true, "_meta": { } } }
```

## 错误码

| 码 | 常量 | 名称 | 触发场景 |
|----|------|------|----------|
| -32700 | `PARSE_ERROR` | ParseError | 请求体不是合法 JSON |
| -32600 | `INVALID_REQUEST` | InvalidRequest | 请求结构非法（jsonrpc 版本不支持、缺少 method、访问来源被拒） |
| -32601 | `METHOD_NOT_FOUND` | MethodNotFound | 方法不存在 |
| -32602 | `INVALID_PARAMS` | InvalidParams | 参数非法（缺少必填 `_meta`、缺少或不匹配请求头、`tools/call` 缺少调用目标、参数反序列化失败） |
| -32603 | `INTERNAL_ERROR` | InternalError | 服务端内部错误（结果序列化失败等） |
| -32020 | `HEADER_MISMATCH` | HeaderMismatch | 请求头与请求体不一致 |
| -32021 | `MISSING_REQUIRED_CLIENT_CAPABILITY` | MissingRequiredClientCapability | 缺少服务端要求的客户端能力（保留） |
| -32022 | `UNSUPPORTED_PROTOCOL_VERSION` | UnsupportedProtocolVersion | 协议版本不在服务端支持列表内；`data.supportedVersions` 携带支持列表 |
| 1001 | `STATION_CYCLE` | — | station 级联自环（kissbot 应用自定义码，位于 JSON-RPC 保留区间之外） |

## HTTP 传输（Streamable HTTP 最小实现）

- 单一 POST 端点承载全部请求，路径默认 `/mcp`，可由 `McpHttpConfig.endpoint_path` 覆盖
- 每个请求一个 POST，响应为单个 JSON 对象（`application/json`）
- 不实现 SSE 流式响应、GET 端点与协议级会话

请求头：

| 头 | 必填 | 说明 |
|----|------|------|
| `Content-Type` | 是 | `application/json` |
| `Accept` | 客户端携带 | `application/json, text/event-stream` |
| `MCP-Protocol-Version` | 是 | 必须与请求体 `_meta` 的协议版本一致 |
| `Mcp-Method` | 是 | 必须与请求 `method` 一致 |
| `Mcp-Name` | `tools/call` 必填 | 必须与 `params.name` 一致 |
| `Origin` | 否 | 请求方来源；命中 `allowed_origins` 白名单或本机回环来源时放行，否则 403 |
| `X-Api-Key` | 视调用方 | kissbot-mcp 不自带认证；station 端点由其上的认证中间件使用 |

状态码映射：

| 情况 | 状态码 | 响应体 |
|------|--------|--------|
| 请求体非法 JSON | 400 | 错误响应 `-32700`（省略 id） |
| 缺少 `MCP-Protocol-Version` 头、缺少 `_meta` 必填项、`tools/call` 缺少 `params.name` 或 `Mcp-Name` 头 | 400 | 错误响应 `-32602` |
| 协议版本头与请求体不一致、`Mcp-Method` 或 `Mcp-Name` 与请求体不一致 | 400 | 错误响应 `-32020` |
| 协议版本不受支持 | 400 | 错误响应 `-32022`（含 `data.supportedVersions`） |
| 访问来源不被允许 | 403 | 错误响应 `-32600` |
| 方法不存在 | 404 | 错误响应 `-32601` |
| 其余（含工具级失败、级联自环等业务错误） | 200 | 正常结果或错误响应 |

## 服务端 API

- `McpHandler`（trait，由使用方实现）：
  - `discover() -> DiscoverResult`：服务端能力与使用说明（协议版本与服务端身份由 `McpServer` 统一填充）
  - `list_tools(params: ToolsListParams) -> Result<ToolsListResult, McpError>`
  - `call_tool(params: ToolsCallParams) -> Result<CallToolResult, McpError>`
- `McpServer::new(handler: Arc<H>, server_info: Implementation)`：持有业务处理器与身份
  - `with_instructions(...)`：设置 `server/discover` 的使用说明
  - `supported_versions()`：支持列表（本实现为 `["2026-07-28"]`）
  - `handle(request: JsonRpcRequest) -> JsonRpcResponse`：校验 JSON-RPC 版本 → 按 `method` 分发 → 解析参数 → 校验协议版本在支持列表内 → 调用处理器 → 统一填充 `resultType = complete` 与响应 `serverInfo`；未知方法返回 `-32601`
- `router(server, config) -> Router`：构建单端点 Router，不含认证（认证由调用方以 Layer 叠加）
  - `McpHttpConfig { endpoint_path: Arc<String>, allowed_origins: Vec<String> }`，默认 `endpoint_path = "/mcp"`、`allowed_origins` 为空
  - `allowed_origins` 为空时只放行无 `Origin` 或本机回环来源（127.0.0.1 / localhost / [::1] 的 http/https）

## 客户端 API

- `McpClient::new(endpoint: &str, timeout_secs: u64, api_key: Option<&str>)`：`endpoint` 为 MCP 端点完整 URL；`api_key` 非空时以 `X-Api-Key` 头发送
- `with_client_info(implementation)`：覆盖自报身份（默认 `name = "kissbot-mcp-client"`，`version` 取包版本）
- `discover() -> DiscoverResult`：返回的支持版本、能力与身份；`DiscoverResult::supports_group()` / `supports_ancestors()` / `supports_kissbot()` 用于判定对端 kissbot 扩展能力
- `list_tools(group: Option<&[String]>, ancestors: &[String]) -> ToolsListResult`
- `call_tool(name: &str, arguments: Value, ancestors: &[String]) -> CallToolResult`
- 每个请求自动填充 `_meta`（协议版本、`clientInfo`、空 `clientCapabilities`、非空时的 `kissbot/ancestors`）与请求头（`Accept`、`MCP-Protocol-Version`、`Mcp-Method`、`tools/call` 时的 `Mcp-Name`）；请求 id 自增
- 错误类型 `McpClientError`：

| 变体 | 说明 |
|------|------|
| `Transport(reqwest::Error)` | 传输失败（连接、超时等） |
| `Http { status, message }` | 响应不是可解析的 JSON-RPC 错误体时的兜底（携带状态码与响应文本） |
| `Remote { code, message }` | 远端返回 JSON-RPC 错误 |
| `Decode(serde_json::Error)` | 结果反序列化为目标类型失败 |
