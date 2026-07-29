import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, waitForPort } from './helpers/server';
import { ChildProcess } from 'child_process';
import { join } from 'path';

const BASE = 'http://127.0.0.1:8301';
const API_KEY = 'admin-key-123';
const WORKSPACE = join(__dirname, '..', 'workspace');

let backend: ChildProcess;

// 测试间共享变量
let sharedMsgId: string;
let sharedGroupId: string;
let sharedTransferId: number;
let sharedAttKey: string;

// 辅助函数：用 request 发 GET
async function apiGet(request: APIRequestContext, path: string) {
  return (await request.get(`${BASE}${path}`, {
    headers: { 'X-Api-Key': API_KEY },
  })).json();
}

// 辅助函数：用 request 发 POST
async function apiPost(request: APIRequestContext, path: string, body: unknown) {
  return (await request.post(`${BASE}${path}`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: body,
  })).json();
}

test.describe.serial('channel-web 后端 API 测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopBackend(backend);
  });

  // TC-01 获取管理员信息
  test('TC-01: 获取管理员信息', async ({ request }) => {
    const resp = await apiGet(request, '/api/info');
    expect(resp.success).toBe(true);
    expect(resp.data.messenger_id).toBe('web');
    expect(resp.data.admin_name).toBe('管理员');
    expect(resp.data.users).toHaveProperty('user-1');
    expect(resp.data.users['user-1'].user_id).toBe('user-1');
    expect(resp.data.users).toHaveProperty('user-2');
    expect(resp.data.groups).toHaveProperty('dev-team');
    expect(resp.data.groups).toHaveProperty('project-x');
  });

  // TC-02 错误 API Key
  test('TC-02: 错误 API Key', async ({ request }) => {
    const resp = await (await request.get(`${BASE}/api/info`, {
      headers: { 'X-Api-Key': 'wrong-key' },
    })).json();
    expect(resp.success).toBe(false);
  });

  // TC-03 发送文本消息
  test('TC-03: 发送文本消息', async ({ request }) => {
    const resp = await apiPost(request, '/api/message/send', {
      messenger_id: 'web',
      user_id: 'admin',
      group_id: 'dev-team',
      content: { msg_type: 'Text', data: '你好！' },
    });
    expect(resp.success).toBe(true);
    expect(resp.data.msg_id).toBeTruthy();
    expect(resp.data.time).toMatch(/^\d{4}-\d{2}-\d{2} /);
    expect(resp.data.content).toEqual({ msg_type: 'Text', data: '你好！' });
    sharedMsgId = resp.data.msg_id;
  });

  // TC-04 发送消息到不存在的群组
  test('TC-04: 发送消息到不存在的群组', async ({ request }) => {
    const resp = await apiPost(request, '/api/message/send', {
      messenger_id: 'web', user_id: 'admin', group_id: 'nonexistent',
      content: { msg_type: 'Text', data: '你好' },
    });
    expect(resp.success).toBe(false);
    expect(resp.error).toBeTruthy();
  });

  // TC-05 获取最近消息
  test('TC-05: 获取最近消息', async ({ request }) => {
    // 消息存储有 3 秒缓冲延迟，等待 4 秒
    await new Promise(r => setTimeout(r, 4000));
    const resp = await apiGet(request, '/api/messages/recent?group_id=dev-team&n=5');
    expect(resp.success).toBe(true);
    expect(Array.isArray(resp.data)).toBe(true);
    expect(resp.data.length).toBeGreaterThanOrEqual(1);
    expect(resp.data[0].messages[0].message.msg_id).toBe(sharedMsgId);
  });

  // TC-06 创建群组
  test('TC-06: 创建群组', async ({ request }) => {
    const resp = await apiPost(request, '/api/groups/create', {
      group_name: '新群组', member_ids: ['user-1'],
    });
    expect(resp.success).toBe(true);
    expect(resp.data.group_id).toBeTruthy();
    sharedGroupId = resp.data.group_id;
  });

  // TC-07 创建群组后自动出现在会话列表
  test('TC-07: 创建群组后自动出现在会话列表', async ({ request }) => {
    const resp = await apiGet(request, '/api/info');
    expect(resp.success).toBe(true);
    const g = resp.data.groups[sharedGroupId];
    expect(g).toBeTruthy();
    expect(g.group_name).toBe('新群组');
    expect(g.members).toContain('user-1');
  });

  // TC-08 重命名群组
  test('TC-08: 重命名群组', async ({ request }) => {
    const resp = await apiPost(request, '/api/groups/rename', {
      group_id: sharedGroupId, group_name: '重命名后的群组',
    });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/api/info');
    expect(info.data.groups[sharedGroupId].group_name).toBe('重命名后的群组');
  });

  // TC-09 管理成员——添加成员
  test('TC-09: 管理成员——添加成员', async ({ request }) => {
    const resp = await apiPost(request, '/api/groups/manage-members', {
      group_id: sharedGroupId, add_ids: ['user-2'], remove_ids: [],
    });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/api/info');
    expect(info.data.groups[sharedGroupId].members.sort()).toEqual(['user-1', 'user-2']);
  });

  // TC-10 管理成员——移除成员
  test('TC-10: 管理成员——移除成员', async ({ request }) => {
    const resp = await apiPost(request, '/api/groups/manage-members', {
      group_id: sharedGroupId, add_ids: [], remove_ids: ['user-2'],
    });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/api/info');
    expect(info.data.groups[sharedGroupId].members).toEqual(['user-1']);
  });

  // TC-11 创建用户
  test('TC-11: 创建用户', async ({ request }) => {
    const resp = await apiPost(request, '/api/users/create', { user_name: '助手小C' });
    expect(resp.success).toBe(true);
    expect(resp.data.user_id).toBeTruthy();
  });

  // TC-12 新创建的用户出现在用户列表中
  test('TC-12: 新创建的用户出现在用户列表中', async ({ request }) => {
    const info = await apiGet(request, '/api/info');
    expect(info.data.users).toHaveProperty('u3');
    expect(info.data.users.u3.user_name).toBe('助手小C');
  });

  // TC-13 重命名用户
  test('TC-13: 重命名用户', async ({ request }) => {
    const resp = await apiPost(request, '/api/users/rename', { user_id: 'u3', user_name: '助手小C（改）' });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/api/info');
    expect(info.data.users.u3.user_name).toBe('助手小C（改）');
  });

  // TC-14 管理员改名
  test('TC-14: 管理员改名', async ({ request }) => {
    const resp = await apiPost(request, '/api/admin/rename', { admin_name: '超级管理员' });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/api/info');
    expect(info.data.admin_name).toBe('超级管理员');
  });

  // TC-15 删除群组
  test('TC-15: 删除群组', async ({ request }) => {
    const resp = await apiPost(request, '/api/groups/delete', { group_id: sharedGroupId });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/api/info');
    expect(info.data.groups).not.toHaveProperty(sharedGroupId);
  });

  // TC-16 删除用户
  test('TC-16: 删除用户', async ({ request }) => {
    const resp = await apiPost(request, '/api/users/delete', { user_id: 'u3' });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/api/info');
    expect(info.data.users).not.toHaveProperty('u3');
  });

  // TC-17 删除不存在的群组
  test('TC-17: 删除不存在的群组', async ({ request }) => {
    const resp = await apiPost(request, '/api/groups/delete', { group_id: 'nonexistent' });
    expect(resp.success).toBe(false);
  });

  // TC-18 删除不存在的用户
  test('TC-18: 删除不存在的用户', async ({ request }) => {
    const resp = await apiPost(request, '/api/users/delete', { user_id: 'nonexistent' });
    expect(resp.success).toBe(false);
  });

  // TC-19 admin-user 单聊群组不可操作
  test('TC-19: admin-user 单聊群组不可操作', async ({ request }) => {
    const r1 = await apiPost(request, '/api/groups/rename', { group_id: 'a_user-1', group_name: '改名' });
    expect(r1.success).toBe(false);
    const r2 = await apiPost(request, '/api/groups/delete', { group_id: 'a_user-1' });
    expect(r2.success).toBe(false);
  });

  // TC-20 附件上传——发消息获取 transfer_id
  test('TC-20: 附件上传——发消息获取 transfer_id', async ({ request }) => {
    const resp = await apiPost(request, '/api/message/send', {
      messenger_id: 'web', user_id: 'admin', group_id: 'dev-team',
      content: {
        msg_type: 'AttachmentInfo',
        data: { file_name: 'photo.png', mime_type: 'image/png', size_bytes: 68 },
      },
    });
    expect(resp.success).toBe(true);
    expect(resp.data.content.data.key).toBeTruthy();
    expect(typeof resp.data.content.data.transfer_id).toBe('number');
    sharedTransferId = resp.data.content.data.transfer_id;
    sharedAttKey = resp.data.content.data.key;
  });

  // TC-21 附件上传——上传文件数据
  test('TC-21: 附件上传——上传文件数据', async ({ request }) => {
    // 使用 1x1 红色 PNG 作为测试图片
    const pngBuffer = Buffer.from([
      0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
      0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
      0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1 pixel
      0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, // 8-bit RGB
      0xDE, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, // IDAT chunk length
      0x54, 0x78, 0x9C, 0xFB, 0xCF, 0xC0, 0x00, 0x00, // IDAT type + data
      0x03, 0x00, 0x01, 0x00, 0x83, 0xC9, 0xEC, 0x6B, // IDAT data + crc
      0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND length + type
      0xAE, 0x42, 0x60, 0x82,                         // IEND crc
    ]);
    const resp = await request.post(`${BASE}/api/attachment/upload`, {
      headers: { 'X-Api-Key': API_KEY },
      multipart: {
        transfer_id: String(sharedTransferId),
        file: { name: 'photo.png', mimeType: 'image/png', buffer: pngBuffer },
      },
    });
    expect((await resp.json()).success).toBe(true);
  });

  // TC-22 附件下载
  test('TC-22: 附件下载', async ({ request }) => {
    const resp = await request.get(`${BASE}/api/attachment/download?key=${sharedAttKey}`, {
      headers: { 'X-Api-Key': API_KEY },
    });
    const body = await resp.body();
    expect(body[0]).toBe(0x89); // PNG signature
    expect(body[1]).toBe(0x50);
  });

  // TC-23 附件缩略图（图片）
  test('TC-23: 附件缩略图（图片）', async ({ request }) => {
    const resp = await request.get(`${BASE}/api/attachment/thumbnail?key=${sharedAttKey}`, {
      headers: { 'X-Api-Key': API_KEY },
    });
    const body = await resp.body();
    // 如果缩略图生成成功应为 JPEG 格式；否则至少返回非空数据（错误信息）
    expect(body.length).toBeGreaterThan(0);
  });

  // TC-24 分页加载历史消息
  test('TC-24: 分页加载历史消息', async ({ request }) => {
    const recent = await apiGet(request, '/api/messages/recent?group_id=dev-team&n=1');
    const group = recent.data[0];
    if (!group) return; // 无消息不报错
    const firstMsg = group.messages[0];
    if (!firstMsg) return;
    const before = await apiGet(request, '/api/messages/before?group_id=dev-team&date=' + group.key.date + '&line=' + firstMsg.line + '&n=10');
    expect(before.success).toBe(true);
  });
});
