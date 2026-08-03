import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startMemoryStore, stopMemoryStore, waitForPort } from './helpers/server';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { join, dirname } from 'path';

const BASE = 'http://127.0.0.1:8082';
const API_KEY = 'user-key-456'; // security.api_key
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

// 测试常量（role_name 仅允许 [A-Za-z0-9_]+）
const AGENT = 'agent_a';
const ROLE = 'admin';
const MESSENGER = 'web';
const USER = 'u1';       // 发送者
const SELF_USER = 'self1'; // 接收方（= channel 绑定的 user_id），文件名按此分文件
const GROUP = 'g1';

let store: ChildProcess;

function pad(n: number): string {
  return String(n).padStart(2, '0');
}

function nowTime(): string {
  const d = new Date();
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function apiPost(request: APIRequestContext, path: string, body: unknown) {
  return (await request.post(`${BASE}${path}`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: body,
  })).json();
}

test.describe.serial('memory-store API 测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    store = startMemoryStore(WORKSPACE);
    await waitForPort(8082, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopMemoryStore(store);
  });

  // TC-01 追加 channel 记录并查询
  test('TC-01: 追加并查询 channel 记录', async ({ request }) => {
    const time = nowTime();
    const date = time.slice(0, 10); // 从 time 推断日期（单一时间源，避免两次取时钟在午夜边界不一致）
    const resp = await apiPost(request, '/store/channel', {
      requests: [{
        agent_id: AGENT, role_name: ROLE, messenger_id: MESSENGER,
        user_id: USER, self_user_id: SELF_USER, group_id: GROUP, is_self: 0,
        messenger_name: 'Web', user_name: '用户1', group_name: '群组1',
        content: { msg_type: 'Text', data: '你好' }, time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    // 等待追加器落盘（FileObjectAppender 100ms 批量）
    await sleep(1000);

    const q = await apiPost(request, '/store/query/channel', {
      agent_id: AGENT, role_name: ROLE, messenger_id: MESSENGER,
      user_id: SELF_USER, group_id: GROUP,
      start_time: `${date} 00:00:00`, end_time: `${date} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(Array.isArray(q.data)).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1]; // [[line, record], ...]
    expect(records.length).toBeGreaterThanOrEqual(1);
    // key 中含 messenger_id/user_id/group_id/date；文件名 user_id 按接收方 self_user_id 分文件
    expect(q.data[0][0].user_id).toBe(SELF_USER);
    expect(q.data[0][0].group_id).toBe(GROUP);
    expect(q.data[0][0].date).toBe(date);
    const record = records[records.length - 1][1];
    // record.user_id 保留发送者身份（区分对话方向）
    expect(record.user_id).toBe(USER);
    // 记录本身不含 group_id；is_self 按追加值回读
    expect(record.group_id).toBeUndefined();
    expect(record.is_self).toBe(0);
    expect(record.content).toEqual({ msg_type: 'Text', data: '你好' });
    expect(record.time).toBe(time);
  });

  // TC-02 追加 think 记录并查询
  test('TC-02: 追加并查询 think 记录', async ({ request }) => {
    const time = nowTime();
    const date = time.slice(0, 10); // 单一时间源
    const resp = await apiPost(request, '/store/think', {
      requests: [{
        agent_id: AGENT, role_name: ROLE,
        content: '思考内容', key: 'think_key_1', time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    await sleep(1000);

    const q = await apiPost(request, '/store/query/think', {
      agent_id: AGENT, role_name: ROLE,
      start_time: `${date} 00:00:00`, end_time: `${date} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1];
    expect(records.length).toBeGreaterThanOrEqual(1);
    const record = records[records.length - 1][1];
    expect(record.content).toBe('思考内容');
    expect(record.key).toBe('think_key_1');
    expect(record.time).toBe(time);
  });

  // TC-03 追加 tool-call 记录并查询
  test('TC-03: 追加并查询 tool-call 记录', async ({ request }) => {
    const time = nowTime();
    const date = time.slice(0, 10); // 单一时间源
    const resp = await apiPost(request, '/store/tool-call', {
      requests: [{
        agent_id: AGENT, role_name: ROLE,
        tool_name: 'get_weather', tool_params: { city: 'Beijing' }, key: 'tool_call_key_1', time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    await sleep(1000);

    const q = await apiPost(request, '/store/query/tool-call', {
      agent_id: AGENT, role_name: ROLE,
      start_time: `${date} 00:00:00`, end_time: `${date} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1];
    expect(records.length).toBeGreaterThanOrEqual(1);
    const record = records[records.length - 1][1];
    expect(record.tool_name).toBe('get_weather');
    expect(record.tool_params).toEqual({ city: 'Beijing' });
    expect(record.key).toBe('tool_call_key_1');
    expect(record.time).toBe(time);
  });

  // TC-04 追加 tool-result 记录并查询
  test('TC-04: 追加并查询 tool-result 记录', async ({ request }) => {
    const time = nowTime();
    const date = time.slice(0, 10); // 单一时间源
    const resp = await apiPost(request, '/store/tool-result', {
      requests: [{
        agent_id: AGENT, role_name: ROLE,
        tool_result: { temp: 25 }, key: 'tool_result_key_1', time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    await sleep(1000);

    const q = await apiPost(request, '/store/query/tool-result', {
      agent_id: AGENT, role_name: ROLE,
      start_time: `${date} 00:00:00`, end_time: `${date} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1];
    expect(records.length).toBeGreaterThanOrEqual(1);
    const record = records[records.length - 1][1];
    expect(record.tool_result).toEqual({ temp: 25 });
    expect(record.key).toBe('tool_result_key_1');
    expect(record.time).toBe(time);
  });
});
