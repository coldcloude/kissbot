import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, startAgent, stopAgent, startMemoryEgo, stopMemoryEgo, startMemoryStore, stopMemoryStore, waitForPort, injectAgentApiKeys } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { join, dirname } from 'path';
import { readFileSync, writeFileSync } from 'fs';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');
const CONFIG_PATH = join(WORKSPACE, 'config.json');
const EGO_BASE = 'http://127.0.0.1:3001';
const STORE_BASE = 'http://127.0.0.1:8082';
const API_KEY = 'user-key-456'; // security.api_key（ego/store 共用）

let ego: ChildProcess;
let store: ChildProcess;
let backend: ChildProcess;
let agent: ChildProcess;
let cli: SpawnedCli;   // u2（管理员）经 channel-web 与 nexus 通信；agent 以 bind_user u1 身份回复
let agentId: string;   // ego 里预建 agent a1 的 UUID（由 /agent/search-name 解析）

function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

function todayDate(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

// 发送文本消息并等待 agent（u1 身份）的真实 LLM 回复
async function sendAndWaitReply(text: string): Promise<void> {
  const baseline = cli.getOutput();
  cli.stdin(`/send ${text}`);
  const replyMarker = /<< \[u1:g1\] \{"msg_type":"Text","data":"([^"]*)/;
  const start = Date.now();
  let replyData = '';
  while (Date.now() - start < 60000) {
    const tail = cli.getOutput().slice(baseline.length);
    const m = tail.match(replyMarker);
    if (m) { replyData = m[1]; break; }
    await sleep(500);
  }
  expect(replyData.length).toBeGreaterThan(0);
}

// 等待基线之后出现匹配输出（避免 waitForOutput 命中缓冲中的旧回复）
async function waitNewOutput(baseline: string, regex: RegExp, timeout = 10000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (regex.test(cli.getOutput().slice(baseline.length))) return;
    await sleep(200);
  }
  throw new Error(`CLI 新输出超时（${timeout}ms），期望 /${regex.source}/，基线后输出: ${cli.getOutput().slice(baseline.length).slice(-200)}`);
}

// 查询 memory-store channel 记录并断言该场景的用户消息（is_self=0）与 agent 回复（is_self=1）
async function assertChannelRecords(request: APIRequestContext, roleName: string): Promise<void> {
  // 等待记忆落盘（memory-store appender 100ms 批量 + 余量）
  await sleep(1500);

  async function query(userId: string) {
    return (await request.post(`${STORE_BASE}/store/query/channel`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: {
        agent_id: agentId, role_name: roleName, messenger_id: 'web',
        user_id: userId, group_id: 'g1',
        start_time: `${todayDate()} 00:00:00`, end_time: `${todayDate()} 23:59:59`,
      },
    })).json();
  }

  // 用户消息：u2 身份，is_self=0
  const qUser = await query('u2');
  expect(qUser.success).toBe(true);
  expect(qUser.data.length).toBeGreaterThanOrEqual(1);
  const keyUser = qUser.data[0][0];
  // ego 读取验证：agent_id 为 ego 解析出的 UUID（非保留值 "0"）
  expect(keyUser.agent_id).toBe(agentId);
  expect(keyUser.role_name).toBe(roleName);
  expect(keyUser.group_id).toBe('g1');
  const recUser = qUser.data[0][1];
  expect(recUser.length).toBeGreaterThanOrEqual(1);
  const lastUser = recUser[recUser.length - 1][1];
  expect(lastUser.user_id).toBe('u2');
  expect(lastUser.is_self).toBe(0);

  // agent 回复：u1 身份，is_self=1
  const qSelf = await query('u1');
  expect(qSelf.success).toBe(true);
  expect(qSelf.data.length).toBeGreaterThanOrEqual(1);
  const recSelf = qSelf.data[0][1];
  expect(recSelf.length).toBeGreaterThanOrEqual(1);
  const lastSelf = recSelf[recSelf.length - 1][1];
  expect(lastSelf.user_id).toBe('u1');
  expect(lastSelf.is_self).toBe(1);
}

test.describe.serial('nexus-ego-chat-store：ego 读取 + channel 记忆写入（4 种正常场景）', () => {

  test.beforeAll(async ({ request }) => {
    resetWorkspace();
    // 配置 memory-ego / memory-store 地址（agent 启动前生效）
    const cfg = JSON.parse(readFileSync(CONFIG_PATH, 'utf8'));
    cfg.api.memory_ego_url = EGO_BASE;
    cfg.api.memory_store_url = STORE_BASE;
    writeFileSync(CONFIG_PATH, JSON.stringify(cfg, null, 2));
    await injectAgentApiKeys();

    ego = startMemoryEgo(WORKSPACE);
    await waitForPort(3001, '127.0.0.1', 30000);
    store = startMemoryStore(WORKSPACE);
    await waitForPort(8082, '127.0.0.1', 30000);

    // 预建 ego agent a1 及其角色 r1（正常场景的 role 需在 ego 中定义）
    const createResp = await (await request.post(`${EGO_BASE}/agent/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { individual_name: 'a1', description: 'ego 测试 agent' },
    })).json();
    expect(createResp.success).toBe(true);
    agentId = createResp.data;
    const roleResp = await (await request.post(`${EGO_BASE}/role/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: agentId, role_name: 'r1', description: '测试角色' },
    })).json();
    expect(roleResp.success).toBe(true);
    // 解析确认：search-name 可命中
    const searchResp = await (await request.post(`${EGO_BASE}/agent/search-name`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { keyword: 'a1' },
    })).json();
    expect(searchResp.success).toBe(true);
    expect(searchResp.data).toBe(agentId);

    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
    await sleep(2000);   // 等待 agent 完成 channel 连接与绑定
    cli = spawnCli(['web', 'u2', 'g1', './downloads'], WORKSPACE);
    await cli.waitForOutput(/bound\./);
  });

  test.afterAll(() => {
    if (cli) cli.proc.kill();
    stopAgent(agent);
    stopBackend(backend);
    stopMemoryStore(store);
    stopMemoryEgo(ego);
  });

  // 场景 1：有 role（角色模式）——memory_role = "r1"
  test('场景1-有role: 绑定 a1/r1 并写入角色模式记忆', async ({ request }) => {
    const base = cli.getOutput();
    cli.stdin('/send /agent a1 r1');
    await waitNewOutput(base, /✅ 已设置 agent: a1 \/ role: r1/);
    await sendAndWaitReply('你好，请用一句话自我介绍');
    await assertChannelRecords(request, 'r1');
  });

  // 场景 2：无 role（角色模式）——memory_role = ""
  test('场景2-无role: 绑定 a1（空 role）并写入角色模式记忆', async ({ request }) => {
    const base = cli.getOutput();
    cli.stdin('/send /agent a1');
    await waitNewOutput(base, /✅ 已设置 agent: a1 \/ role: "}/);
    await sendAndWaitReply('你好，请用一句话自我介绍');
    await assertChannelRecords(request, '');
  });

  // 场景 3：无 role + 事件模式——memory_role = "-ev1"
  test('场景3-无role有event: 绑定 a1 + 事件模式 ev1', async ({ request }) => {
    const base = cli.getOutput();
    cli.stdin('/send /agent a1');
    await waitNewOutput(base, /✅ 已设置 agent: a1 \/ role: "}/);
    const base2 = cli.getOutput();
    cli.stdin('/send /mode event ev1');
    await waitNewOutput(base2, /✅ 新事件 ID: ev1/);
    await sendAndWaitReply('你好，请用一句话自我介绍');
    await assertChannelRecords(request, '-ev1');
  });

  // 场景 4：有 role + 事件模式——memory_role = "r1-ev2"
  test('场景4-有role有event: 绑定 a1/r1 + 事件模式 ev2', async ({ request }) => {
    const base = cli.getOutput();
    cli.stdin('/send /agent a1 r1');
    await waitNewOutput(base, /✅ 已设置 agent: a1 \/ role: r1/);
    const base2 = cli.getOutput();
    cli.stdin('/send /mode event ev2');
    await waitNewOutput(base2, /✅ 新事件 ID: ev2/);
    await sendAndWaitReply('你好，请用一句话自我介绍');
    await assertChannelRecords(request, 'r1-ev2');
  });
});
