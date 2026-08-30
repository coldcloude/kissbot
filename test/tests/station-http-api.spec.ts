import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startAgent, stopAgent, waitForPort } from './helpers/server';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';

const STATION_BASE = 'http://127.0.0.1:9100';
const API_KEY = 'user-key-456'; // security.api_key
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let agent: ChildProcess;

async function apiPost(request: APIRequestContext, path: string, body: unknown, key = API_KEY) {
  return (await request.post(`${STATION_BASE}${path}`, {
    headers: { 'X-Api-Key': key, 'Content-Type': 'application/json' },
    data: body,
  }));
}

test.describe.serial('station HTTP API 测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    agent = startAgent(WORKSPACE);
    await waitForPort(9100, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopAgent(agent);
  });

  test('TC-01: 无 key / 错误 key → 401', async ({ request }) => {
    const noKey = await request.post(`${STATION_BASE}/station/tools`, {
      data: { ancestors: [] },
    });
    expect(noKey.status()).toBe(401);

    const wrongKey = await request.post(`${STATION_BASE}/station/tools`, {
      headers: { 'X-Api-Key': 'wrong', 'Content-Type': 'application/json' },
      data: { ancestors: [] },
    });
    expect(wrongKey.status()).toBe(401);
  });

  test('TC-02: /station/tools 返回本地工具元数据', async ({ request }) => {
    const resp = await (await apiPost(request, '/station/tools', {
      filter: ['filesystem'],
      ancestors: [],
    })).json();
    expect(resp.success).toBe(true);
    expect(Array.isArray(resp.data)).toBe(true);
    expect(resp.data.length).toBeGreaterThanOrEqual(1);
    expect(resp.data[0].name).toBe('read');
    expect(resp.data[0].parameters.type).toBe('object');
  });

  test('TC-03: /station/mcps 路由存在并返回空/占位 MCP 列表', async ({ request }) => {
    const resp = await (await apiPost(request, '/station/mcps', {
      ancestors: [],
    })).json();
    expect(resp.success).toBe(true);
    expect(Array.isArray(resp.data)).toBe(true);
  });

  test('TC-04: /station/call-tool 成功调用本地 read', async ({ request }) => {
    const resp = await (await apiPost(request, '/station/call-tool', {
      tool_name: 'read',
      parameters: { path: 'config.json' },
      ancestors: [],
    })).json();
    expect(resp.success).toBe(true);
    expect(typeof resp.data).toBe('string');
    expect(resp.data).toContain('"agent"');
  });

  test('TC-05: /station/call-tool 工具调用失败返回 HTTP 200 + success=false', async ({ request }) => {
    const resp = await (await apiPost(request, '/station/call-tool', {
      tool_name: 'missing-tool',
      parameters: {},
      ancestors: [],
    })).json();
    expect(resp.success).toBe(false);
    expect(resp.error).toBeTruthy();
  });

  test('TC-06: /station/tools 检测到自己在祖先链中 → 非 200', async ({ request }) => {
    const resp = await apiPost(request, '/station/tools', {
      ancestors: ['parent', 'station-a'],
    });
    expect(resp.status()).toBe(400);
    const body = await resp.json();
    expect(body.success).toBe(false);
    expect(body.error).toContain('cycle');
  });
});
