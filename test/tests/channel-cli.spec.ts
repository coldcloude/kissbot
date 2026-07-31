import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, waitForPort } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { generateSmallPng, generateTextFile } from './helpers/assets';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { join, dirname } from 'path';
import { writeFileSync, mkdtempSync, readFileSync } from 'fs';
import { tmpdir } from 'os';

const BASE = 'http://127.0.0.1:8301';
const API_KEY = 'admin-key-123';
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let backend: ChildProcess;
let cli1: SpawnedCli;   // u1 绑定 g1（channel-client 侧）
let cli2: SpawnedCli;   // u2 绑定 g2（channel-client 侧）
let tmpDir: string;

// 测试间共享变量
let apiAttKey = '';     // 附件 key（API 上传 → client-cli 下载）
let cliAttKey = '';     // 附件 key（client-cli 上传 → API 下载）

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

// 轮询最近消息直到满足条件的消息出现（消息存储有 1 秒缓冲）
async function waitForRecentMessage(
  request: APIRequestContext,
  groupId: string,
  predicate: (msg: any) => boolean,
  timeout = 15000,
): Promise<any> {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const resp = await apiGet(request, `/api/messages/recent?group_id=${groupId}&n=50`);
    if (resp.success) {
      for (const gm of resp.data ?? []) {
        for (const lm of gm.messages ?? []) {
          if (predicate(lm.message)) return lm.message;
        }
      }
    }
    await new Promise(r => setTimeout(r, 500));
  }
  throw new Error(`timeout waiting for message in group ${groupId}`);
}

// 通过 API 上传附件（发消息注册 → multipart 上传），返回 key
async function apiUploadAttachment(
  request: APIRequestContext,
  groupId: string,
  file: { name: string; mimeType: string; buffer: Buffer },
): Promise<string> {
  const resp = await apiPost(request, '/api/message/send', {
    messenger_id: 'web', user_id: 'admin', group_id: groupId,
    content: {
      msg_type: 'AttachmentInfo',
      data: { file_name: file.name, mime_type: file.mimeType, size_bytes: file.buffer.length },
    },
  });
  expect(resp.success).toBe(true);
  const transferId = resp.data.content.data.transfer_id;
  const key = resp.data.content.data.key;
  expect(typeof transferId).toBe('number');
  expect(key).toBeTruthy();

  const up = await request.post(`${BASE}/api/attachment/upload`, {
    headers: { 'X-Api-Key': API_KEY },
    multipart: {
      transfer_id: String(transferId),
      file: { name: file.name, mimeType: file.mimeType, buffer: file.buffer },
    },
  });
  expect((await up.json()).success).toBe(true);
  return key;
}

test.describe.serial('channel-cli 测试：channel-web API ↔ channelmanager ↔ channel-client', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    tmpDir = mkdtempSync(join(tmpdir(), 'kissbot-cli-'));
  });

  test.afterAll(() => {
    if (cli1) cli1.proc.kill();
    if (cli2) cli2.proc.kill();
    stopBackend(backend);
  });

  // ===== 辅助：如果 CLI 已断开则重启 =====
  async function ensureCli1() {
    if (cli1 && (cli1.proc.exitCode !== null || cli1.proc.killed)) {
      cli1 = spawnCli(['web', 'u1', 'g1', './downloads'], WORKSPACE);
      await cli1.waitForOutput(/bound\./);
    }
  }

  // ================================================================
  // TC-01 client-cli 启动并绑定（channelmanager 的 bind 协议）
  // ================================================================
  test('TC-01: client-cli 启动并绑定', async ({ request }) => {
    // 后端 /api/info 正常（channel-web 服务可用）
    const info = await apiGet(request, '/api/info');
    expect(info.success).toBe(true);
    expect(info.data.messenger_id).toBe('web');

    // u1 绑定 g1、u2 绑定 g2
    cli1 = spawnCli(['web', 'u1', 'g1', './downloads'], WORKSPACE);
    await cli1.waitForOutput(/bound\./);
    cli2 = spawnCli(['web', 'u2', 'g2', './downloads'], WORKSPACE);
    await cli2.waitForOutput(/bound\./);
  });

  // ================================================================
  // TC-02 API 发文本消息 → 两个绑定的 client-cli 都收到（下行扇出）
  // ================================================================
  test('TC-02: API 发文本消息 → client-cli 收到（channelmanager 下行分发）', async ({ request }) => {
    const resp = await apiPost(request, '/api/message/send', {
      messenger_id: 'web', user_id: 'admin', group_id: 'g1',
      content: { msg_type: 'Text', data: 'api-to-cli 测试消息' },
    });
    expect(resp.success).toBe(true);

    // g1 成员 u1、u2 各有一条绑定连接，都应收到同一条消息
    await cli1.waitForOutput(/<< \[admin:g1\].*api-to-cli 测试消息/, 10000);
    await cli2.waitForOutput(/<< \[admin:g1\].*api-to-cli 测试消息/, 10000);
  });

  // ================================================================
  // TC-03 client-cli 发文本消息 → API 消息历史可查（上行链路）
  // ================================================================
  test('TC-03: client-cli 发文本消息 → API 历史可查（channelclient 上行链路）', async ({ request }) => {
    cli1.stdin('cli-to-api 测试消息');
    await cli1.waitForOutput(/>> sent msg_id=/);

    // 上行：channelclient → channelmanager → messenger → 本地存储，经 API 可查
    const msg = await waitForRecentMessage(
      request,
      'g1',
      (m: any) => m.content?.msg_type === 'Text' && m.content?.data === 'cli-to-api 测试消息',
    );
    expect(msg.user_id).toBe('u1');
    expect(msg.group_id).toBe('g1');
  });

  // ================================================================
  // TC-04 API 上传图片附件 → client-cli 下载（附件下行链路）
  // ================================================================
  test('TC-04: API 上传图片附件 → client-cli 下载', async ({ request }) => {
    await ensureCli1();
    const pngPath = join(tmpDir, 'api-upload.png');
    const pngBuffer = generateSmallPng();
    writeFileSync(pngPath, pngBuffer);

    // API 上传（发消息注册 → multipart 上传文件实体）
    apiAttKey = await apiUploadAttachment(request, 'g1', {
      name: 'api-upload.png', mimeType: 'image/png', buffer: pngBuffer,
    });

    // client-cli 收到 AttachmentInfoResponse
    await cli1.waitForOutput(/<< \[admin:g1\].*api-upload\.png/, 10000);

    // client-cli 经 channelmanager 的附件下载协议拉取文件
    cli1.stdin(`/download ${apiAttKey}`);
    await cli1.waitForOutput(/>> downloading .*api-upload\.png \(\d+ bytes\)/);
    await cli1.waitForOutput(/>> downloaded to .*api-upload\.png/);

    // 验证下载文件内容与上传一致
    const saved = readFileSync(join(WORKSPACE, 'downloads', 'api-upload.png'));
    expect(saved.equals(pngBuffer)).toBe(true);
  });

  // ================================================================
  // TC-05 client-cli 上传图片附件 → API 下载（附件上行链路）
  // ================================================================
  test('TC-05: client-cli 上传图片附件 → API 下载', async ({ request }) => {
    await ensureCli1();
    const pngPath = join(tmpDir, 'cli-upload.png');
    const pngBuffer = generateSmallPng();
    writeFileSync(pngPath, pngBuffer);

    cli1.stdin(`/upload ${pngPath}`);
    const cliOutput = await cli1.waitForOutput(/>> uploaded cli-upload\.png key=(\S+)/);
    const keyMatch = cliOutput.match(/key=(\S+)/);
    if (keyMatch) {
      cliAttKey = keyMatch[1];
    }

    // 上行：channelclient → channelmanager → messenger → 存储，附件消息出现在历史中
    const msg = await waitForRecentMessage(
      request,
      'g1',
      (m: any) => m.content?.msg_type === 'AttachmentInfoResponse'
        && m.content?.data?.info?.file_name === 'cli-upload.png',
    );
    const key = msg.content.data.key;
    expect(key).toBeTruthy();
    cliAttKey = key;

    // API 下载并验证内容一致
    const resp = await request.get(`${BASE}/api/attachment/download?key=${cliAttKey}`, {
      headers: { 'X-Api-Key': API_KEY },
    });
    const body = await resp.body();
    expect(body.equals(pngBuffer)).toBe(true);
  });

  // ================================================================
  // TC-06 API 上传文件附件 → client-cli 下载
  // ================================================================
  test('TC-06: API 上传文件附件 → client-cli 下载', async ({ request }) => {
    await ensureCli1();
    const txtBuffer = generateTextFile('This is a test document from API.');
    const txtPath = join(tmpDir, 'api-upload.txt');
    writeFileSync(txtPath, txtBuffer);

    apiAttKey = await apiUploadAttachment(request, 'g1', {
      name: 'api-upload.txt', mimeType: 'text/plain', buffer: txtBuffer,
    });

    await cli1.waitForOutput(/<< \[admin:g1\].*api-upload\.txt/, 10000);

    cli1.stdin(`/download ${apiAttKey}`);
    await cli1.waitForOutput(/>> downloading .*api-upload\.txt \(\d+ bytes\)/);
    await cli1.waitForOutput(/>> downloaded to .*api-upload\.txt/);

    const saved = readFileSync(join(WORKSPACE, 'downloads', 'api-upload.txt'));
    expect(saved.equals(txtBuffer)).toBe(true);
  });

  // ================================================================
  // TC-07 client-cli 上传文件附件 → API 下载
  // ================================================================
  test('TC-07: client-cli 上传文件附件 → API 下载', async ({ request }) => {
    await ensureCli1();
    const txtBuffer = generateTextFile('Hello from client-cli!');
    const txtPath = join(tmpDir, 'cli-upload.txt');
    writeFileSync(txtPath, txtBuffer);

    cli1.stdin(`/upload ${txtPath}`);
    await cli1.waitForOutput(/>> uploaded cli-upload\.txt key=/);

    const msg = await waitForRecentMessage(
      request,
      'g1',
      (m: any) => m.content?.msg_type === 'AttachmentInfoResponse'
        && m.content?.data?.info?.file_name === 'cli-upload.txt',
    );
    cliAttKey = msg.content.data.key;

    const resp = await request.get(`${BASE}/api/attachment/download?key=${cliAttKey}`, {
      headers: { 'X-Api-Key': API_KEY },
    });
    const body = await resp.body();
    expect(body.equals(txtBuffer)).toBe(true);
  });

  // ================================================================
  // TC-08 群组管理 → client-cli 收到 leave/join 通知（channelmanager 通知下发）
  // ================================================================
  test('TC-08: 群组管理 → client-cli 收到 leave/join 通知', async ({ request }) => {
    // 移除 u2 出 g2 → cli2 收到 leave
    const r1 = await apiPost(request, '/api/groups/manage-members', {
      group_id: 'g2', add_ids: [], remove_ids: ['u2'],
    });
    expect(r1.success).toBe(true);
    await cli2.waitForOutput(/<< leave group: g2 @ web/, 10000);

    // 添加 u2 回 g2 → cli2 收到 join
    const r2 = await apiPost(request, '/api/groups/manage-members', {
      group_id: 'g2', add_ids: ['u2'], remove_ids: [],
    });
    expect(r2.success).toBe(true);
    await cli2.waitForOutput(/<< join group: g2 @ web/, 10000);
  });

  // ================================================================
  // TC-09 admin-user 单聊群组路由（仅 u1 收到，u2 不收）
  // ================================================================
  test('TC-09: admin-user 单聊群组消息按用户路由', async ({ request }) => {
    const resp = await apiPost(request, '/api/message/send', {
      messenger_id: 'web', user_id: 'admin', group_id: 'a_u1',
      content: { msg_type: 'Text', data: '单聊路由测试' },
    });
    expect(resp.success).toBe(true);

    // u1 收到单聊消息
    await cli1.waitForOutput(/<< \[admin:a_u1\].*单聊路由测试/, 10000);

    // u2 的绑定连接不应收到（channelmanager 按接收用户分发）
    await new Promise(r => setTimeout(r, 2000));
    expect(cli2.hasOutput(/单聊路由测试/)).toBe(false);
  });

});
