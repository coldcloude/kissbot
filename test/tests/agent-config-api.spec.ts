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
    expect(resp.data.providers.deepseek.provider_config.provider_type).toBe('openai');
    expect(resp.data.agent_roles['0'].config_map.llm).toEqual(
      expect.objectContaining({ provider: 'deepseek', model: 'deepseek-flash' }));
    expect(resp.data.channels['web-main'].enabled).toBe(true);
    expect(resp.data.pipelines['0'][''].pipeline_name).toBe('default_memory');
  });

  test('TC-02: POST /config/providers 添加并落盘', async ({ request }) => {
    const resp = await apiPost(request, '/config/providers', {
      name: 'anthropic',
      config: {
        provider_config: {
          provider_type: 'anthropic',
          base_url: 'https://api.anthropic.com',
          api_key: '',
        },
        // provider 默认模型参数（max_tokens_usage 必填，未配字段回落全局默认）
        default_model_config: { max_tokens_usage: 200000, timeout_secs: 60, retry_count: 3 },
        // model_configs 的 key 为模型标识（max_tokens_usage 必填）
        model_configs: { 'claude-sonnet-4': { max_tokens_usage: 200000, timeout_secs: 30 } },
      },
    });
    expect(resp.success).toBe(true);
    // GET 验证
    const info = await apiGet(request, '/config');
    expect(info.data.providers.anthropic.provider_config.base_url).toBe('https://api.anthropic.com');
    expect(info.data.providers.anthropic.default_model_config.max_tokens_usage).toBe(200000);
    // nexus.json 落盘验证
    const saved = JSON.parse(readFileSync(NEXUS_FILE, 'utf8'));
    expect(saved.providers.anthropic.provider_config.provider_type).toBe('anthropic');
    expect(saved.providers.anthropic.model_configs['claude-sonnet-4'].timeout_secs).toBe(30);
  });

  test('TC-03: POST /config/channels/remove 删除并落盘', async ({ request }) => {
    const add = await apiPost(request, '/config/channels', {
      channel_id: 'web-3', ws_url: 'ws://127.0.0.1:8201',
      admins: [], enabled: false,
    });
    expect(add.success).toBe(true);
    const del = await apiPost(request, '/config/channels/remove', { channel_id: 'web-3' });
    expect(del.success).toBe(true);
    const info = await apiGet(request, '/config');
    expect(info.data.channels['web-3']).toBeUndefined();
    const saved = JSON.parse(readFileSync(NEXUS_FILE, 'utf8'));
    expect(saved.channels['web-3']).toBeUndefined();
  });

  test('TC-04: POST /config/channels 与 /config/admins', async ({ request }) => {
    const ch = await apiPost(request, '/config/channels', {
      channel_id: 'web-2', ws_url: 'ws://127.0.0.1:8201',
      admins: [], enabled: false,
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

  test('TC-07: pipeline 新增/改名/重载/移除与 input channel 管理', async ({ request }) => {
    const agent = 'p-agent';
    // 未知名称 → 失败
    expect((await apiPost(request, '/config/pipelines', {
      agent_id: agent, role_name: '', pipeline_name: 'no-such-pipeline',
    })).success).toBe(false);
    // 新增 default pipeline
    expect((await apiPost(request, '/config/pipelines', {
      agent_id: agent, role_name: '', pipeline_name: 'default',
    })).success).toBe(true);
    // 重复新增 → 失败
    expect((await apiPost(request, '/config/pipelines', {
      agent_id: agent, role_name: '', pipeline_name: 'default',
    })).success).toBe(false);
    // 添加 input channel（web-main 已启用，会真实绑定到 channel）
    const input = {
      agent_id: agent, role_name: '',
      channel_id: 'web-main', messenger_id: 'web', user_id: 'u-pipe',
    };
    expect((await apiPost(request, '/config/pipelines/input-channels', input)).success).toBe(true);
    // 重复添加同一身份 → 失败
    expect((await apiPost(request, '/config/pipelines/input-channels', input)).success).toBe(false);
    // 落盘验证
    const saved = JSON.parse(readFileSync(NEXUS_FILE, 'utf8'));
    expect(saved.pipelines[agent][''].pipeline_name).toBe('default');
    expect(saved.agent_roles[agent].config_map.input_channels.input_channels[0].user_id).toBe('u-pipe');
    // 改名（按新名称重新加载）
    expect((await apiPost(request, '/config/pipelines/change', {
      agent_id: agent, role_name: '', pipeline_name: 'default_memory',
    })).success).toBe(true);
    const info = await apiGet(request, '/config');
    expect(info.data.pipelines[agent][''].pipeline_name).toBe('default_memory');
    // 重新加载
    expect((await apiPost(request, '/config/pipelines/reload', {
      agent_id: agent, role_name: '',
    })).success).toBe(true);
    // 移除 input channel（再次移除幂等成功）
    expect((await apiPost(request, '/config/pipelines/input-channels/remove', input)).success).toBe(true);
    expect((await apiPost(request, '/config/pipelines/input-channels/remove', input)).success).toBe(true);
    // 移除 pipeline
    expect((await apiPost(request, '/config/pipelines/remove', {
      agent_id: agent, role_name: '',
    })).success).toBe(true);
    const after = await apiGet(request, '/config');
    expect(after.data.pipelines[agent]).toBeUndefined();
  });

  test('TC-08: channel enable / disable 同步运行态', async ({ request }) => {
    expect((await apiPost(request, '/config/channels', {
      channel_id: 'toggle-ch', ws_url: 'ws://127.0.0.1:1',
      admins: [], enabled: false,
    })).success).toBe(true);
    // enable → 配置置 true（连接目标不可达时按间隔重试，不影响接口返回）
    expect((await apiPost(request, '/config/channels/enable', { channel_id: 'toggle-ch' })).success).toBe(true);
    let info = await apiGet(request, '/config');
    expect(info.data.channels['toggle-ch'].enabled).toBe(true);
    // disable → 配置置 false 并移除运行态
    expect((await apiPost(request, '/config/channels/disable', { channel_id: 'toggle-ch' })).success).toBe(true);
    info = await apiGet(request, '/config');
    expect(info.data.channels['toggle-ch'].enabled).toBe(false);
    // 不存在的 channel → 失败
    expect((await apiPost(request, '/config/channels/disable', { channel_id: 'no-such-channel' })).success).toBe(false);
  });
});
