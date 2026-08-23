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
let agentId: string;   // ego 里预建 agent a1 的 agent_id（手工指定，创建时传入）

function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

// 本地日期（agent think 记录 time 来自 Local::now()，归档按本地日期分区）
function todayDate(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

// UTC 日期（backend channel 记录 time 来自 Utc::now()，归档按 UTC 日期分区；
// 深夜本地日期 > UTC 日期时两者不同日，查询须用各自时间源的日期）
function utcDate(): string {
  return new Date().toISOString().slice(0, 10);
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

// 查询 memory-store channel 记录并断言该场景的双向消息（is_self=0 用户消息 / is_self=1 agent 回复）
// 所有 channel 记录同文件（channel-records-{date}.jsonl）；身份在记录字段中（user_id=发送者、self_user_id=接收方/绑定用户）
async function assertChannelRecords(request: APIRequestContext, roleName: string): Promise<void> {
  // 等待记忆落盘（memory-store appender 100ms 批量 + 余量）
  await sleep(1500);

  const resp = await (await request.post(`${STORE_BASE}/store/query/channel`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: {
      agent_id: agentId, role_name: roleName,
      start_time: `${utcDate()} 00:00:00`, end_time: `${utcDate()} 23:59:59`,
      // 查询日期用 UTC（backend channel 记录 time 来自 Utc::now()）
    },
  })).json();
  expect(resp.success).toBe(true);
  expect(resp.data.length).toBeGreaterThanOrEqual(1);
  const key = resp.data[0][0];
  // key 为公共 RecordKey；agent_id 为 ego 解析出的 UUID（非保留值 "0"）
  expect(key.agent_id).toBe(agentId);
  expect(key.role_name).toBe(roleName);

  const recs: any[] = resp.data[0][1].map((entry: [number, any]) => entry[1]);
  // 用户消息：u2 发送，is_self=0
  const userMsgs = recs.filter((r) => r.is_self === 0);
  expect(userMsgs.length).toBeGreaterThanOrEqual(1);
  expect(userMsgs[userMsgs.length - 1].user_id).toBe('u2');
  // agent 回复：以绑定用户 u1 身份发送，is_self=1
  const selfMsgs = recs.filter((r) => r.is_self === 1);
  expect(selfMsgs.length).toBeGreaterThanOrEqual(1);
  expect(selfMsgs[selfMsgs.length - 1].user_id).toBe('u1');
}

// ==================== out_channel 路由测试辅助 ====================

// 读取 nexus.json 的 (agent, role) 有效 out_channel 配置（role 覆盖 or agent 默认回落；命令回写先于回复，断言时已持久化）
function readOutChannel(agentId: string, roleName: string): any {
  const nexus = JSON.parse(readFileSync(join(WORKSPACE, 'agent-data', 'nexus.json'), 'utf8'));
  const agent = nexus.agent_contexts?.[agentId];
  const role = agent?.roles?.[roleName];
  return role?.out_channel ?? agent?.default_context_config?.out_channel ?? null;
}

// 查询 memory-store channel 记录（单文件全量），返回记录数组（含 is_self/user_id/content 字段）
async function queryChannelRecords(request: APIRequestContext, roleName: string): Promise<any[]> {
  const resp = await (await request.post(`${STORE_BASE}/store/query/channel`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: {
      agent_id: agentId, role_name: roleName,
      start_time: `${utcDate()} 00:00:00`, end_time: `${utcDate()} 23:59:59`,
    },
  })).json();
  expect(resp.success).toBe(true);
  return (resp.data ?? []).flatMap((entry: [any, any]) =>
    (entry[1] as [number, any][]).map((e) => e[1]));
}

// 等待指定内容的用户消息 channel 记录落盘（is_self=0；agent 收到消息即写入，与是否回复无关）
async function waitUserMessageRecord(request: APIRequestContext, roleName: string, content: string): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < 10000) {
    const recs = await queryChannelRecords(request, roleName);
    if (recs.some((r) => r.is_self === 0 && r.content?.data === content)) return;
    await sleep(300);
  }
  throw new Error(`超时未见用户消息 channel 记录: ${content}`);
}

// 断言基线后不再出现 agent 回复回显（<< [u1:g1] / [u3:g1] = out_channel 产出）；
// 窗口覆盖模型调用耗时（若 out_channel 未关，回复约 2-5s 内必到）
async function assertNoAgentReply(baseline: string, timeout = 8000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const tail = cli.getOutput().slice(baseline.length);
    if (/<< \[u1:g1\]|<< \[u3:g1\]/.test(tail)) {
      throw new Error(`不应出现 agent 回复回显（out_channel 应已关闭/清空），实际新输出: ${tail.slice(-200)}`);
    }
    await sleep(200);
  }
}

// 查询 memory-store think 记录并断言思考记忆已生成（reasoning_content 或 thinking 任一非空 + key 非空）
// 本测试在模板 default_thinking=enabled 下运行，deepseek-v4-flash 思考模式开启时必有 reasoning_content
async function assertThinkRecords(request: APIRequestContext, roleName: string): Promise<void> {
  // 等待记忆落盘（memory-store appender 100ms 批量 + 余量）
  await sleep(1500);

  const resp = await (await request.post(`${STORE_BASE}/store/query/think`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: {
      agent_id: agentId, role_name: roleName,
      start_time: `${todayDate()} 00:00:00`, end_time: `${todayDate()} 23:59:59`,
      // 查询日期用本地（agent think 记录 time 来自 Local::now()）
    },
  })).json();
  expect(resp.success).toBe(true);
  expect(resp.data.length).toBeGreaterThanOrEqual(1, 'think 记录应已写入（agent 侧 MemoryStoreClient 推送 /store/think）');
  const key = resp.data[0][0];
  // ego 读取验证：agent_id 为 ego 解析出的 UUID（非保留值 "0"）
  expect(key.agent_id).toBe(agentId);
  expect(key.role_name).toBe(roleName);
  const thinkRecs: any[] = resp.data[0][1].map((entry: [number, any]) => entry[1]);
  expect(thinkRecs.length).toBeGreaterThanOrEqual(1, 'think 记录应已写入（agent 侧 MemoryStoreClient 推送 /store/think）');
  const t = thinkRecs[thinkRecs.length - 1];
  const hasReasoning = t.reasoning_content && t.reasoning_content.length > 0;
  const hasThinking = t.thinking && t.thinking.length > 0;
  expect(hasReasoning || hasThinking).toBe(true, 'reasoning_content 或 thinking 应有内容');
  expect(t.key, 'key 应非空（UUID 关联 ChannelRecord(Think)）').toBeTruthy();
}

test.describe.serial('nexus-ego-chat-store：ego 读取 + channel 记忆写入 + out_channel 路由（4 种正常场景 + 2 种路由场景）', () => {

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
      data: { agent_id: 'a1', description: 'ego 测试 agent' },
    })).json();
    expect(createResp.success).toBe(true);
    agentId = createResp.data;
    const roleResp = await (await request.post(`${EGO_BASE}/role/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: agentId, role_name: 'r1', description: '测试角色' },
    })).json();
    expect(roleResp.success).toBe(true);
    // 预建 out_channel 路由场景 role（out1/out2）：66f0b32 起 /agent 切换前经 ego 校验 role 存在
    const out1Resp = await (await request.post(`${EGO_BASE}/role/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: agentId, role_name: 'out1', description: '输出角色1' },
    })).json();
    expect(out1Resp.success).toBe(true);
    const out2Resp = await (await request.post(`${EGO_BASE}/role/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: agentId, role_name: 'out2', description: '输出角色2' },
    })).json();
    expect(out2Resp.success).toBe(true);
    // 预建无默认 out_channel 的 agent b1（场景 5/6 验证"只存不回复"需回落 None；b1 不配模板默认）
    const createB1 = await (await request.post(`${EGO_BASE}/agent/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: 'b1', description: 'ego 测试 agent（无默认 out_channel）' },
    })).json();
    expect(createB1.success).toBe(true);
    for (const roleName of ['out1', 'out2']) {
      const r = await (await request.post(`${EGO_BASE}/role/create`, {
        headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
        data: { agent_id: createB1.data, role_name: roleName, description: '输出角色' },
      })).json();
      expect(r.success).toBe(true);
    }
    // 解析确认：agent 存在
    const getResp = await (await request.post(`${EGO_BASE}/agent/get`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: agentId },
    })).json();
    expect(getResp.success).toBe(true);
    expect(getResp.data.agent_id).toBe(agentId);

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
    await assertThinkRecords(request, 'r1');
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

  // 场景 5：out_channel 路由——unbind-outgoing 只存不回复，恢复后 agent 又回复
  // 用无默认 out_channel 的 b1：/bind-outgoing 建立 (b1, out1) 回复通道；unbind 清除后回落 b1 默认（None）= 只存不回复
  test('场景5-out_channel: unbind-outgoing 只存不回复，恢复后回复', async ({ request }) => {
    // 切回角色模式并设 agent b1 / role out1（b1 无模板默认 out_channel）
    let base = cli.getOutput();
    cli.stdin('/send /mode role');
    await waitNewOutput(base, /✅ 已切换为角色模式/);
    base = cli.getOutput();
    cli.stdin('/send /agent b1 out1');
    await waitNewOutput(base, /✅ 已设置 agent: b1 \/ role: out1/);

    // 1. /bind-outgoing 建立 (b1, out1) 回复通道（校验 u1 已绑定通过）
    base = cli.getOutput();
    cli.stdin('/send /bind-outgoing web u1 g1');
    await waitNewOutput(base, /✅ 已设发送通道: web \/ u1 -> g1/);
    expect(readOutChannel('b1', 'out1')).toEqual({ channel_id: 'web-main', user: { messenger_id: 'web', user_id: 'u1' }, group_id: 'g1' });

    // 2. 普通消息经 out_channel 回复
    await sendAndWaitReply('你好，请确认你收到了这条消息');

    // 3. 管理员关 out_channel（/unbind-outgoing）：清 (b1, out1) → 回落 b1 默认（None）
    base = cli.getOutput();
    cli.stdin('/send /unbind-outgoing');
    await waitNewOutput(base, /✅ 已取消发送通道（只存不回复）/);
    // 落盘验证：b1 无默认，(b1, out1) 有效 out_channel 已清空
    expect(readOutChannel('b1', 'out1')).toBeNull();

    // 4. 再发普通消息：无回复（不进 Agentic Loop），但 channel 记录仍写入（is_self=0）
    const baseline = cli.getOutput();
    const offlineMsg = '路由关闭测试-只存不回复';
    cli.stdin(`/send ${offlineMsg}`);
    await waitUserMessageRecord(request, 'out1', offlineMsg);
    await assertNoAgentReply(baseline);

    // 5. 恢复 out_channel（/bind-outgoing web u1 g1）
    base = cli.getOutput();
    cli.stdin('/send /bind-outgoing web u1 g1');
    await waitNewOutput(base, /✅ 已设发送通道: web \/ u1 -> g1/);
    expect(readOutChannel('b1', 'out1')).toEqual({ channel_id: 'web-main', user: { messenger_id: 'web', user_id: 'u1' }, group_id: 'g1' });

    // 6. 再发普通消息：agent 又回复
    await sendAndWaitReply('恢复发送通道后，请再次确认你收到了消息');
  });

  // 场景 6：out_channel 路由——/bind 追加 + /bind-outgoing 指向新绑 user + /unbind 后 send 校验未绑定才清理
  test('场景6-out_channel: bind 追加 u3 + bind-outgoing 指向 u3 + unbind 后 send 校验未绑定才清理', async ({ request }) => {
    // 切回角色模式并设 agent b1 / role out2（b1 无模板默认 out_channel）
    let base = cli.getOutput();
    cli.stdin('/send /mode role');
    await waitNewOutput(base, /✅ 已切换为角色模式/);
    base = cli.getOutput();
    cli.stdin('/send /agent b1 out2');
    await waitNewOutput(base, /✅ 已设置 agent: b1 \/ role: out2/);

    // 1. /bind 追加绑定 u3（去重追加语义由命令层保证，此处验证追加成功并落盘）
    base = cli.getOutput();
    cli.stdin('/send /bind messenger web u3');
    await waitNewOutput(base, /✅ 已绑定 channel 用户: web \/ u3/);
    const ch = JSON.parse(readFileSync(join(WORKSPACE, 'agent-data', 'nexus.json'), 'utf8')).channels['web-main'];
    expect(ch.bind_users).toContainEqual({ messenger_id: 'web', user_id: 'u3' });

    // 2. /bind-outgoing 指向新绑的 u3（校验已绑定通过；本轮不回发 u3 消息，u3 未在 ws 绑定）
    base = cli.getOutput();
    cli.stdin('/send /bind-outgoing web u3 g1');
    await waitNewOutput(base, /✅ 已设发送通道: web \/ u3 -> g1/);
    // 落盘：(b1, out2) role 覆盖写入 out_channel
    expect(readOutChannel('b1', 'out2')).toEqual({ channel_id: 'web-main', user: { messenger_id: 'web', user_id: 'u3' }, group_id: 'g1' });

    // 3. /unbind 移除 u3：out_channel 在 (agent, role) context，与 channel 绑定解耦——unbind 不改配置
    base = cli.getOutput();
    cli.stdin('/send /unbind messenger web u3');
    await waitNewOutput(base, /✅ 已移除 channel 用户: web \/ u3/);
    expect(readOutChannel('b1', 'out2')).not.toBeNull();

    // 4. u2 发普通消息：run_agentic_loop 读 (b1, out2) out_channel（仍指向 u3）→ send 前校验 u3 已不在
    //    bind_users → 清理该 (agent, role) out 配置并跳过发送：无回复（只存不回复），channel 记录仍写入
    const baseline = cli.getOutput();
    const offlineMsg = 'unbind后消息-无回复';
    cli.stdin(`/send ${offlineMsg}`);
    await waitUserMessageRecord(request, 'out2', offlineMsg);
    await assertNoAgentReply(baseline);
    // send 校验失败已清理 (b1, out2) 的 out_channel
    expect(readOutChannel('b1', 'out2')).toBeNull();
  });
});
