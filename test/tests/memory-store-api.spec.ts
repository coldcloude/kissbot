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

// 测试常量（role_event.role_name 仅允许 [A-Za-z0-9_]+）
const AGENT = 'agent_a';
const ROLE = 'admin';
const MESSENGER = 'web';
const USER = 'u1';       // 发送者
const SELF_USER = 'self1'; // 接收方（= channel 绑定的 user_id）；记录中与发送者 user_id 区分，is_self 仅 agent 实际发送为 1
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
        agent_id: AGENT, role_event: { role_name: ROLE }, messenger_id: MESSENGER,
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
      agent_id: AGENT, role_event: { role_name: ROLE },
      start_time: `${date} 00:00:00`, end_time: `${date} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(Array.isArray(q.data)).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1]; // [[line, record], ...]
    expect(records.length).toBeGreaterThanOrEqual(1);
    // key 为公共 RecordKey（agent_id/role_event/date）；所有 channel 记录同文件，key 无身份字段
    expect(q.data[0][0].agent_id).toBe(AGENT);
    expect(q.data[0][0].role_event).toEqual({ role_name: ROLE });
    expect(q.data[0][0].date).toBe(date);
    const record = records[records.length - 1][1];
    // record 保存完整身份：user_id=发送者、self_user_id=接收方（绑定用户）、messenger_id/group_id
    expect(record.user_id).toBe(USER);
    expect(record.self_user_id).toBe(SELF_USER);
    expect(record.messenger_id).toBe(MESSENGER);
    expect(record.group_id).toBe(GROUP);
    // is_self 按追加值回读
    expect(record.is_self).toBe(0);
    expect(record.content).toEqual({ msg_type: 'Text', data: '你好' });
    expect(record.time).toBe(time);
  });

  // TC-02 通道记录内容内嵌思考/工具记录，随通道记录一起查询
  test('TC-02: 通道记录内嵌思考/工具内容', async ({ request }) => {
    const time = nowTime();
    const date = time.slice(0, 10); // 单一时间源
    const content = {
      msg_type: 'Multi',
      data: [
        { msg_type: 'Think', data: { reasoning_content: '思考内容' } },
        { msg_type: 'ToolCall', data: { tool_name: 'get_weather', tool_params: { city: 'Beijing' } } },
        { msg_type: 'ToolResult', data: { tool_result: [{ type: 'text', text: '{"temp":25}' }] } },
        { msg_type: 'Text', data: '回复' },
      ],
    };
    const resp = await apiPost(request, '/store/channel', {
      requests: [{
        agent_id: AGENT, role_event: { role_name: ROLE }, messenger_id: MESSENGER,
        user_id: SELF_USER, self_user_id: SELF_USER, group_id: GROUP, is_self: 1,
        messenger_name: 'Web', user_name: 'agent', group_name: '群组1',
        content, time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    await sleep(1000);

    const q = await apiPost(request, '/store/query/channel', {
      agent_id: AGENT, role_event: { role_name: ROLE },
      start_time: `${date} 00:00:00`, end_time: `${date} 23:59:59`,
    });
    expect(q.success).toBe(true);
    const records = q.data.flatMap((group: any[]) => group[1]);
    const record = records[records.length - 1][1];
    expect(record.content).toEqual(content);
    // 记录本身没有 key/time 之外的详情位置字段；思考/工具内容不单独成记录
    expect(record.content.data[0].data.key).toBeUndefined();
    expect(record.content.data[0].data.time).toBeUndefined();
  });

  // TC-03 独立的思考/工具记录端点已移除
  test('TC-03: 独立记录端点已移除', async ({ request }) => {
    for (const path of ['/store/think', '/store/tool-call', '/store/tool-result']) {
      const resp = await request.post(`${BASE}${path}`, {
        headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
        data: { requests: [], force: 0 },
      });
      expect(resp.status(), `${path} 应返回 404`).toBe(404);
    }
    for (const path of ['/store/query/think', '/store/query/tool-call', '/store/query/tool-result']) {
      const resp = await request.post(`${BASE}${path}`, {
        headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
        data: { agent_id: AGENT, role_event: { role_name: ROLE }, start_time: '2026-01-01', end_time: '2026-12-31' },
      });
      expect(resp.status(), `${path} 应返回 404`).toBe(404);
    }
  });
});
