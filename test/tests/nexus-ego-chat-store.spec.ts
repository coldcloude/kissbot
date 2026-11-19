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
const NEXUS_PATH = join(WORKSPACE, 'agent-data', 'nexus.json');
const CHANNEL_REPO_PATH = join(WORKSPACE, 'channel-data', 'channel-web-repo.json');
const EGO_BASE = 'http://127.0.0.1:3001';
const STORE_BASE = 'http://127.0.0.1:8082';
const API_KEY = 'user-key-456'; // security.api_key（ego/store 共用）

let ego: ChildProcess;
let store: ChildProcess;
let backend: ChildProcess;
let agent: ChildProcess;
let cli: SpawnedCli;   // u2（管理员，位于 g1/g2/g3，经 channel-web 发送）
let agentId: string;   // ego 里预建 agent a1 的 agent_id
let b1Id: string;      // ego 里预建 agent b1 的 agent_id

// 每个 pipeline 使用互不重叠的群组，避免同一群消息同时命中多条 pipeline：
//   a1/""   → u1 @ g1
//   a1/r1   → u3 @ g2
//   b1/out1 → u4 @ g3
const GROUP_EMPTY_ROLE = 'g1';
const GROUP_ROLE = 'g2';
const GROUP_OUT = 'g3';

function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

/** 记忆后缀（测试内部的目录/文件寻址用）→ 请求里的 RoleEvent 字段（按第一个 '-' 拆角色与事件） */
function roleEventOf(suffix: string): { role_name: string; event_id?: string } {
  const i = suffix.indexOf('-');
  return i < 0 ? { role_name: suffix } : { role_name: suffix.slice(0, i), event_id: suffix.slice(i + 1) };
}

// UTC 日期（backend channel 记录 time 来自 Utc::now()，归档按 UTC 日期分区）
function utcDate(): string {
  return new Date().toISOString().slice(0, 10);
}

/** 切换 CLI 当前群组（CLI 按 args 里的 user 发送，群组可运行期切换） */
async function switchGroup(group: string): Promise<void> {
  if (cli.getOutput().includes(`>> current group: ${group}`)) return;
  const baseline = cli.getOutput();
  cli.stdin(`/group ${group}`);
  await waitNewOutput(baseline, new RegExp(`>> current group: ${group}`));
}

/** 在当前群组发送文本，等待指定身份的 LLM 回复回显 */
async function sendAndWaitReplyOn(sendGroup: string, replyUser: string, replyGroup: string, text: string): Promise<void> {
  await switchGroup(sendGroup);
  const baseline = cli.getOutput();
  cli.stdin(`/send ${text}`);
  const replyMarker = new RegExp(`<< \\[${replyUser}:${replyGroup}\\] .*?"msg_type":"Text","data":"([^"]*)`);
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

/** 在指定群组发送文本，等待该群组自身身份（如 u1@g1）的回复 */
async function sendAndWaitReply(text: string, group = GROUP_EMPTY_ROLE, selfUser = 'u1'): Promise<void> {
  await sendAndWaitReplyOn(group, selfUser, group, text);
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

/** 递归取 Content 中的文本段（Text 或 Multi 内的 Text） */
function textOfContent(content: any): string | null {
  if (!content) return null;
  if (content.msg_type === 'Text') return content.data;
  if (content.msg_type === 'Multi') {
    const texts = (content.data as any[]).map(textOfContent).filter((t): t is string => t !== null);
    return texts.length > 0 ? texts.join('\n') : null;
  }
  return null;
}

/** 递归取 Content 中的内嵌思考/工具记录（Multi 内的 Think/ToolCall/ToolResult） */
function detailRecordsOfContent(content: any): any[] {
  if (!content) return [];
  if (content.msg_type === 'Think' || content.msg_type === 'ToolCall' || content.msg_type === 'ToolResult') {
    return [{ msgType: content.msg_type, data: content.data }];
  }
  if (content.msg_type === 'Multi') {
    return (content.data as any[]).flatMap(detailRecordsOfContent);
  }
  return [];
}

/** 查询 memory-store channel 记录（单文件全量），返回记录数组；查询失败抛错，由调用方决定重试 */
async function queryChannelRecords(request: APIRequestContext, targetAgentId: string, roleName: string): Promise<any[]> {
  const resp = await (await request.post(`${STORE_BASE}/store/query/channel`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: {
      agent_id: targetAgentId, role_event: roleEventOf(roleName),
      start_time: `${utcDate()} 00:00:00`, end_time: `${utcDate()} 23:59:59`,
    },
  })).json();
  if (!resp.success) {
    throw new Error(`查询 channel 记录失败（agent=${targetAgentId} role=${roleName}）: ${JSON.stringify(resp)}`);
  }
  return (resp.data ?? []).flatMap((entry: [any, any]) =>
    (entry[1] as [number, any][]).map((e) => e[1]));
}

// 查询 memory-store channel 记录并断言该场景的双向消息（is_self=0 用户消息 / is_self=1 agent 回复）
async function assertChannelRecords(request: APIRequestContext, targetAgentId: string, roleName: string, selfUser: string): Promise<void> {
  // 等待记忆落盘（memory-store appender 100ms 批量 + 余量）
  await sleep(1500);

  const resp = await (await request.post(`${STORE_BASE}/store/query/channel`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: {
      agent_id: targetAgentId, role_event: roleEventOf(roleName),
      start_time: `${utcDate()} 00:00:00`, end_time: `${utcDate()} 23:59:59`,
    },
  })).json();
  expect(resp.success).toBe(true);
  expect(resp.data.length).toBeGreaterThanOrEqual(1);
  const key = resp.data[0][0];
  expect(key.agent_id).toBe(targetAgentId);
  expect(key.role_event).toEqual(roleEventOf(roleName));

  const recs: any[] = resp.data[0][1].map((entry: [number, any]) => entry[1]);
  // 用户消息：u2 发送，is_self=0
  const userMsgs = recs.filter((r) => r.is_self === 0);
  expect(userMsgs.length).toBeGreaterThanOrEqual(1);
  expect(userMsgs[userMsgs.length - 1].user_id).toBe('u2');
  // agent 回复：以该 pipeline 的回复身份发送，is_self=1（内容可能是 Multi，其中内嵌思考/工具记录）
  const selfMsgs = recs.filter((r) => r.is_self === 1);
  expect(selfMsgs.length).toBeGreaterThanOrEqual(1);
  const selfTexts = selfMsgs
    .map((record) => ({ record, text: textOfContent(record.content) }))
    .filter((entry): entry is { record: any; text: string } => entry.text !== null);
  expect(selfTexts.length).toBeGreaterThanOrEqual(1);
  expect(selfTexts[selfTexts.length - 1].record.user_id).toBe(selfUser);
  // 消息标识：上行来自 incoming_message、下行来自 outgoing_message_response，均写入记录（去重用）
  const userMsgId = userMsgs[userMsgs.length - 1].msg_id;
  const selfMsgId = selfTexts[selfTexts.length - 1].record.msg_id;
  expect(userMsgId, '上行记录的 msg_id 应非空').toBeTruthy();
  expect(selfMsgId, '下行记录的 msg_id 应非空').toBeTruthy();
  expect(userMsgId).not.toBe(selfMsgId);
  // 思考/工具记录随回复记录写入：直接内嵌完整内容，不再有 key 与详情时间
  for (const detail of selfMsgs.flatMap((record) => detailRecordsOfContent(record.content))) {
    expect(detail.data, `内嵌记录 ${detail.msgType} 应带内容`).toBeTruthy();
    expect(detail.data.key, `内嵌记录 ${detail.msgType} 不应带 key`).toBeUndefined();
    expect(detail.data.time, `内嵌记录 ${detail.msgType} 不应带详情时间`).toBeUndefined();
  }
}

/** 等待指定内容的用户消息 channel 记录落盘（is_self=0；agent 收到消息即写入，与是否回复无关） */
async function waitUserMessageRecord(request: APIRequestContext, targetAgentId: string, roleName: string, content: string): Promise<void> {
  const start = Date.now();
  let lastError = '';
  while (Date.now() - start < 10000) {
    const recs = await queryChannelRecords(request, targetAgentId, roleName).catch((e: Error) => {
      lastError = e.message;
      return [] as any[];
    });
    if (recs.some((r) => r.is_self === 0 && r.content?.data === content)) return;
    await sleep(300);
  }
  throw new Error(`超时未见用户消息 channel 记录: ${content}${lastError ? `（最后一次查询失败: ${lastError}）` : ''}`);
}

/** 断言基线后不再出现 agent 回复回显（out_channel 产出以 u1@g1 发送） */
async function assertNoAgentReply(baseline: string, timeout = 8000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const tail = cli.getOutput().slice(baseline.length);
    if (/<< \[u1:g1\]/.test(tail)) {
      throw new Error(`不应出现 agent 回复回显（out_channel 应已关闭/清空），实际新输出: ${tail.slice(-200)}`);
    }
    await sleep(200);
  }
}

/** 把 ego 返回的 agent UUID 写入 nexus.json：pipeline 集合 + input channels + 默认 out_channel */
function configureNexus(agentUuid: string, b1Uuid: string) {
  const nexus = JSON.parse(readFileSync(NEXUS_PATH, 'utf8'));
  const templateAgent = nexus.agent_roles['0'];
  const llm = templateAgent.config_map.llm;

  // 通用（agent 级）配置：llm、合批、工具分组；不写 pipeline（pipeline 改由顶层 pipelines 声明）
  function baseConfig() {
    return {
      llm,
      channel_batch: { channel_batch_interval_secs: 3 },
      group_set: { group_set: ['filesystem'] },
    };
  }

  nexus.agent_roles = {
    [agentUuid]: {
      config_map: {
        ...baseConfig(),
        out_channel: { out_channel: { channel_id: 'web-main', user: { messenger_id: 'web', user_id: 'u1' }, group_id: GROUP_EMPTY_ROLE } },
        input_channels: {
          input_channels: [{ channel_id: 'web-main', messenger_id: 'web', user_id: 'u1' }],
        },
      },
      roles: {
        r1: {
          out_channel: { out_channel: { channel_id: 'web-main', user: { messenger_id: 'web', user_id: 'u3' }, group_id: GROUP_ROLE } },
          input_channels: {
            input_channels: [{ channel_id: 'web-main', messenger_id: 'web', user_id: 'u3' }],
          },
        },
      },
    },
    [b1Uuid]: {
      config_map: baseConfig(),
      roles: {
        out1: {
          input_channels: {
            input_channels: [{ channel_id: 'web-main', messenger_id: 'web', user_id: 'u4' }],
          },
        },
      },
    },
  };
  nexus.pipelines = {
    [agentUuid]: {
      '': { pipeline_name: 'default_memory' },
      r1: { pipeline_name: 'default_memory' },
    },
    [b1Uuid]: {
      out1: { pipeline_name: 'default_memory' },
    },
  };
  writeFileSync(NEXUS_PATH, JSON.stringify(nexus, null, 2) + '\n');
}

/** 拆分群组：g1 只有 u1，g2 只有 u3，g3 只有 u4；u2 在三个群组中发送 */
function configureChannelUsers() {
  const repo = JSON.parse(readFileSync(CHANNEL_REPO_PATH, 'utf8'));
  repo.users.u4 = { user_id: 'u4', user_name: '助手小D' };
  repo.groups.g1 = { group_id: 'g1', group_name: '空角色组', members: ['admin', 'u1', 'u2'] };
  repo.groups.g2 = { group_id: 'g2', group_name: '角色组', members: ['u2', 'u3'] };
  repo.groups.g3 = { group_id: 'g3', group_name: '输出组', members: ['u2', 'u4'] };
  repo.next_user_seq = 5;
  repo.next_group_seq = 4;
  writeFileSync(CHANNEL_REPO_PATH, JSON.stringify(repo, null, 2) + '\n');
}

test.describe.serial('nexus-ego-chat-store：ego 读取 + channel 记忆写入 + out_channel 路由（pipeline 配置驱动）', () => {
  test.describe.configure({ timeout: 60000 });

  test.beforeAll(async ({ request }) => {
    resetWorkspace();
    // 配置 memory-ego / memory-store 地址（agent 启动前生效）
    const cfg = JSON.parse(readFileSync(CONFIG_PATH, 'utf8'));
    cfg.api.memory_ego_url = EGO_BASE;
    cfg.api.memory_store_url = STORE_BASE;
    writeFileSync(CONFIG_PATH, JSON.stringify(cfg, null, 2));

    ego = startMemoryEgo(WORKSPACE);
    await waitForPort(3001, '127.0.0.1', 30000);
    store = startMemoryStore(WORKSPACE);
    await waitForPort(8082, '127.0.0.1', 30000);

    // 预建 ego agent a1 / b1 及角色
    const createResp = await (await request.post(`${EGO_BASE}/agent/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: 'a1', description: 'ego 测试 agent' },
    })).json();
    expect(createResp.success).toBe(true);
    agentId = createResp.data;
    for (const roleName of ['r1']) {
      const roleResp = await (await request.post(`${EGO_BASE}/role/create`, {
        headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
        data: { agent_id: agentId, role_name: roleName, description: '测试角色' },
      })).json();
      expect(roleResp.success).toBe(true);
    }
    const createB1 = await (await request.post(`${EGO_BASE}/agent/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: 'b1', description: 'ego 测试 agent（无默认 out_channel）' },
    })).json();
    expect(createB1.success).toBe(true);
    b1Id = createB1.data;
    const outRoleResp = await (await request.post(`${EGO_BASE}/role/create`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: b1Id, role_name: 'out1', description: '输出角色' },
    })).json();
    expect(outRoleResp.success).toBe(true);

    // agent 启动前写入 pipeline / input channel 配置与隔离的群组
    configureChannelUsers();
    configureNexus(agentId, b1Id);
    await injectAgentApiKeys();

    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
    await sleep(2000);   // 等待 agent 装配 pipeline、连接 channel 并绑定 input identity
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

  // 场景 1：a1/r1 角色模式
  test('场景1-有role: a1/r1 角色模式写入记忆', async ({ request }) => {
    await sendAndWaitReply('你好，请用一句话自我介绍', GROUP_ROLE, 'u3');
    await assertChannelRecords(request, agentId, 'r1', 'u3');
  });

  // 场景 2：a1 空 role + 事件模式 ev1
  test('场景2-无role有event: a1 空 role + 事件模式 ev1', async ({ request }) => {
    await switchGroup(GROUP_EMPTY_ROLE);
    let base = cli.getOutput();
    cli.stdin('/send /mode event ev1');
    await waitNewOutput(base, /✅ 新事件 ID: ev1/);
    await sendAndWaitReply('你好，请用一句话自我介绍', GROUP_EMPTY_ROLE, 'u1');
    await assertChannelRecords(request, agentId, '-ev1', 'u1');
  });

  // 场景 3：b1/out1 的 out_channel 路由（unbind-outgoing 只存不回复，恢复后回复）
  test('场景3-out_channel: unbind-outgoing 只存不回复，恢复后回复', async ({ request }) => {
    await switchGroup(GROUP_OUT);
    // 1. /bind-outgoing 建立 (b1, out1) 回复通道（校验 u1 已被 a1 的 input channel 绑定通过）
    let base = cli.getOutput();
    cli.stdin('/send /bind-outgoing web u1 g1');
    await waitNewOutput(base, /✅ 已设发送通道: web \/ u1 -> g1/);

    // 2. 普通消息经 out_channel 回复到 u1@g1
    await sendAndWaitReplyOn(GROUP_OUT, 'u1', GROUP_EMPTY_ROLE, '你好，请确认你收到了这条消息');
    await assertChannelRecords(request, b1Id, 'out1', 'u1');

    // 3. 管理员关 out_channel（/unbind-outgoing）：清 (b1, out1) → 回落无默认（只存不回复）
    base = cli.getOutput();
    cli.stdin('/send /unbind-outgoing');
    await waitNewOutput(base, /✅ 已取消发送通道（只存不回复）/);

    // 4. 再发普通消息：无回复，但 channel 记录仍写入（is_self=0）
    const baseline = cli.getOutput();
    const offlineMsg = '路由关闭测试-只存不回复';
    cli.stdin(`/send ${offlineMsg}`);
    await waitUserMessageRecord(request, b1Id, 'out1', offlineMsg);
    await assertNoAgentReply(baseline);

    // 5. 恢复 out_channel 后再发消息：agent 又回复
    base = cli.getOutput();
    cli.stdin('/send /bind-outgoing web u1 g1');
    await waitNewOutput(base, /✅ 已设发送通道: web \/ u1 -> g1/);
    await sendAndWaitReplyOn(GROUP_OUT, 'u1', GROUP_EMPTY_ROLE, '恢复发送通道后，请再次确认你收到了消息');
  });
});
