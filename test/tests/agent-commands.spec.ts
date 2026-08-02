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

test.describe.serial('agent 管理命令测试（多会话路由，cli 经 channel-web 发送）', () => {

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

  test('TC-02: 管理员（u2）发送 /agent a1 r1 设置 channel 的 agent 与 role', async () => {
    cliAdmin.stdin('/send /agent a1 r1');
    await cliAdmin.waitForOutput(/✅ 已设置 agent: a1 \/ role: r1/, 10000);
  });

  test('TC-03: 管理员（u2）发送 /admin web u3 添加管理权限', async () => {
    cliAdmin.stdin('/send /admin web u3');
    await cliAdmin.waitForOutput(/✅ 已添加管理权限: web \/ u3/, 10000);
  });

  test('TC-05: 管理员（u2）发送 /role r2 修改 channel 角色（回写并重定位会话）', async () => {
    cliAdmin.stdin('/send /role r2');
    await cliAdmin.waitForOutput(/✅ 已设置 role: r2/, 10000);
  });

  test('TC-06: 管理员（u2）发送 /mode event 进入事件模式（自动生成事件 ID）', async () => {
    cliAdmin.stdin('/send /mode event');
    await cliAdmin.waitForOutput(/✅ 新事件 ID: [0-9a-f-]{36}/, 10000);
  });

  test('TC-07: 管理员（u2）发送 /send-channel on/off 切换发送 channel（回写）', async () => {
    cliAdmin.stdin('/send /send-channel on');
    await cliAdmin.waitForOutput(/✅ 已设为发送 channel/, 10000);
    cliAdmin.stdin('/send /send-channel off');
    await cliAdmin.waitForOutput(/✅ 已取消发送 channel/, 10000);
  });

  test('TC-08: /unbind 暂不操作，回复提示', async () => {
    cliAdmin.stdin('/send /unbind messenger web');
    await cliAdmin.waitForOutput(/ℹ️ \/unbind 暂不支持/, 10000);
  });

  test('TC-09: 无参 /agent 挂载保留 agent（空 agent_name；无模型态下普通消息静默忽略，管理命令照常回复）', async () => {
    cliAdmin.stdin('/send /agent');
    await cliAdmin.waitForOutput(/✅ 已设置 agent:  \/ role: /, 10000);
    // 注意：无参 /agent 挂载保留 agent（空 agent_name → agent_id="0"；agent_name="0" 已是普通代号），测试环境无 api_key → 无模型态
    // 无模型态下普通消息被静默忽略：不进入 agentic loop，也不产生任何 agent 回复
    // 注意：channel 会把发送消息回显给群组成员（<< [u2:g1] ...hello），因此不能断言发送文本本身
    // 语义是 agent 未进入 agentic loop：无"模型调用失败"回复（若进入 loop，api_key 为空必然报错）
    const baseline = cliAdmin.getOutput();
    cliAdmin.stdin('/send hello');
    await sleep(3000);
    const tail = cliAdmin.getOutput().slice(baseline.length);
    expect(tail).not.toMatch(/模型调用失败/);
    // 挂载态下管理命令仍可执行
    cliAdmin.stdin('/send /agent a1 r1');
    await cliAdmin.waitForOutput(/✅ 已设置 agent: a1 \/ role: r1/, 10000);
  });

  test('TC-10: 管理员（u2）发送 /unadmin web u3 移除权限', async () => {
    cliAdmin.stdin('/send /unadmin web u3');
    await cliAdmin.waitForOutput(/✅ 已移除管理权限: web \/ u3/, 10000);
  });
});
