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

test.describe.serial('agent 管理命令测试（channel 级 admin + pipeline 命令，cli 经 channel-web 发送）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
    // 等待 agent 装配 pipeline、连接 channel 并绑定 input identity
    await new Promise((r) => setTimeout(r, 2000));
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

  test('TC-01: 管理员（u2）发送 /admin web u3 添加管理权限', async () => {
    cliAdmin.stdin('/send /admin web u3');
    await cliAdmin.waitForOutput(/✅ 已添加管理权限: web \/ u3/, 10000);
  });

  test('TC-02: 管理员（u2）发送 /unadmin web u3 移除权限', async () => {
    cliAdmin.stdin('/send /unadmin web u3');
    await cliAdmin.waitForOutput(/✅ 已移除管理权限: web \/ u3/, 10000);
  });

  test('TC-03: 管理员发送 /mode event 进入事件模式（pipeline 命令，自动生成事件 ID）', async () => {
    cliAdmin.stdin('/send /mode event');
    await cliAdmin.waitForOutput(/✅ 新事件 ID: [0-9a-f-]{36}/, 10000);
  });

  test('TC-04: /model 参数个数不符报格式错误（pipeline 命令）', async () => {
    cliAdmin.stdin('/send /model deepseek deepseek-flash maybe');
    await cliAdmin.waitForOutput(/⚠️ 格式: \/model <provider> <model>/, 10000);
  });

  test('TC-05: 已移除的管理命令不再产生命令回复', async () => {
    // /agent、/role、/bind、/unbind 等命令已删除：pipeline 不认识，按普通消息处理
    const baseline = cliAdmin.getOutput();
    cliAdmin.stdin('/send /agent a1 r1');
    await new Promise((r) => setTimeout(r, 2000));
    expect(cliAdmin.getOutput().slice(baseline.length)).not.toMatch(/✅ 已设置 agent/);
  });
});
