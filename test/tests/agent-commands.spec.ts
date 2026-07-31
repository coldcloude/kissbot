import { test, expect } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, startAgent, stopAgent, waitForPort } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let backend: ChildProcess;
let agent: ChildProcess;
let cliAdmin: SpawnedCli;   // u2：初始管理员
let cliUser: SpawnedCli;    // u3：admin/unadmin 测试对象

// 等待 cli 输出（返回 Promise，用于"不应出现"的断言配合超时）
function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

test.describe.serial('agent 管理命令测试（/admin 与 /model，cli 经 channel-web 发送）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
    // 等待 agent 完成 channel 连接与绑定
    await sleep(2000);
    cliAdmin = spawnCli(['web', 'u2', 'g1', './downloads'], WORKSPACE);
    await cliAdmin.waitForOutput(/bound\./);
    cliUser = spawnCli(['web', 'u3', 'g1', './downloads'], WORKSPACE);
    await cliUser.waitForOutput(/bound\./);
  });

  test.afterAll(() => {
    if (cliAdmin) cliAdmin.proc.kill();
    if (cliUser) cliUser.proc.kill();
    stopAgent(agent);
    stopBackend(backend);
  });

  test('TC-01: 非管理员（u3）发送 /model 被忽略', async () => {
    // 记录发送前的输出基线，只断言基线之后没有 agent 回复
    const baseline = cliUser.getOutput();
    cliUser.stdin('/send /model deepseek deepseek-4-flash');
    // 等待一段时间确认没有 agent 回复
    await sleep(3000);
    const tail = cliUser.getOutput().slice(baseline.length);
    expect(tail).not.toMatch(/切换模型|模型调用失败|不存在/);
  });

  test('TC-02: 管理员（u2）发送 /admin web u3 添加管理权限', async () => {
    cliAdmin.stdin('/send /admin web u3');
    await cliAdmin.waitForOutput(/✅ 已添加管理权限: web \/ u3/, 10000);
  });

  test('TC-03: u3 成为管理员后发送 /model 生效', async () => {
    cliUser.stdin('/send /model deepseek deepseek-4-flash');
    await cliUser.waitForOutput(/✅ 已切换模型为: deepseek\/deepseek-4-flash/, 15000);
  });

  test('TC-04: 管理员（u2）发送 /unadmin web u3 移除权限', async () => {
    cliAdmin.stdin('/send /unadmin web u3');
    await cliAdmin.waitForOutput(/✅ 已移除管理权限: web \/ u3/, 10000);
  });

  test('TC-05: 移除权限后 u3 发送 /model 再被忽略', async () => {
    const baseline = cliUser.getOutput();
    cliUser.stdin('/send /model deepseek deepseek-4-flash');
    await sleep(3000);
    const tail = cliUser.getOutput().slice(baseline.length);
    expect(tail).not.toMatch(/切换模型/);
  });
});
