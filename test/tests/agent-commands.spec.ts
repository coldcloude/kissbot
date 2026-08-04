import { test, expect } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, startAgent, stopAgent, waitForPort, startMemoryEgo, stopMemoryEgo } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { readFileSync, writeFileSync } from 'fs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');
const CONFIG_PATH = join(WORKSPACE, 'config.json');
const EGO_BASE = 'http://127.0.0.1:3001';
const EGO_API_KEY = 'user-key-456'; // security.api_key

let ego: ChildProcess;
let backend: ChildProcess;
let agent: ChildProcess;
let cliAdmin: SpawnedCli;   // u2：初始管理员
let cliUser: SpawnedCli;    // u3：admin/unadmin 测试对象

// 等待 cli 输出（返回 Promise，用于"不应出现"的断言配合超时）
function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

test.describe.serial('agent 管理命令测试（多会话路由，cli 经 channel-web 发送）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    // 让 agent 可解析 agent_name -> agent_id（/agent 切换前先解析；模板默认 memory_ego_url 为空）
    const cfg = JSON.parse(readFileSync(CONFIG_PATH, 'utf8'));
    cfg.api.memory_ego_url = EGO_BASE;
    writeFileSync(CONFIG_PATH, JSON.stringify(cfg, null, 2));
    ego = startMemoryEgo(WORKSPACE);
    await waitForPort(3001, '127.0.0.1', 30000);
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
    stopMemoryEgo(ego);
  });

  test('TC-02: 管理员（u2）发送 /agent a1 r1 设置 channel 的 agent 与 role', async ({ request }) => {
    // 预建 ego agent a1：/agent 切换前先解析（search-name 全匹配），不可解析则保持原 agent 并报错
    const createResp = await (await request.post(`${EGO_BASE}/agent/create`, {
      headers: { 'X-Api-Key': EGO_API_KEY, 'Content-Type': 'application/json' },
      data: { individual_name: 'a1', description: '测试 agent' },
    })).json();
    expect(createResp.success).toBe(true);
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

  test('TC-07: /send-channel 已删除（is_send_channel 删除，out_channel 改由 /bind-outgoing 设置）', async () => {
    // /send-channel 命令已删除：返回未知命令错误，不再回写配置
    cliAdmin.stdin('/send /send-channel on');
    await cliAdmin.waitForOutput(/⚠️ Invalid command: 未知命令: send-channel/, 10000);
  });

  test('TC-08: /unbind 缺 user_id 报格式错误；带 user_id 正常移除（并清空引用的 outgoing）', async () => {
    // 缺 user_id：格式错误（Task 3 起 /unbind 必须带 <user_id>，原"暂不支持"行为已删除）
    cliAdmin.stdin('/send /unbind messenger web');
    await cliAdmin.waitForOutput(/⚠️ Invalid command: 格式: \/unbind messenger <messenger_id> <user_id>/, 10000);
    // 带 user_id：正常移除（web/u1 是模板绑定用户；引用该身份的 outgoing 同步清空）
    cliAdmin.stdin('/send /unbind messenger web u1');
    await cliAdmin.waitForOutput(/✅ 已移除 channel 用户: web \/ u1/, 10000);
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

  // TC-11: /model 新语法——非法第 4 段报格式错误（纯本地解析，不触发 API 调用）
  test('TC-11: /model 非法默认参数报格式错误', async () => {
    cliAdmin.stdin('/send /model deepseek deepseek-v4-flash maybe');
    await cliAdmin.waitForOutput(/格式: \/model <provider> <model> \[true\|false\]/, 10000);
  });
});
