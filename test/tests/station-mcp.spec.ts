import { test, expect } from '@playwright/test';
import { resetWorkspace, startAgent, stopAgent, waitForPort } from './helpers/server';
import { mcpCall, MCP_PROTOCOL_VERSION, STATION_API_KEY } from './helpers/mcp';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';

const MCP_ENDPOINT = 'http://127.0.0.1:9100/mcp';
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let agent: ChildProcess;

test.describe.serial('station MCP 协议测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    agent = startAgent(WORKSPACE);
    await waitForPort(9100, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopAgent(agent);
  });

  test('TC-01: 无 key / 错误 key → 401', async ({ request }) => {
    const noKey = await mcpCall(request, MCP_ENDPOINT, { method: 'tools/list', key: '' });
    expect(noKey.status()).toBe(401);

    const wrongKey = await mcpCall(request, MCP_ENDPOINT, { method: 'tools/list', key: 'wrong' });
    expect(wrongKey.status()).toBe(401);
  });

  test('TC-02: server/discover 返回协议版本、tools 能力与服务端身份', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, { method: 'server/discover' });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.resultType).toBe('complete');
    expect(body.result.supportedVersions).toContain(MCP_PROTOCOL_VERSION);
    expect(body.result.capabilities.tools).toBeDefined();
    expect(body.result._meta['io.modelcontextprotocol/serverInfo'].name).toBe('kissbot-station');
  });

  test('TC-03: tools/list 按 group 过滤，group 随工具携带', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/list',
      params: { group: ['filesystem'] },
    });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.resultType).toBe('complete');
    expect(Array.isArray(body.result.tools)).toBe(true);
    expect(body.result.tools.length).toBeGreaterThanOrEqual(1);
    expect(body.result.tools[0].name).toBe('read');
    expect(body.result.tools[0].inputSchema.type).toBe('object');
    expect(body.result.tools[0]._meta['kissbot/group']).toBe('filesystem');
  });

  test('TC-04: tools/list 不带 group 返回全部工具', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, { method: 'tools/list' });
    const body = await resp.json();
    expect(body.result.tools.some((t: any) => t.name === 'read')).toBe(true);
  });

  test('TC-05: tools/list group 白名单为空 → 空结果', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, { method: 'tools/list', params: { group: [] } });
    const body = await resp.json();
    expect(body.result.tools).toEqual([]);
  });

  test('TC-06: tools/call 成功调用本地 read', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/call',
      name: 'read',
      params: { name: 'read', arguments: { path: 'config.json' } },
    });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.isError).toBe(false);
    // read 是文本结果：文本块即文件内容，不设 structuredContent
    expect(body.result.content[0].text).toContain('"agent"');
    expect(body.result.structuredContent).toBeUndefined();
  });

  test('TC-07: tools/call 工具不存在 → isError=true 且错误码在 structuredContent', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/call',
      name: 'missing-tool',
      params: { name: 'missing-tool', arguments: {} },
    });
    // 工具调用失败不是 HTTP 失败：错误随 CallToolResult 返回（TERR_TOOL_NOT_FOUND = 1）
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.isError).toBe(true);
    expect(body.result.structuredContent.code).toBe(1);
    expect(body.result.content[0].text).toContain('工具不存在');
  });

  test('TC-08: 祖先链含自身 → JSON-RPC 错误 STATION_CYCLE', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/list',
      ancestors: ['parent', 'station-a'],
    });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.error.code).toBe(1001);
    expect(body.result).toBeUndefined();
  });

  test('TC-09: MCP-Protocol-Version 头与请求体不一致 → 400 + HeaderMismatch', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/list',
      headerProtocolVersion: '1999-01-01',
    });
    expect(resp.status()).toBe(400);
    const body = await resp.json();
    expect(body.error.code).toBe(-32020);
  });

  test('TC-10: 缺少 MCP-Protocol-Version 头 → 400', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/list',
      omitProtocolVersionHeader: true,
    });
    expect(resp.status()).toBe(400);
    const body = await resp.json();
    expect(body.error.code).toBe(-32602);
  });

  test('TC-11: 未知方法 → 404 + Method not found', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, { method: 'tools/nope' });
    expect(resp.status()).toBe(404);
    const body = await resp.json();
    expect(body.error.code).toBe(-32601);
  });

  test('TC-12: tools/call 缺少 Mcp-Name 头 → 400', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/call',
      params: { name: 'read', arguments: {} },
    });
    expect(resp.status()).toBe(400);
    const body = await resp.json();
    expect(body.error.code).toBe(-32602);
    expect(body.error.message).toContain('Mcp-Name');
  });

  test('TC-13: 不支持的协议版本 → 400 + UnsupportedProtocolVersion', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/list',
      headerProtocolVersion: '1999-01-01',
      bodyProtocolVersion: '1999-01-01',
    });
    expect(resp.status()).toBe(400);
    const body = await resp.json();
    expect(body.error.code).toBe(-32022);
    expect(body.error.data.supportedVersions).toContain(MCP_PROTOCOL_VERSION);
  });

  test('TC-14: api key 正确才可访问（回归）', async ({ request }) => {
    const resp = await mcpCall(request, MCP_ENDPOINT, {
      method: 'tools/list',
      key: STATION_API_KEY,
    });
    expect(resp.status()).toBe(200);
  });
});
