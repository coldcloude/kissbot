import { test, expect } from '@playwright/test';
import { startAgent, stopAgent, waitForPort } from './helpers/server';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { cpSync, mkdtempSync, rmSync, readFileSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';

const API_KEY = 'user-key-456';
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = join(__dirname, '..', '..');
const TEMPLATE = join(REPO_ROOT, 'test', 'workspace-template');

const MGMT_A = 9092;
const STATION_A = 9102;
const MGMT_B = 9093;
const STATION_B = 9103;

let dirA: string;
let dirB: string;
let agentA: ChildProcess;
let agentB: ChildProcess;

function writeJson(file: string, value: unknown): void {
  writeFileSync(file, JSON.stringify(value, null, 2) + '\n');
}

test.describe.serial('station 远程调用端到端测试（A 调用子站 B）', () => {

  test.beforeAll(async () => {
    dirA = mkdtempSync(join(tmpdir(), 'kissbot-station-a-'));
    dirB = mkdtempSync(join(tmpdir(), 'kissbot-station-b-'));
    cpSync(TEMPLATE, dirA, { recursive: true });
    cpSync(TEMPLATE, dirB, { recursive: true });

    // A：独立端口，本地无 toolkit，仅配子站 B
    const cfgA = JSON.parse(readFileSync(join(dirA, 'config.json'), 'utf8'));
    cfgA.agent.mgmt_port = MGMT_A;
    cfgA.agent.station_port = STATION_A;
    writeJson(join(dirA, 'config.json'), cfgA);
    writeJson(join(dirA, 'agent-data', 'station.json'), {
      station_id: 'station-a',
      toolkits: {},
      sub_stations: {
        'station-b': {
          station_id: 'station-b',
          base_url: `http://127.0.0.1:${STATION_B}`,
          timeout_secs: 5,
        },
      },
    });

    // B：独立端口，本地 filesystem toolkit
    const cfgB = JSON.parse(readFileSync(join(dirB, 'config.json'), 'utf8'));
    cfgB.agent.mgmt_port = MGMT_B;
    cfgB.agent.station_port = STATION_B;
    writeJson(join(dirB, 'config.json'), cfgB);
    writeJson(join(dirB, 'agent-data', 'station.json'), {
      station_id: 'station-b',
      toolkits: { filesystem: {} },
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

  test('A 能平铺到子站 B 的工具元数据', async ({ request }) => {
    const resp = await (await request.post(`http://127.0.0.1:${STATION_A}/station/tools`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { ancestors: [] },
    })).json();
    expect(resp.success).toBe(true);
    expect(resp.data.length).toBeGreaterThanOrEqual(1);
    expect(resp.data.some((t: any) => t.name === 'read')).toBe(true);
  });

  test('A 能远程调用子站 B 的 read 工具', async ({ request }) => {
    // 先拉取一次工具列表，确保 A 的路由缓存已包含 B 的 read
    await request.post(`http://127.0.0.1:${STATION_A}/station/tools`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { ancestors: [] },
    });

    const resp = await (await request.post(`http://127.0.0.1:${STATION_A}/station/call-tool`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: {
        tool_name: 'read',
        parameters: { path: 'config.json' },
        ancestors: [],
      },
    })).json();
    expect(resp.success).toBe(true);
    expect(typeof resp.data).toBe('string');
    expect(resp.data).toContain('"agent"');
  });
});
