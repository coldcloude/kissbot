import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startAgent, stopAgent, waitForPort } from './helpers/server';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { readFileSync } from 'fs';

const BASE = 'http://127.0.0.1:9090';
const ADMIN_KEY = 'admin-key-123';
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');
const NEXUS_FILE = join(WORKSPACE, 'agent-data', 'nexus.json');

let agent: ChildProcess;

async function apiGet(request: APIRequestContext, path: string, key = ADMIN_KEY) {
  return (await request.get(`${BASE}${path}`, {
    headers: { 'X-Api-Key': key },
  })).json();
}

async function apiPost(request: APIRequestContext, path: string, body: unknown, key = ADMIN_KEY) {
  return (await request.post(`${BASE}${path}`, {
    headers: { 'X-Api-Key': key, 'Content-Type': 'application/json' },
    data: body,
  })).json();
}

test.describe.serial('agent 配置管理 API 测试（HTTP 修改配置）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopAgent(agent);
  });

  test('TC-01: GET /config 读初始快照', async ({ request }) => {
    const resp = await apiGet(request, '/config');
    expect(resp.success).toBe(true);
    expect(resp.data.providers.deepseek).toBeTruthy();
    expect(resp.data.default_model).toEqual({ provider: 'deepseek', model: 'deepseek-4-flash' });
  });

  test('TC-02: POST /config/providers 添加并落盘', async ({ request }) => {
    const resp = await apiPost(request, '/config/providers', {
      name: 'anthropic', provider_type: 'anthropic',
      base_url: 'https://api.anthropic.com', api_key: '',
      default_context_length: 200000, default_max_tokens: 8192,
      default_temperature: 0.7, default_timeout_secs: 60, default_retry_count: 3,
      models: { 'claude-sonnet-4': { model: 'claude-sonnet-4' } },
    });
    expect(resp.success).toBe(true);
    // GET 验证
    const info = await apiGet(request, '/config');
    expect(info.data.providers.anthropic).toBeTruthy();
    // nexus.json 落盘验证
    const saved = JSON.parse(readFileSync(NEXUS_FILE, 'utf8'));
    expect(saved.providers.anthropic).toBeTruthy();
  });

  test('TC-03: POST /config/default 修改默认模型', async ({ request }) => {
    const resp = await apiPost(request, '/config/default', {
      provider: 'anthropic', model: 'claude-sonnet-4',
    });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/config');
    expect(info.data.default_model).toEqual({ provider: 'anthropic', model: 'claude-sonnet-4' });
    const saved = JSON.parse(readFileSync(NEXUS_FILE, 'utf8'));
    expect(saved.default_model.model).toBe('claude-sonnet-4');
  });

  test('TC-04: POST /config/channels 与 /config/admins', async ({ request }) => {
    const ch = await apiPost(request, '/config/channels', {
      channel_id: 'web-2', ws_url: 'ws://127.0.0.1:8201',
      admins: [], bind_user: { messenger_id: 'web', user_id: 'u1' },
      agent_id: '0', role_name: '0', is_send_channel: false, enabled: false,
    });
    expect(ch.success).toBe(true);
    const adm = await apiPost(request, '/config/admins', {
      channel_id: 'web-2', messenger_id: 'web', user_id: 'u3',
    });
    expect(adm.success).toBe(true);
    const info = await apiGet(request, '/config');
    expect(info.data.channels['web-2']).toBeTruthy();
  });

  test('TC-05: 错误 API Key → 401', async ({ request }) => {
    const resp = await apiGet(request, '/config', 'wrong-key');
    expect(resp.success).toBe(false);
    const resp401 = await request.get(`${BASE}/config`, {
      headers: { 'X-Api-Key': 'wrong-key' },
    });
    expect(resp401.status()).toBe(401);
  });

  test('TC-06: 删除不存在的 provider → 失败', async ({ request }) => {
    const resp = await apiPost(request, '/config/providers/remove', { name: 'nope' });
    expect(resp.success).toBe(false);
    expect(resp.error).toBeTruthy();
  });
});
