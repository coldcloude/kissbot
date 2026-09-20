import { test, expect } from '@playwright/test';
import { startAgent, stopAgent, waitForPort } from './helpers/server';
import { mcpCall } from './helpers/mcp';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { cpSync, mkdtempSync, rmSync, readFileSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = join(__dirname, '..', '..');
const TEMPLATE = join(REPO_ROOT, 'test', 'workspace-template');

const MGMT_A = 9092;
const STATION_A = 9102;
const MGMT_B = 9093;
const STATION_B = 9103;

const ENDPOINT_A = `http://127.0.0.1:${STATION_A}/mcp`;
const ENDPOINT_B = `http://127.0.0.1:${STATION_B}/mcp`;

let dirA: string;
let dirB: string;
let agentA: ChildProcess;
let agentB: ChildProcess;

function writeJson(file: string, value: unknown): void {
  writeFileSync(file, JSON.stringify(value, null, 2) + '\n');
}

test.describe.serial('station 级联端到端测试（A 经 MCP 调用子站 B）', () => {

  test.beforeAll(async () => {
    dirA = mkdtempSync(join(tmpdir(), 'kissbot-station-a-'));
    dirB = mkdtempSync(join(tmpdir(), 'kissbot-station-b-'));
    cpSync(TEMPLATE, dirA, { recursive: true });
    cpSync(TEMPLATE, dirB, { recursive: true });

    // A：独立端口，本级无 group，仅配子站 B
    const cfgA = JSON.parse(readFileSync(join(dirA, 'config.json'), 'utf8'));
    cfgA.agent.mgmt_port = MGMT_A;
    cfgA.agent.station_port = STATION_A;
    writeJson(join(dirA, 'config.json'), cfgA);
    writeJson(join(dirA, 'agent-data', 'station.json'), {
      station_id: 'station-a',
      groups: {},
      sub_stations: {
        'station-b': {
          station_id: 'station-b',
          base_url: `http://127.0.0.1:${STATION_B}`,
          timeout_secs: 5,
          default_group: 'remote-b',
        },
      },
    });

    // B：独立端口，本级 filesystem group
    const cfgB = JSON.parse(readFileSync(join(dirB, 'config.json'), 'utf8'));
    cfgB.agent.mgmt_port = MGMT_B;
    cfgB.agent.station_port = STATION_B;
    writeJson(join(dirB, 'config.json'), cfgB);
    writeJson(join(dirB, 'agent-data', 'station.json'), {
      station_id: 'station-b',
      groups: { filesystem: {} },
      sub_stations: {},
    });

    agentB = startAgent(dirB);
    await waitForPort(STATION_B, '127.0.0.1', 30000);
    agentA = startAgent(dirA);
    await waitForPort(STATION_A, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopAgent(agentA);
    stopAgent(agentB);
    if (dirA) rmSync(dirA, { recursive: true, force: true });
    if (dirB) rmSync(dirB, { recursive: true, force: true });
  });

  test('子站 B 自身可列出其 filesystem group 的 read 工具', async ({ request }) => {
    const resp = await mcpCall(request, ENDPOINT_B, { method: 'tools/list' });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.tools.some((t: any) => t.name === 'read')).toBe(true);
    expect(body.result._meta['kissbot/group']).toContain('filesystem');
  });

  test('A 能平铺到子站 B 的工具元数据（含 group 回带）', async ({ request }) => {
    const resp = await mcpCall(request, ENDPOINT_A, { method: 'tools/list' });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.tools.some((t: any) => t.name === 'read')).toBe(true);
    expect(body.result._meta['kissbot/group']).toContain('filesystem');
  });

  test('A 的 group 白名单会下传给子站 B', async ({ request }) => {
    const hit = await mcpCall(request, ENDPOINT_A, {
      method: 'tools/list',
      params: { group: ['filesystem'] },
    });
    expect((await hit.json()).result.tools.some((t: any) => t.name === 'read')).toBe(true);

    const miss = await mcpCall(request, ENDPOINT_A, {
      method: 'tools/list',
      params: { group: ['other'] },
    });
    expect((await miss.json()).result.tools).toEqual([]);
  });

  test('A 能经 MCP 远程调用子站 B 的 read 工具', async ({ request }) => {
    // 先拉取一次工具列表，确保 A 的路由缓存已包含 B 的 read
    await mcpCall(request, ENDPOINT_A, { method: 'tools/list' });

    const resp = await mcpCall(request, ENDPOINT_A, {
      method: 'tools/call',
      name: 'read',
      params: { name: 'read', arguments: { path: 'config.json' } },
    });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.result.isError).toBe(false);
    // read 是文本结果：文本块即文件内容
    expect(body.result.content[0].text).toContain('"agent"');
  });

  test('A 的祖先链已含自身时拒绝查询（防环）', async ({ request }) => {
    const resp = await mcpCall(request, ENDPOINT_A, {
      method: 'tools/list',
      ancestors: ['station-a'],
    });
    const body = await resp.json();
    expect(body.error.code).toBe(1001);
  });
});
