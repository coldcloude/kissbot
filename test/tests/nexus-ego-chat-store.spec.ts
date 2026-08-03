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

async function storeQuery(request: APIRequestContext, user_id: string) {
  return (await request.post(`${STORE_BASE}/store/query/channel`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: {
      agent_id: agentId, role_name: 'r1', messenger_id: 'web',
      user_id, group_id: 'g1',
      start_time: `${todayDate()} 00:00:00`, end_time: `${todayDate()} 23:59:59`,
    },
  })).json();
}

test.describe.serial('nexus-ego-chat-store：ego 读取 + channel 记忆写入', () => {

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

    // 预建 ego agent a1：agent 绑定 /agent a1 时经 /agent/search-name 解析出其 UUID
    const createResp = await (await request.post(`${EGO_BASE}/agent/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { individual_name: 'a1', description: 'ego 测试 agent' },
    })).json();
    expect(createResp.success).toBe(true);
    agentId = createResp.data;
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

  // TC-1 绑定 ego agent：/agent a1 r1 触发 agent 经 ego /agent/search-name 解析 agent_id（UUID）
  test('TC-1: /agent a1 r1 绑定（ego 解析）', async () => {
    cli.stdin('/send /agent a1 r1');
    await cli.waitForOutput(/✅ 已设置 agent: a1 \/ role: r1/, 10000);
  });

  // TC-2 发送文本消息得到真实 LLM 回复（agentic loop 运行，产生 is_self=1 回复记录）
  test('TC-2: 普通文本消息得到真实 LLM 非空回复', async () => {
    // 基线：TC-1 的管理命令回复已在缓冲区；用"基线之后的新输出"轮询 agent 的 LLM 回复特征：
    // agent 以 channel bind_user（u1）身份回复，CLI 打印形如 << [u1:g1] {"msg_type":"Text","data":"..."}
    const baseline = cli.getOutput();
    cli.stdin('/send 你好，请用一句话自我介绍');
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
  });

  // TC-3 channel 记忆写入验证：
  // - 用户消息（u2）→ is_self=0 记录；agent 回复（u1）→ is_self=1 记录
  // - 记录 key.agent_id == ego 预建 a1 的 UUID（非保留值 "0"），证明 ego 解析链路成功
  test('TC-3: 用户消息与 agent 回复均写入 channel 记忆（agent_id 为 ego UUID）', async ({ request }) => {
    // 等待记忆落盘（memory-store appender 100ms 批量 + 余量）
    await sleep(1500);

    const qUser = await storeQuery(request, 'u2');
    expect(qUser.success).toBe(true);
    expect(qUser.data.length).toBeGreaterThanOrEqual(1);
    const keyUser = qUser.data[0][0];
    // ego 读取验证：agent_id 为 ego 解析出的 UUID，而非解析失败的保留值 "0"
    expect(keyUser.agent_id).toBe(agentId);
    expect(keyUser.role_name).toBe('r1');
    expect(keyUser.group_id).toBe('g1');
    const recUser = qUser.data[0][1];
    expect(recUser.length).toBeGreaterThanOrEqual(1);
    const lastUser = recUser[recUser.length - 1][1];
    expect(lastUser.user_id).toBe('u2');
    expect(lastUser.is_self).toBe(0);

    const qSelf = await storeQuery(request, 'u1');
    expect(qSelf.success).toBe(true);
    expect(qSelf.data.length).toBeGreaterThanOrEqual(1);
    const recSelf = qSelf.data[0][1];
    expect(recSelf.length).toBeGreaterThanOrEqual(1);
    const lastSelf = recSelf[recSelf.length - 1][1];
    expect(lastSelf.user_id).toBe('u1');
    expect(lastSelf.is_self).toBe(1);
  });
});
