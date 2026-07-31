import { test, expect } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, waitForPort } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { generateSmallPng, generateTextFile } from './helpers/assets';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { join, dirname } from 'path';
import { writeFileSync, mkdtempSync, readdirSync, existsSync } from 'fs';
import { tmpdir } from 'os';

const BASE = 'http://127.0.0.1:8301';
const API_KEY = 'admin-key-123';
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');
const UI = 'http://localhost:5173';

let backend: ChildProcess;
let cli1: SpawnedCli;
let tmpDir: string;

// 测试间共享变量
let sharedAttKey = '';        // 附件 key（web→cli 方向）
let sharedCliUploadKey = '';  // 附件 key（cli→web 方向）

test.describe.serial('channel-web 与 channel-client 通信测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    tmpDir = mkdtempSync(join(tmpdir(), 'kissbot-client-'));
  });

  test.afterAll(() => {
    if (cli1) cli1.proc.kill();
    stopBackend(backend);
  });

  // ===== 辅助：如果 CLI 已断开则重启 =====
  async function ensureCli() {
    if (cli1 && (cli1.proc.exitCode !== null || cli1.proc.killed)) {
      cli1 = spawnCli(['web', 'user-1', 'dev-team', './downloads'], WORKSPACE);
      await cli1.waitForOutput(/bound\./);
    }
  }

  // ===== 辅助：登录 =====
  async function login(page: import('@playwright/test').Page) {
    await page.goto(UI);
    await page.waitForTimeout(500);
    await page.locator('text=测试环境').first().click();
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill(API_KEY);
    await page.getByRole('button', { name: '连接' }).click();
    await expect(page.locator('.app-name')).toBeVisible({ timeout: 5000 });
  }

  // ================================================================
  // TC-01 web 登录
  // ================================================================
  test('TC-01: web 登录', async ({ page }) => {
    await page.goto(UI);
    await page.waitForTimeout(500);

    // 选中测试环境
    await page.locator('text=测试环境').first().click();

    // 输入 Admin Key
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill(API_KEY);

    // 点击连接
    await page.getByRole('button', { name: '连接' }).click();

    // 验证成功进入聊天主界面
    await expect(page.locator('.app-name')).toBeVisible({ timeout: 5000 });
    await expect(page.locator('.app-name')).toHaveText('Kissbot Web Chat');

    // 验证会话列表显示 4 个会话
    await expect(page.locator('.conversation-name').filter({ hasText: '助手小A' })).toBeVisible();
    await expect(page.locator('.conversation-name').filter({ hasText: '助手小B' })).toBeVisible();
    await expect(page.locator('.conversation-name').filter({ hasText: '开发组' })).toBeVisible();
    await expect(page.locator('.conversation-name').filter({ hasText: '项目X' })).toBeVisible();
  });

  // ================================================================
  // TC-02 web → cli 发送文本消息
  // ================================================================
  test('TC-02: web → cli 发送文本消息', async ({ page }) => {
    // 启动 CLI，绑定 user-1 到 dev-team
    cli1 = spawnCli(['web', 'user-1', 'dev-team', './downloads'], WORKSPACE);
    await cli1.waitForOutput(/bound\./);

    // web 登录并选中开发组
    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(500);

    // 发送文本消息
    const input = page.locator('input[type="text"]');
    await input.fill('web-to-cli 测试消息');
    await page.keyboard.press('Enter');

    // 验证 CLI 端收到消息
    await cli1.waitForOutput(/<< \[admin:dev-team\].*web-to-cli 测试消息/);
  });

  // ================================================================
  // TC-03 cli → web 发送文本消息
  // ================================================================
  test('TC-03: cli → web 发送文本消息', async ({ page }) => {
    // 重新登录并选中开发组
    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(500);

    // 从 CLI 发送消息
    cli1.stdin('cli-to-web 测试消息');

    // 验证 CLI 输出发送确认
    await cli1.waitForOutput(/>> sent msg_id=/);

    // 验证 web 端收到灰色气泡（靠左，发送者为 user-1）
    const msg = page.locator('.message.other .message-bubble').filter({ hasText: 'cli-to-web 测试消息' });
    await expect(msg).toBeVisible({ timeout: 5000 });
  });

  // ================================================================
  // TC-04 web → cli 发送图片附件
  // ================================================================
  test('TC-04: web → cli 发送图片附件', async ({ page }) => {
    // 生成测试图片
    const pngPath = join(tmpDir, 'test-photo.png');
    writeFileSync(pngPath, generateSmallPng());

    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(500);

    // 选择图片文件（隐藏的 file input）
    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles(pngPath);

    // 验证附件预览显示文件名
    await expect(page.locator('.attachment-preview-item')).toContainText('test-photo.png');

    // 点击"上传附件"发送
    await page.getByText('上传附件').click();

    // 验证 CLI 端收到 AttachmentInfoResponse（含 key 和 transfer_id）
    const output = await cli1.waitForOutput(/<< \[admin:dev-team\] .*test-photo\.png.*/);
    const keyMatch = output.match(/"key":"([^"]+)"/);
    if (keyMatch) {
      sharedAttKey = keyMatch[1];
    }

    // 验证 web 端显示图片缩略图
    await page.waitForTimeout(2000);
    const images = page.locator('.message.self .image-attachment');
    await expect(images.first()).toBeVisible({ timeout: 5000 });
  });

  // ================================================================
  // TC-05 cli 下载 web 上传的图片
  // ================================================================
  test('TC-05: cli 下载 web 上传的图片', async () => {
    expect(sharedAttKey).toBeTruthy();

    // CLI 执行 /download
    cli1.stdin(`/download ${sharedAttKey}`);

    // 验证下载开始
    await cli1.waitForOutput(/>> downloading .*\.png \(\d+ bytes\)/);

    // 验证下载完成（接收完所有 chunk 后自动打印）
    await cli1.waitForOutput(/>> downloaded to .*test-photo\.png/);

    // 验证文件存在于 WORKSPACE/downloads/
    const downloadDir = join(WORKSPACE, 'downloads');
    const files = readdirSync(downloadDir);
    expect(files.some((f: string) => f.endsWith('.png'))).toBe(true);
  });

  // ================================================================
  // TC-06 cli → web 发送图片附件
  // ================================================================
  test('TC-06: cli → web 发送图片附件', async ({ page }) => {
    // 生成测试图片到 tmpDir
    const pngPath = join(tmpDir, 'cli-test-image.png');
    writeFileSync(pngPath, generateSmallPng());

    // CLI 执行 /upload
    cli1.stdin(`/upload ${pngPath}`);

    // 验证 CLI 上传完成输出，提取 key
    const output = await cli1.waitForOutput(/>> uploaded cli-test-image\.png key=.*/);
    const keyMatch = output.match(/key=(\S+)/);
    if (keyMatch) {
      sharedCliUploadKey = keyMatch[1];
    }

    // 等待后端 appender 缓冲落盘（1 秒缓冲 + 余量）
    await new Promise(r => setTimeout(r, 4000));

    // 登录 web 并选中开发组（消息已从后端存储加载）
    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(2000);

    // 验证 web 端显示文件附件（CLI 硬编码 mime_type 为 octet-stream，前端渲染为文件链接）
    const fileLinks = page.locator('.message.other .file-attachment');
    await expect(fileLinks.first()).toBeVisible({ timeout: 5000 });
  });

  // ================================================================
  // TC-07 web 查看 cli 上传的图片
  // ================================================================
  test('TC-07: web 查看 cli 上传的图片', async ({ page }) => {
    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(1000);

    // CLI 上传的 mime_type 为 octet-stream，前端渲染为文件链接
    // 点击文件链接触发浏览器下载
    const fileLink = page.locator('.message.other .file-attachment').first();
    await expect(fileLink).toBeVisible({ timeout: 5000 });

    // 尝试触发下载（Playwright 的 download 事件）
    const downloadPromise = page.waitForEvent('download', { timeout: 5000 }).catch(() => null);
    await fileLink.click();
    const download = await downloadPromise;
    if (download) {
      expect(download.suggestedFilename()).toBeTruthy();
    }
  });

  // ================================================================
  // TC-08 web 下载 cli 上传的图片
  // ================================================================
  test('TC-08: web 下载 cli 上传的图片', async ({ request }) => {
    expect(sharedCliUploadKey).toBeTruthy();

    // 通过 API 下载并验证为 PNG
    const resp = await request.get(`${BASE}/api/attachment/download?key=${sharedCliUploadKey}`, {
      headers: { 'X-Api-Key': API_KEY },
    });
    const body = await resp.body();
    expect(body[0]).toBe(0x89); // PNG signature byte 1
    expect(body[1]).toBe(0x50); // PNG signature byte 2
  });

  // ================================================================
  // TC-09 web → cli 发送文件附件
  // ================================================================
  test('TC-09: web → cli 发送文件附件', async ({ page }) => {
    // CLI 可能已断开，重启
    await ensureCli();
    const txtPath = join(tmpDir, 'test-document.txt');
    writeFileSync(txtPath, generateTextFile('This is a test document from web.'));

    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(500);

    // 选择文件
    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles(txtPath);

    // 验证附件预览
    await expect(page.locator('.attachment-preview-item')).toContainText('test-document.txt');

    // 发送
    await page.getByText('上传附件').click();

    // 验证 CLI 收到 AttachmentInfoResponse（用 file_name 区分 TC-04 图片的行）
    const output = await cli1.waitForOutput(/<< \[admin:dev-team\] .*test-document\.txt.*/);
    const keyMatch = output.match(/"key":"([^"]+)"/);
    if (keyMatch) {
      sharedAttKey = keyMatch[1]; // 用于 TC-10
    }

    // 验证 web 端显示文件链接
    await page.waitForTimeout(2000);
    const fileLinks = page.locator('.message.self .file-attachment');
    await expect(fileLinks.first()).toBeVisible({ timeout: 5000 });
  });

  // ================================================================
  // TC-10 cli 下载 web 上传的文件
  // ================================================================
  test('TC-10: cli 下载 web 上传的文件', async () => {
    expect(sharedAttKey).toBeTruthy();

    cli1.stdin(`/download ${sharedAttKey}`);

    await cli1.waitForOutput(/>> downloading .*\.txt \(\d+ bytes\)/);
    await cli1.waitForOutput(/>> downloaded to .*test-document\.txt/);

    const downloadDir = join(WORKSPACE, 'downloads');
    const files = readdirSync(downloadDir);
    expect(files.some((f: string) => f.endsWith('.txt'))).toBe(true);
  });

  // ================================================================
  // TC-11 cli → web 发送文件附件
  // ================================================================
  test('TC-11: cli → web 发送文件附件', async ({ page }) => {
    await ensureCli();
    const txtPath = join(tmpDir, 'cli-test-file.txt');
    writeFileSync(txtPath, generateTextFile('Hello from CLI!'));

    cli1.stdin(`/upload ${txtPath}`);

    const output = await cli1.waitForOutput(/>> uploaded cli-test-file\.txt key=.*/);
    const keyMatch = output.match(/key=(\S+)/);
    if (keyMatch) {
      sharedCliUploadKey = keyMatch[1]; // 用于 TC-12
    }

    // 等待后端 appender 缓冲落盘
    await new Promise(r => setTimeout(r, 4000));

    // 登录 web 并选中开发组
    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(2000);

    // 验证 web 端显示文件链接（other 侧，来自 CLI）
    const fileLinks = page.locator('.message.other .file-attachment');
    await expect(fileLinks.first()).toBeVisible({ timeout: 5000 });
  });

  // ================================================================
  // TC-12 web 下载 cli 上传的文件
  // ================================================================
  test('TC-12: web 下载 cli 上传的文件', async ({ request }) => {
    expect(sharedCliUploadKey).toBeTruthy();

    // 通过 API 下载
    const resp = await request.get(`${BASE}/api/attachment/download?key=${sharedCliUploadKey}`, {
      headers: { 'X-Api-Key': API_KEY },
    });
    expect(resp.ok()).toBe(true);

    // 验证文件内容
    const text = await resp.text();
    expect(text).toContain('Hello from CLI!');
  });

  // ================================================================
  // TC-13 群组管理添加成员 → cli user-2 收到 JoinGroup 通知
  // ================================================================
  test('TC-13: 群组管理添加成员 → cli 收到 JoinGroup 通知', async ({ page }) => {
    // 启动 user-2 的 CLI，绑定到 project-x（待会会收到 dev-team join 通知）
    const cli2 = spawnCli(['web', 'user-2', 'project-x', './downloads'], WORKSPACE);
    await cli2.waitForOutput(/bound\./);

    // web 登录
    await login(page);

    // 打开管理员下拉 → 群组管理
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '群组管理' }).click();
    await page.waitForTimeout(500);

    // 在"管理成员"区域选择"开发组"（dev-team）
    const manageSection = page.locator('.admin-panel-section').filter({ hasText: '管理成员' }).first();
    const select = manageSection.locator('select');
    await select.selectOption({ label: '开发组' });
    await page.waitForTimeout(200);

    // 点击"助手小B"（user-2）member-tag 添加
    const memberTag = manageSection.locator('.member-tag').filter({ hasText: '助手小B' });
    await memberTag.click();

    // 点击"添加成员"
    await manageSection.getByRole('button', { name: '添加成员' }).click();
    await page.waitForTimeout(500);

    // 验证 cli2（user-2）收到 join group 通知
    await cli2.waitForOutput(/<< join group: dev-team @ web/);

    // 清理 cli2
    cli2.proc.kill();
  });

  // ================================================================
  // TC-14 刷新页面重新登录 → 历史消息持久化
  // ================================================================
  test('TC-14: 刷新页面重新登录 → 历史消息持久化', async ({ page }) => {
    // 等待消息落盘缓冲（后端 appender 1 秒缓冲 + 余量）
    await new Promise(r => setTimeout(r, 4000));

    // 重新登录
    await login(page);

    // 选中开发组（触发 loadMessages）
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(2000);

    // 验证历史文本消息存在（TC-02 和 TC-03 的消息）
    await expect(page.locator('.message-bubble').filter({ hasText: 'web-to-cli 测试消息' })).toBeVisible();
    await expect(page.locator('.message-bubble').filter({ hasText: 'cli-to-web 测试消息' })).toBeVisible();

    // 验证消息列表不为空（图片/文件消息也包含在内）
    const msgCount = await page.locator('.message-list .message').count();
    expect(msgCount).toBeGreaterThan(0);
  });

  // ================================================================
  // TC-15 杀后端 → 等待 → 起后端 → cli 发消息 → web 自动重连
  // ================================================================
  test('TC-15: web 端 SSE 断线重连', async ({ page }) => {
    // 登录并选中开发组
    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(500);

    // 杀掉后端
    stopBackend(backend);
    await new Promise(r => setTimeout(r, 5000));

    // 重启后端
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);

    // 重启 CLI（旧进程已断连退出）
    if (cli1) cli1.proc.kill();
    cli1 = spawnCli(['web', 'user-1', 'dev-team', './downloads'], WORKSPACE);
    await cli1.waitForOutput(/bound\./);

    // CLI 发消息
    cli1.stdin('reconnect 测试');

    // 验证 CLI 输出发送确认
    await cli1.waitForOutput(/>> sent msg_id=/);

    // 验证 web 页面出现该消息（SSE 自动重连后收到）
    const msg = page.locator('.message-bubble').filter({ hasText: 'reconnect 测试' }).last();
    await expect(msg).toBeVisible({ timeout: 15000 });
  });

  // ================================================================
  // TC-16 心跳保活——长时间空闲后仍可收发（heartbeat interval=5s, timeout=15s）
  // ================================================================
  test('TC-16: 心跳保活——长时间空闲连接不中断', async ({ page, }) => {
    test.setTimeout(35000);
    await login(page);
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(500);

    // 清理旧 CLI 连接，重新绑定
    if (cli1) cli1.proc.kill();
    cli1 = spawnCli(['web', 'user-1', 'dev-team', './downloads'], WORKSPACE);
    await cli1.waitForOutput(/bound\./);

    // 先发一条消息确认连通
    cli1.stdin('心跳前测试');
    await cli1.waitForOutput(/>> sent msg_id=/);
    await expect(page.locator('.message-bubble').filter({ hasText: '心跳前测试' }).last())
      .toBeVisible({ timeout: 5000 });

    // 等待 20 秒（> 15s 心跳超时，余量 5s），期间无任何用户交互
    // 若心跳有 bug，连接将断开，CLI 进程退出
    await new Promise(r => setTimeout(r, 20000));

    // 验证 CLI 进程仍在运行
    expect(cli1.proc.exitCode).toBeNull();

    // 再发一条消息验证连接仍然畅通
    cli1.stdin('心跳后测试');
    await cli1.waitForOutput(/>> sent msg_id=/);

    // web 页面也应收到该消息
    const afterMsg = page.locator('.message-bubble').filter({ hasText: '心跳后测试' }).last();
    await expect(afterMsg).toBeVisible({ timeout: 10000 });
  });

});
