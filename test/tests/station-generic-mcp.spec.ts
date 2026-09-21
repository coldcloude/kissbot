import { test, expect } from '@playwright/test';
import { startAgent, stopAgent, waitForPort } from './helpers/server';
import { mcpCall } from './helpers/mcp';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { cpSync, mkdtempSync, rmSync, readFileSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';
import { createServer, Server } from 'node:http';
import { AddressInfo } from 'node:net';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = join(__dirname, '..', '..');
const TEMPLATE = join(REPO_ROOT, 'test', 'workspace-template');

const MGMT = 9094;
const STATION = 9104;
const ENDPOINT = `http://127.0.0.1:${STATION}/mcp`;

let dir: string;
let agent: ChildProcess;
let plain: Server;
let plainUrl: string;

/**
 * 启动一个「通用 MCP 服务端」（非 kissbot）：
 * - server/discover 只声明 tools 能力，不声明 kissbot 扩展
 * - tools/list 返回固定工具，且工具定义不携带 group
 * - tools/call 返回结构化结果
 */
async function startPlainMcpServer(): Promise<{ server: Server; url: string }> {
  const server = createServer((req, res) => {
    let body = '';
    req.on('data', (c) => { body += c; });
    req.on('end', () => {
      const rpc = JSON.parse(body || '{}');
      const reply = (result: unknown) => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ jsonrpc: '2.0', id: rpc.id, result }));
      };
      switch (rpc.method) {
        case 'server/discover':
          reply({
            resultType: 'complete',
            supportedVersions: ['2026-07-28'],
            capabilities: {
              tools: { listChanged: false },
              // 第三方声明的本实现未建模能力：应被忽略而不影响解析
              resources: {},
              prompts: {},
            },
            _meta: { 'io.modelcontextprotocol/serverInfo': { name: 'plain-mcp', version: '1.0.0' } },
          });
          break;
        case 'tools/list':
          reply({
            resultType: 'complete',
            tools: [{ name: 'plain-tool', description: '第三方工具', inputSchema: { type: 'object' } }],
          });
          break;
        case 'tools/call':
          reply({
            resultType: 'complete',
            content: [
              { type: 'text', text: 'plain-ok' },
              // 非文本块：经 Station 代理转发时应逐字保留
              { type: 'image', data: 'eHl6', mimeType: 'image/png' },
            ],
            structuredContent: { ok: true, name: rpc.params?.name },
            isError: false,
          });
          break;
        default:
          res.writeHead(404, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ jsonrpc: '2.0', id: rpc.id, error: { code: -32601, message: 'method not found' } }));
      }
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return { server, url: `http://127.0.0.1:${port}` };
}

test.describe.serial('station 对接第三方 MCP 服务端（非 Station）', () => {

  test.beforeAll(async () => {
    const started = await startPlainMcpServer();
    plain = started.server;
    plainUrl = started.url;

    dir = mkdtempSync(join(tmpdir(), 'kissbot-station-plain-'));
    cpSync(TEMPLATE, dir, { recursive: true });
    const cfg = JSON.parse(readFileSync(join(dir, 'config.json'), 'utf8'));
    cfg.agent.mgmt_port = MGMT;
    cfg.agent.station_port = STATION;
    writeFileSync(join(dir, 'config.json'), JSON.stringify(cfg, null, 2) + '\n');
    // 本级无 group，只挂一个第三方 MCP 服务端，其工具默认 group 由 station_id 派生（[plain]）
    writeFileSync(join(dir, 'agent-data', 'station.json'), JSON.stringify({
      station_id: 'station-a',
      groups: [],
      sub_stations: {
        plain: {
          station_id: 'plain',
          base_url: plainUrl,
          timeout_secs: 5,
        },
      },
    }, null, 2) + '\n');

    agent = startAgent(dir);
    await waitForPort(STATION, '127.0.0.1', 30000);
  });

  test.afterAll(async () => {
    stopAgent(agent);
    await new Promise<void>((resolve) => plain.close(() => resolve()));
    if (dir) rmSync(dir, { recursive: true, force: true });
  });

  test('第三方工具归入子站默认 group', async ({ request }) => {
    const resp = await mcpCall(request, ENDPOINT, { method: 'tools/list' });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.tools.map((t: any) => t.name)).toContain('plain-tool');
    // 对端工具未携带 group → 归入子站默认 group（由 station_id 派生）
    expect(body.result.tools[0]._meta['kissbot/group']).toBe('[plain]');
  });

  test('未知的 _meta 扩展键被忽略，第三方工具照常返回', async ({ request }) => {
    const resp = await mcpCall(request, ENDPOINT, {
      method: 'tools/list',
      params: { _meta: { 'kissbot/groups': ['other'] } },
    });
    const body = await resp.json();
    expect(body.result.tools.map((t: any) => t.name)).toContain('plain-tool');
  });

  test('可经 MCP 调用第三方工具', async ({ request }) => {
    // 先拉取一次建立路由缓存
    await mcpCall(request, ENDPOINT, { method: 'tools/list' });
    const resp = await mcpCall(request, ENDPOINT, {
      method: 'tools/call',
      name: 'plain-tool',
      params: { name: 'plain-tool', arguments: {} },
    });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.isError).toBe(false);
    expect(body.result.structuredContent.ok).toBe(true);
    expect(body.result.structuredContent.name).toBe('plain-tool');
    // 非文本内容块经 Station 代理转发后逐字保留
    const image = body.result.content.find((c: any) => c.type === 'image');
    expect(image).toBeDefined();
    expect(image.data).toBe('eHl6');
    expect(image.mimeType).toBe('image/png');
  });

  test('本级 discover 声明 kissbot 级联扩展（对端可据此判定）', async ({ request }) => {
    const resp = await mcpCall(request, ENDPOINT, { method: 'server/discover' });
    const body = await resp.json();
    expect(body.result.capabilities.kissbot.ancestors).toBe(true);
  });
});
