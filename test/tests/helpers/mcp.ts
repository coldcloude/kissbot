import { APIRequestContext, APIResponse } from '@playwright/test';

/** 本实现所遵循的 MCP 协议版本 */
export const MCP_PROTOCOL_VERSION = '2026-07-28';
/** 测试环境 security.api_key */
export const STATION_API_KEY = 'user-key-456';

export interface McpCallOptions {
  /** JSON-RPC 方法（server/discover / tools/list / tools/call） */
  method: string;
  /** 业务参数（`_meta` 由 helper 统一补齐） */
  params?: Record<string, unknown>;
  /** tools/call 的工具名（同时写入 Mcp-Name 头） */
  name?: string;
  /** station 级联祖先链（写入 `_meta["kissbot/ancestors"]`） */
  ancestors?: string[];
  /** tools/list 组白名单（写入 `_meta["kissbot/groups"]`；空数组 = 空结果） */
  groups?: string[];
  /** 覆盖 api key；空字符串表示不携带 X-Api-Key */
  key?: string;
  id?: number;
  /** 请求头 MCP-Protocol-Version；用于构造头部与请求体不一致的用例 */
  headerProtocolVersion?: string;
  /** 请求体 `_meta` 协议版本 */
  bodyProtocolVersion?: string;
  /** 不发送 MCP-Protocol-Version 请求头 */
  omitProtocolVersionHeader?: boolean;
}

/** 构造 MCP 请求头（含 Streamable HTTP 要求的元数据头） */
export function mcpHeaders(opts: McpCallOptions): Record<string, string> {
  const { method, name, key = STATION_API_KEY } = opts;
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    'Accept': 'application/json, text/event-stream',
    'Mcp-Method': method,
  };
  if (!opts.omitProtocolVersionHeader) {
    headers['MCP-Protocol-Version'] = opts.headerProtocolVersion ?? MCP_PROTOCOL_VERSION;
  }
  if (name) headers['Mcp-Name'] = name;
  if (key) headers['X-Api-Key'] = key;
  return headers;
}

/** 构造 MCP 请求体（JSON-RPC 2.0 + `_meta`） */
export function mcpBody(opts: McpCallOptions): Record<string, unknown> {
  return {
    jsonrpc: '2.0',
    id: opts.id ?? 1,
    method: opts.method,
    params: {
      ...(opts.params ?? {}),
      _meta: {
        'io.modelcontextprotocol/protocolVersion': opts.bodyProtocolVersion ?? MCP_PROTOCOL_VERSION,
        'io.modelcontextprotocol/clientInfo': { name: 'playwright', version: '1.0.0' },
        'io.modelcontextprotocol/clientCapabilities': {},
        ...(opts.ancestors && opts.ancestors.length > 0 ? { 'kissbot/ancestors': opts.ancestors } : {}),
        ...(opts.groups !== undefined ? { 'kissbot/groups': opts.groups } : {}),
      },
    },
  };
}

/** 向 /mcp 端点发送一次 MCP 请求 */
export async function mcpCall(
  request: APIRequestContext,
  endpoint: string,
  opts: McpCallOptions,
): Promise<APIResponse> {
  return request.post(endpoint, { headers: mcpHeaders(opts), data: mcpBody(opts) });
}
