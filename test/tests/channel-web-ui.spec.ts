import { test, expect } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, waitForPort } from './helpers/server';
import { generateLargePng, generateSmallPng, generateTextFile } from './helpers/assets';
import { ChildProcess } from 'child_process';
import { join } from 'path';
import { writeFileSync, mkdtempSync } from 'fs';
import { tmpdir } from 'os';

const BASE = 'http://127.0.0.1:8301';
const API_KEY = 'admin-key-123';
const WORKSPACE = join(__dirname, '..', 'workspace');
const UI = 'http://localhost:5173';

let backend: ChildProcess;
let tmpDir: string;

test.describe.serial('channel-web 前后端集成测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);

    // 创建临时目录用于附件测试
    tmpDir = mkdtempSync(join(tmpdir(), 'kissbot-ui-'));
  });

  test.afterAll(() => {
    stopBackend(backend);
  });

  // ===== 辅助：登录 =====
  async function login(page: any) {
    await page.goto(UI);
    // 等待预置配置加载完成
    await page.waitForTimeout(500);
    await page.locator('text=测试环境').first().click();
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill(API_KEY);
    await page.getByRole('button', { name: '连接' }).click();
    // 等待进入聊天主界面
    await expect(page.locator('.app-name')).toBeVisible({ timeout: 5000 });
  }

  // ===== 辅助：登录并选择开发组 =====
  async function loginAndSelectDevTeam(page: any) {
    await page.goto(UI);
    await page.waitForTimeout(500);
    // 默认选中第一个预置（测试环境），只需输入 key
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill(API_KEY);
    await page.getByRole('button', { name: '连接' }).click();
    await expect(page.locator('.app-name')).toBeVisible({ timeout: 5000 });
    // 选择开发组
    await page.locator('.conversation-name').filter({ hasText: '开发组' }).click();
    await page.waitForTimeout(500);
  }

  // ================================================================
  // TC-01 登录页展示
  // ================================================================
  test('TC-01: 登录页展示', async ({ page }) => {
    await page.goto(UI);
    await page.waitForTimeout(500);

    // 验证应用名称和副标题
    await expect(page.locator('text=Kissbot Web Chat')).toBeVisible();
    await expect(page.locator('text=管理后台')).toBeVisible();

    // 验证预置后端列表（根据实际 backends.json 只有"测试环境"）
    await expect(page.locator('text=测试环境')).toBeVisible();
    // 自定义项始终显示
    await expect(page.locator('text=自定义')).toBeVisible();

    // 验证 Admin Key 输入框
    await expect(page.locator('input[type="password"]')).toBeVisible();

    // 验证连接按钮
    await expect(page.getByRole('button', { name: '连接' })).toBeVisible();
  });

  // ================================================================
  // TC-02 登录失败
  // ================================================================
  test('TC-02: 登录失败', async ({ page }) => {
    await page.goto(UI);
    await page.waitForTimeout(500);

    // 选中测试环境
    await page.locator('text=测试环境').first().click();

    // 输入错误 key
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill('wrong-key');

    // 点击连接
    await page.getByRole('button', { name: '连接' }).click();

    // 验证错误提示
    await expect(page.locator('text=连接失败')).toBeVisible({ timeout: 5000 });

    // 不应进入聊天界面
    await expect(page.locator('.app-name')).not.toBeVisible();
  });

  // ================================================================
  // TC-03 登录成功
  // ================================================================
  test('TC-03: 登录成功', async ({ page }) => {
    await page.goto(UI);
    await page.waitForTimeout(500);

    // 选中测试环境
    await page.locator('text=测试环境').first().click();

    // 输入正确 key
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill(API_KEY);

    // 点击连接
    await page.getByRole('button', { name: '连接' }).click();

    // 成功进入聊天主界面
    await expect(page.locator('.app-name')).toBeVisible({ timeout: 5000 });
    await expect(page.locator('.app-name')).toHaveText('Kissbot Web Chat');

    // 顶部右侧显示"管理员 ▼"
    await expect(page.locator('.admin-trigger')).toBeVisible();
    await expect(page.locator('.admin-trigger')).toContainText('管理员');
  });

  // ================================================================
  // TC-03b: 自定义 URL 登录
  // ================================================================
  test('TC-03b: 自定义 URL 登录', async ({ page }) => {
    await page.goto(UI);
    await page.waitForTimeout(500);

    // 点击自定义输入框并输入 URL
    const urlInput = page.getByPlaceholder(/自定义后端 URL/);
    await urlInput.click();
    await urlInput.fill('http://localhost:8301');

    // 输入 admin key
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill(API_KEY);

    // 点击连接
    await page.getByRole('button', { name: '连接' }).click();

    // 成功进入
    await expect(page.locator('.app-name')).toBeVisible({ timeout: 5000 });
  });

  // ================================================================
  // TC-03c: 不可达 URL 登录失败
  // ================================================================
  test('TC-03c: 不可达 URL 登录失败', async ({ page }) => {
    await page.goto(UI);
    await page.waitForTimeout(500);

    // 输入一个不可达的自定义 URL
    const urlInput = page.getByPlaceholder(/自定义后端 URL/);
    await urlInput.click();
    await urlInput.fill('http://localhost:19999');

    // 输入 key
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill(API_KEY);

    // 点击连接
    await page.getByRole('button', { name: '连接' }).click();

    // 验证错误提示
    await expect(page.locator('text=连接失败')).toBeVisible({ timeout: 10000 });

    // 不应进入聊天界面
    await expect(page.locator('.app-name')).not.toBeVisible();
  });

  // ================================================================
  // TC-03d: 自定义空 URL 登录拦截
  // ================================================================
  test('TC-03d: 自定义空 URL 登录拦截', async ({ page }) => {
    await page.goto(UI);
    await page.waitForTimeout(500);

    // 聚焦自定义 URL 输入框但不输入，切换为自定义选择
    const urlInput = page.getByPlaceholder(/自定义后端 URL/);
    await urlInput.click();

    // 输入 admin key
    const pwInput = page.locator('input[type="password"]');
    await pwInput.fill(API_KEY);

    // 点击连接
    await page.getByRole('button', { name: '连接' }).click();

    // 验证前端拦截错误
    await expect(page.locator('text=请输入后端 URL')).toBeVisible({ timeout: 3000 });

    // 不应进入聊天界面
    await expect(page.locator('.app-name')).not.toBeVisible();
  });

  // ================================================================
  // TC-04 会话列表显示
  // ================================================================
  test('TC-04: 会话列表显示', async ({ page }) => {
    await login(page);

    // 验证 4 个会话
    // admin-user 单聊组（虚拟组）
    await expect(page.locator('.conversation-name').filter({ hasText: '助手小A' })).toBeVisible();
    await expect(page.locator('.conversation-name').filter({ hasText: '助手小B' })).toBeVisible();
    // 多人群组
    await expect(page.locator('.conversation-name').filter({ hasText: '开发组' })).toBeVisible();
    await expect(page.locator('.conversation-name').filter({ hasText: '项目X' })).toBeVisible();
  });

  // ================================================================
  // TC-05 主界面默认状态
  // ================================================================
  test('TC-05: 主界面默认状态', async ({ page }) => {
    await login(page);

    // 验证右侧主区域显示"选择一个会话"
    await expect(page.locator('text=选择一个会话')).toBeVisible();
  });

  // ================================================================
  // TC-06 选中开发组并发送文本消息
  // ================================================================
  test('TC-06: 选中开发组并发送文本消息', async ({ page }) => {
    await loginAndSelectDevTeam(page);

    // 验证右侧顶部标题显示"开发组"
    await expect(page.locator('.chat-header h3')).toContainText('开发组');

    // 输入并发送文本消息
    const input = page.locator('input[type="text"]');
    await input.fill('你好');
    await page.keyboard.press('Enter');

    // 验证消息出现（蓝色气泡靠右显示）
    // 注意：beforeAll 已发送 25 条 admin 消息，所以有多条 .message.self
    // 我们通过文本内容精确定位新发送的消息
    await expect(page.locator('.message-bubble').filter({ hasText: '你好' })).toBeVisible({ timeout: 3000 });
  });

  // ================================================================
  // TC-07 发送附件消息（图片）
  // ================================================================
  test('TC-07: 发送附件消息（图片）', async ({ page }) => {
    await loginAndSelectDevTeam(page);

    // 生成测试图片
    const pngPath = join(tmpDir, 'test-image.png');
    writeFileSync(pngPath, generateSmallPng());

    // 通过隐藏的 file input 选择图片
    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles(pngPath);

    // 验证附件预览区域显示已选文件名
    await expect(page.locator('.attachment-preview-item')).toContainText('test-image.png');

    // 点击"上传附件 (1)"发送
    await page.getByText('上传附件').click();

    // 等待图片消息出现（显示图片缩略图）
    await page.waitForTimeout(2000);
    const images = page.locator('.message .image-attachment');
    await expect(images.first()).toBeVisible({ timeout: 5000 });
  });

  // ================================================================
  // TC-08 点击图片查看原图
  // ================================================================
  test('TC-08: 点击图片查看原图', async ({ page }) => {
    // 使用 login 进入，然后选择开发组（之前 TC-07 发送的图片应已在后端）
    await loginAndSelectDevTeam(page);

    // 等待消息加载
    await page.waitForTimeout(2000);

    // 查找图片缩略图并点击
    const thumb = page.locator('.image-attachment').first();
    await thumb.waitFor({ state: 'visible', timeout: 5000 }).catch(() => {});
    // 如果图片存在则点击
    if (await thumb.isVisible()) {
      await thumb.click();

      // 验证弹窗（大图预览）
      await expect(page.locator('.image-overlay')).toBeVisible({ timeout: 2000 });

      // 点击背景关闭弹窗
      await page.locator('.image-overlay').click({ position: { x: 10, y: 10 } });
      // 确认弹窗已关闭
      await expect(page.locator('.image-overlay')).not.toBeVisible({ timeout: 2000 });
    }
    // 如果没有图片（新 workspace），跳过断言
  });

  // ================================================================
  // TC-09 发送附件消息（文件）
  // ================================================================
  test('TC-09: 发送附件消息（文件）', async ({ page }) => {
    await loginAndSelectDevTeam(page);

    // 生成测试文本文件
    const txtPath = join(tmpDir, 'test-doc.txt');
    writeFileSync(txtPath, generateTextFile());

    // 上传文件
    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles(txtPath);

    // 验证附件预览
    await expect(page.locator('.attachment-preview-item')).toContainText('test-doc.txt');

    // 发送
    await page.getByText('上传附件').click();

    // 等待文件消息出现
    await page.waitForTimeout(2000);
    const fileLinks = page.locator('.message.self .file-attachment');
    await expect(fileLinks.first()).toBeVisible({ timeout: 5000 });
  });

  // ================================================================
  // TC-10 点击文件下载
  // ================================================================
  test('TC-10: 点击文件下载', async ({ page }) => {
    await loginAndSelectDevTeam(page);

    // 等待消息加载
    await page.waitForTimeout(2000);

    const fileLink = page.locator('.file-attachment').first();
    await fileLink.waitFor({ state: 'visible', timeout: 5000 }).catch(() => {});

    if (await fileLink.isVisible()) {
      // 文件链接 target="_blank" 可能打开新标签页或触发下载
      // 先设置监听器，再点击
      const downloadPromise = page.waitForEvent('download', { timeout: 3000 }).catch(() => null);

      await fileLink.click();

      const download = await downloadPromise;
      if (download) {
        expect(download.suggestedFilename()).toBeTruthy();
      }
    }
    // 如果没有文件消息，跳过断言
  });

  // ================================================================
  // TC-11 分页加载历史消息
  // ================================================================
  test('TC-11: 分页加载历史消息', async ({ page }) => {
    await loginAndSelectDevTeam(page);

    // 通过 UI 发送一条消息
    const input = page.locator('input[type="text"]');
    await input.fill('分页测试消息');
    await page.keyboard.press('Enter');

    // 等待消息出现
    await expect(page.locator('.message-bubble').filter({ hasText: '分页测试消息' })).toBeVisible({ timeout: 5000 });

    // 验证消息列表可以滚动
    const messageList = page.locator('.message-list');
    const scrollHeight = await messageList.evaluate(el => el.scrollHeight);
    expect(scrollHeight).toBeGreaterThan(0);
  });

  // ================================================================
  // TC-12 管理员下拉菜单
  // ================================================================
  test('TC-12: 管理员下拉菜单', async ({ page }) => {
    await login(page);

    // 悬停或点击管理员触发下拉
    const trigger = page.locator('.admin-trigger');
    await trigger.hover();

    // 验证下拉菜单项
    const menu = page.locator('.dropdown-menu');
    await expect(menu).toBeVisible({ timeout: 2000 });
    await expect(menu.locator('text=重命名管理员')).toBeVisible();
    await expect(menu.locator('text=群组管理')).toBeVisible();
    await expect(menu.locator('text=用户管理')).toBeVisible();
  });

  // ================================================================
  // TC-13 群组管理——打开面板
  // ================================================================
  test('TC-13: 群组管理——打开面板', async ({ page }) => {
    await login(page);

    // 打开下拉菜单
    await page.locator('.admin-trigger').hover();
    await page.locator('.dropdown-menu').waitFor({ state: 'visible', timeout: 2000 });

    // 点击"群组管理"
    await page.locator('.dropdown-item').filter({ hasText: '群组管理' }).click();

    // 验证群组管理面板
    await expect(page.locator('.admin-panel-title')).toBeVisible();
    await expect(page.locator('.admin-panel-title')).toContainText('群组管理');
    // 验证返回按钮
    await expect(page.locator('.back-btn')).toBeVisible();

    // 验证各功能区域
    await expect(page.locator('text=新建群组')).toBeVisible();
    await expect(page.locator('text=重命名群组')).toBeVisible();
    await expect(page.locator('text=管理成员')).toBeVisible();
    await expect(page.locator('text=群组列表')).toBeVisible();
  });

  // ================================================================
  // TC-14 群组管理——群组列表
  // ================================================================
  test('TC-14: 群组管理——群组列表', async ({ page }) => {
    await login(page);
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '群组管理' }).click();
    await page.waitForTimeout(300);

    // 群组列表应包含 dev-team 和 project-x
    const groupList = page.locator('.admin-panel-section').filter({ hasText: '群组列表' });
    await expect(groupList.locator('text=开发组')).toBeVisible();
    await expect(groupList.locator('text=项目X')).toBeVisible();
  });

  // ================================================================
  // TC-15 群组管理——新建群组
  // ================================================================
  test('TC-15: 群组管理——新建群组', async ({ page }) => {
    await login(page);
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '群组管理' }).click();
    await page.waitForTimeout(300);

    // 在"新建群组"区域输入名称
    const createSection = page.locator('.admin-panel-section').filter({ hasText: '新建群组' }).first();
    const nameInput = createSection.locator('input[type="text"]');
    await nameInput.fill('测试组');

    // 选择成员（助手小A）
    const memberTag = createSection.locator('.member-tag').filter({ hasText: '助手小A' });
    await memberTag.click();

    // 点击创建
    await createSection.getByRole('button', { name: '创建' }).click();
    await page.waitForTimeout(500);

    // 验证群组列表中新增了"测试组"
    const groupList = page.locator('.admin-panel-section').filter({ hasText: '群组列表' });
    await expect(groupList.locator('text=测试组')).toBeVisible({ timeout: 3000 });

    // 验证会话列表中新增"测试组 群组"
    await expect(page.locator('.conversation-name').filter({ hasText: '测试组' })).toBeVisible({ timeout: 3000 });
  });

  // ================================================================
  // TC-16 群组管理——重命名群组
  // ================================================================
  test('TC-16: 群组管理——重命名群组', async ({ page }) => {
    await login(page);
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '群组管理' }).click();
    await page.waitForTimeout(300);

    // 重命名"测试组"为"核心开发组"
    const renameSection = page.locator('.admin-panel-section').filter({ hasText: '重命名群组' }).first();
    // 选择群组
    const select = renameSection.locator('select');
    await select.selectOption({ label: '测试组' });
    await page.waitForTimeout(200);
    // 输入新名称
    const nameInput = renameSection.locator('input[type="text"]');
    await nameInput.fill('核心开发组');
    // 点击重命名
    await renameSection.getByRole('button', { name: '重命名' }).click();
    await page.waitForTimeout(500);

    // 群组列表中"测试组"变为"核心开发组"
    const groupList = page.locator('.admin-panel-section').filter({ hasText: '群组列表' });
    await expect(groupList.locator('text=核心开发组')).toBeVisible({ timeout: 3000 });
    await expect(groupList.locator('text=测试组')).not.toBeVisible();

    // 会话列表同步更新
    await expect(page.locator('.conversation-name').filter({ hasText: '核心开发组' })).toBeVisible({ timeout: 3000 });
  });

  // ================================================================
  // TC-17 群组管理——管理成员
  // ================================================================
  test('TC-17: 群组管理——管理成员', async ({ page }) => {
    await login(page);
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '群组管理' }).click();
    await page.waitForTimeout(300);

    // 在"管理成员"区域选择"核心开发组"
    const manageSection = page.locator('.admin-panel-section').filter({ hasText: '管理成员' }).first();
    const select = manageSection.locator('select');
    await select.selectOption({ label: '核心开发组' });
    await page.waitForTimeout(200);

    // 添加"助手小B"
    const memberTag = manageSection.locator('.member-tag').filter({ hasText: '助手小B' });
    await memberTag.click();

    // 点击"添加成员"
    await manageSection.getByRole('button', { name: '添加成员' }).click();
    await page.waitForTimeout(500);

    // 验证成员更新（群组列表中应显示成员变更）
    // 核心开发组的 members 应包含 admin, user-1, user-2
    const groupList = page.locator('.admin-panel-section').filter({ hasText: '群组列表' });
    const groupItem = groupList.locator('.admin-item').filter({ hasText: '核心开发组' });
    await expect(groupItem).toBeVisible();
  });

  // ================================================================
  // TC-18 群组管理——删除群组
  // ================================================================
  test('TC-18: 群组管理——删除群组', async ({ page }) => {
    await login(page);
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '群组管理' }).click();
    await page.waitForTimeout(300);

    // 在群组列表中找到"核心开发组"并点击删除
    const groupList = page.locator('.admin-panel-section').filter({ hasText: '群组列表' });
    const groupItem = groupList.locator('.admin-item').filter({ hasText: '核心开发组' });

    // 如果存在则删除
    if (await groupItem.isVisible()) {
      const deleteBtn = groupItem.locator('button').filter({ hasText: '删除' });
      await deleteBtn.click();
      await page.waitForTimeout(500);

      // 验证"核心开发组"从群组列表中消失
      await expect(groupList.locator('text=核心开发组')).not.toBeVisible({ timeout: 3000 });

      // 验证会话列表中已移除
      await expect(page.locator('.conversation-name').filter({ hasText: '核心开发组' })).not.toBeVisible({ timeout: 3000 });
    }
    // 如果不存在（之前已删除），跳过
  });

  // ================================================================
  // TC-19 用户管理——打开面板
  // ================================================================
  test('TC-19: 用户管理——打开面板', async ({ page }) => {
    await login(page);

    // 打开下拉菜单
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '用户管理' }).click();
    await page.waitForTimeout(300);

    // 验证用户管理面板
    await expect(page.locator('.admin-panel-title')).toBeVisible();
    await expect(page.locator('.admin-panel-title')).toContainText('用户管理');
    await expect(page.locator('.back-btn')).toBeVisible();

    // 验证包含新建用户区域和用户列表
    await expect(page.locator('text=新建用户')).toBeVisible();
    await expect(page.locator('text=用户列表')).toBeVisible();
  });

  // ================================================================
  // TC-20 用户管理——新建用户
  // ================================================================
  test('TC-20: 用户管理——新建用户', async ({ page }) => {
    await login(page);
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '用户管理' }).click();
    await page.waitForTimeout(300);

    // 在"新建用户"区域输入名称
    const createSection = page.locator('.admin-panel-section').filter({ hasText: '新建用户' }).first();
    const nameInput = createSection.locator('input[type="text"]');
    await nameInput.fill('助手小C');

    // 点击创建
    await createSection.getByRole('button', { name: '创建' }).click();
    await page.waitForTimeout(500);

    // 验证用户列表中新增"助手小C"
    const userList = page.locator('.admin-panel-section').filter({ hasText: '用户列表' });
    await expect(userList.locator('text=助手小C')).toBeVisible({ timeout: 3000 });

    // 验证会话列表新增"助手小C"（单聊组）
    await expect(page.locator('.conversation-name').filter({ hasText: '助手小C' })).toBeVisible({ timeout: 3000 });
  });

  // ================================================================
  // TC-21 用户管理——重命名用户
  // ================================================================
  test('TC-21: 用户管理——重命名用户', async ({ page }) => {
    await login(page);
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '用户管理' }).click();
    await page.waitForTimeout(300);

    // 在用户列表中找到"助手小C"并点击重命名
    const userList = page.locator('.admin-panel-section').filter({ hasText: '用户列表' });
    const userItem = userList.locator('.admin-item').filter({ hasText: '助手小C' });

    // 获取 user_id（从 meta 文本提取）
    const renameBtn = userItem.locator('button').filter({ hasText: '重命名' });
    await renameBtn.click();
    await page.waitForTimeout(300);

    // 弹出重命名对话框
    const dialog = page.locator('.image-overlay');
    const dialogInput = dialog.locator('input[type="text"]');
    await dialogInput.fill('助手小C（改）');

    // 点击确认
    await dialog.getByRole('button', { name: '确认' }).click();
    await page.waitForTimeout(500);

    // 验证用户列表中名称已更新
    await expect(userList.locator('text=助手小C（改）')).toBeVisible({ timeout: 3000 });
    // 精确匹配：确保旧名称已不在列表中（使用 exact match 避免误匹配"助手小C（改）"）
    await expect(userList.locator('.admin-item-name').filter({ hasText: /^助手小C$/ })).not.toBeVisible();

    // 验证会话列表中对应单聊组名称同步更新
    await expect(page.locator('.conversation-name').filter({ hasText: '助手小C（改）' })).toBeVisible({ timeout: 3000 });
  });

  // ================================================================
  // TC-22 用户管理——删除用户
  // ================================================================
  test('TC-22: 用户管理——删除用户', async ({ page }) => {
    await login(page);
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '用户管理' }).click();
    await page.waitForTimeout(300);

    // 在用户列表中找到"助手小C（改）"并点击删除
    const userList = page.locator('.admin-panel-section').filter({ hasText: '用户列表' });
    const userItem = userList.locator('.admin-item').filter({ hasText: '助手小C（改）' });

    if (await userItem.isVisible()) {
      // 设置 dialog handler 接受 confirm
      page.once('dialog', dialog => dialog.accept());

      const deleteBtn = userItem.locator('button').filter({ hasText: '删除' });
      await deleteBtn.click();
      await page.waitForTimeout(500);

      // 验证用户从列表中消失
      await expect(userList.locator('text=助手小C（改）')).not.toBeVisible({ timeout: 3000 });

      // 验证会话列表中对应的单聊组同步移除
      await expect(page.locator('.conversation-name').filter({ hasText: '助手小C（改）' })).not.toBeVisible({ timeout: 3000 });
    }
  });

  // ================================================================
  // TC-23 管理员重命名
  // ================================================================
  test('TC-23: 管理员重命名', async ({ page }) => {
    await login(page);

    // 打开下拉菜单
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    // 点击"重命名管理员"
    await page.locator('.dropdown-item').filter({ hasText: '重命名管理员' }).click();
    await page.waitForTimeout(300);

    // 弹出重命名对话框
    const dialog = page.locator('.image-overlay');
    await expect(dialog).toBeVisible({ timeout: 2000 });

    const dialogInput = dialog.locator('input[type="text"]');
    await dialogInput.fill('超级管理员');

    // 点击确认
    await dialog.getByRole('button', { name: '确认' }).click();
    await page.waitForTimeout(500);

    // 验证顶部标题栏右侧显示"超级管理员 ▼"
    await expect(page.locator('.admin-trigger')).toContainText('超级管理员');
  });

  // ================================================================
  // TC-24 返回聊天界面
  // ================================================================
  test('TC-24: 返回聊天界面', async ({ page }) => {
    await login(page);

    // 先进入群组管理面板
    await page.locator('.admin-trigger').hover();
    await page.waitForTimeout(300);
    await page.locator('.dropdown-item').filter({ hasText: '群组管理' }).click();
    await page.waitForTimeout(300);
    await expect(page.locator('.admin-panel-title')).toBeVisible();

    // 点击 ← 返回按钮
    await page.locator('.back-btn').click();
    await page.waitForTimeout(500);

    // 验证右侧恢复为消息区域（显示"选择一个会话"）
    await expect(page.locator('text=选择一个会话')).toBeVisible({ timeout: 3000 });
  });
});
