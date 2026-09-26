import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { ChildProcess } from 'child_process';
import { fileURLToPath } from 'url';
import { join, dirname } from 'path';
import { existsSync, readFileSync } from 'fs';
import {
  MEMORY_STORE_NOTIFY_PORT,
  MEMORY_STRUCT_ABSTRACT_PORT,
  resetWorkspace,
  startMemoryStore,
  startMemoryStructAbstract,
  stopMemoryStore,
  stopMemoryStructAbstract,
  waitForPort,
} from './helpers/server';

const STORE_BASE = 'http://127.0.0.1:8082';
const ABSTRACT_BASE = 'http://127.0.0.1:8083';
const API_KEY = 'user-key-456'; // security.api_key

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

// 测试常量（role_name 仅允许 [A-Za-z0-9_]+）
const AGENT = 'agent_a';
const ROLE = 'admin';
const MESSENGER = 'web';
const SELF_USER = 'self1';
const GROUP = 'g1';
/** 测试常量对应的通道 */
const ROLE_CHANNEL = { messenger: MESSENGER, selfUser: SELF_USER, group: GROUP };

let store: ChildProcess;
let abstract: ChildProcess;

function pad(n: number): string {
  return String(n).padStart(2, '0');
}

/** 递增的时间字符串（同一秒内多条记录也能保持有序） */
function timeAt(offsetSecs: number): string {
  const d = new Date(Date.now() + offsetSecs * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function apiPost(request: APIRequestContext, base: string, path: string, body: unknown) {
  return (await request.post(`${base}${path}`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: body,
  })).json();
}

/** 推送一条通道消息到 memory-store（is_self 为 1 表示 agent 实际发送） */
async function pushChannel(request: APIRequestContext, user: string, isSelf: number, text: string, time: string, msgId = '') {
  const resp = await apiPost(request, STORE_BASE, '/store/channel', {
    requests: [{
      msg_id: msgId,
      agent_id: AGENT,
      role_name: ROLE,
      messenger_id: MESSENGER,
      user_id: user,
      self_user_id: SELF_USER,
      group_id: GROUP,
      is_self: isSelf,
      messenger_name: '',
      user_name: user,
      group_name: '',
      content: { msg_type: 'Text', data: text },
      time,
    }],
    force: 0,
  });
  expect(resp.success, `推送记录失败: ${JSON.stringify(resp)}`).toBe(true);
}

/** case 树查询（含本次记忆后缀各通道的当前 case 指针）：状态观测面之一 */
async function treeOf(request: APIRequestContext, role: string) {
  const resp = await apiPost(request, ABSTRACT_BASE, '/abstract/tree/get', { agent_id: AGENT, role });
  expect(resp.success).toBe(true);
  return { nodes: resp.data.nodes ?? [], current_cases: resp.data.current_cases ?? [] } as {
    nodes: any[];
    current_cases: any[];
  };
}

/** 读取某记忆后缀的通道水位状态文件（watermark.json，排障与断言用） */
function watermarkFile(role: string): any | null {
  const path = join(WORKSPACE, 'memory-data', AGENT, 'memory-struct-abstract', role, 'watermark.json');
  if (!existsSync(path)) return null;
  try {
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch {
    return null;
  }
}

/** 等待某通道的水位状态文件出现并给出预期水位（归档持久化完成） */
async function waitForWatermark(role: string, selfUser: string, channelTime: string) {
  await waitFor(() => {
    const file = watermarkFile(role);
    const entry = (file?.channels ?? []).find((c: any) => c.channel?.self_user_id === selfUser);
    return entry?.watermark?.channel?.time === channelTime;
  });
  return watermarkFile(role);
}

/**
 * 手动触发一次归档：`/abstract/rebuild` 的 rebuild=false 路径（唯一处理待归档记录的入口）。
 * 查询接口（tree/get、case/*）本身只读，不触发归档；locate 会带上本批消息一起处理。
 */
async function flushRole(request: APIRequestContext, role: string) {
  const resp = await apiPost(request, ABSTRACT_BASE, '/abstract/rebuild', { agent_id: AGENT, role, rebuild: false });
  expect(resp.success, `手动触发归档失败: ${JSON.stringify(resp)}`).toBe(true);
  return resp.data;
}

/**
 * 轮询某个 case 的原文条数：每轮先手动触发一次归档（等 memory-store 批量刷盘落地），
 * 再读 case/records 断言条数
 */
async function waitForCaseRecords(request: APIRequestContext, role: string, caseId: string, count: number) {
  await waitFor(async () => {
    await flushRole(request, role);
    const resp = await apiPost(request, ABSTRACT_BASE, '/abstract/case/records', { agent_id: AGENT, role, case_id: caseId });
    return resp.success === true && (resp.data.records ?? []).length === count;
  });
  return (await apiPost(request, ABSTRACT_BASE, '/abstract/case/records', { agent_id: AGENT, role, case_id: caseId })).data.records;
}

/** 轮询等待条件成立（通知、手动触发与定位都是异步链路） */
async function waitFor(check: () => Promise<boolean>, timeoutMs = 15000, intervalMs = 200): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (await check()) return;
    await sleep(intervalMs);
  }
  throw new Error('等待条件超时');
}


/** 定位一批消息（可多条、可跨通道）：返回消息涉及的全部 case 与分组 */
async function locateMessages(
  request: APIRequestContext,
  role: string,
  messages: { channel: TestChannel; text: string; user?: string; isSelf?: number; time?: string; msgId?: string }[],
) {
  const resp = await apiPost(request, ABSTRACT_BASE, '/abstract/locate', {
    agent_id: AGENT,
    role,
    messages: messages.map((item) => ({
      msg_id: item.msgId ?? '',
      time: item.time ?? timeAt(200),
      user_name: item.user ?? 'alice',
      is_self: item.isSelf ?? 0,
      messenger_id: item.channel.messenger,
      self_user_id: item.channel.selfUser,
      group_id: item.channel.group,
      content: { msg_type: 'Text', data: item.text },
    })),
  });
  expect(resp.success, `定位失败: ${JSON.stringify(resp)}`).toBe(true);
  return resp.data;
}

/** 通道三元组（messenger + self_user_id + group）：同一会话的 self 与非 self 记录同属一个通道 */
interface TestChannel {
  messenger: string;
  selfUser: string;
  group: string;
}

/** 往指定通道推送一条通道消息（msgId 供消息与记录去重） */
async function pushChannelIn(
  request: APIRequestContext,
  role: string,
  channel: TestChannel,
  user: string,
  isSelf: number,
  text: string,
  time: string,
  msgId = '',
) {
  const resp = await apiPost(request, STORE_BASE, '/store/channel', {
    requests: [{
      msg_id: msgId,
      agent_id: AGENT,
      role_name: role,
      messenger_id: channel.messenger,
      user_id: user,
      self_user_id: channel.selfUser,
      group_id: channel.group,
      is_self: isSelf,
      messenger_name: '',
      user_name: user,
      group_name: '',
      content: { msg_type: 'Text', data: text },
      time,
    }],
    force: 0,
  });
  expect(resp.success, `推送记录失败: ${JSON.stringify(resp)}`).toBe(true);
}

/**
 * 等待记录真正落盘：memory-store 的写入走批量刷盘（先入缓冲、后台任务延时落盘），
 * 推送接口返回不代表记录已可读；触发归档前必须先等到占位记录与思考记录出现在存储文件里。
 */
async function waitForStoreFile(role: string, kind: 'channel' | 'think', time: string, marker: string) {
  const date = time.slice(0, 10);
  const year = date.slice(0, 4);
  const path = join(WORKSPACE, 'memory-data', AGENT, 'memory-store', `${year}-${role}`, `${kind}-records-${date}.jsonl`);
  await waitFor(() => Promise.resolve(existsSync(path) && readFileSync(path, 'utf8').includes(marker)));
}

/** 推送一条思考记录（key 关联同 key 的占位通道消息） */
async function pushThink(request: APIRequestContext, role: string, key: string, text: string, time: string) {
  const resp = await apiPost(request, STORE_BASE, '/store/think', {
    requests: [{
      agent_id: AGENT,
      role_name: role,
      reasoning_content: text,
      key,
      time,
    }],
    force: 0,
  });
  expect(resp.success, `推送思考记录失败: ${JSON.stringify(resp)}`).toBe(true);
}


test.describe.serial('memory-struct-abstract 摘要记忆测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    // provider_type=stub：模型判定用确定性桩实现，不调用外部模型
    store = startMemoryStore(WORKSPACE);
    await waitForPort(8082, '127.0.0.1', 30000);
    await waitForPort(MEMORY_STORE_NOTIFY_PORT, '127.0.0.1', 30000);
    abstract = startMemoryStructAbstract(WORKSPACE);
    await waitForPort(MEMORY_STRUCT_ABSTRACT_PORT, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopMemoryStructAbstract(abstract);
    stopMemoryStore(store);
  });

  // TC-01 初始状态：case 树为空、没有通道的当前 case 指针、没有水位状态文件
  test('TC-01: 初始作用域为空', async ({ request }) => {
    const tree = await treeOf(request, ROLE);
    expect(tree.nodes).toHaveLength(0);
    expect(tree.current_cases).toHaveLength(0);
    expect(watermarkFile(ROLE)).toBeNull();
  });

  // TC-02 懒处理：通知只登记；查询只读，手动触发（rebuild=false）才归档
  test('TC-02: 手动触发归档并生成 case 摘要', async ({ request }) => {
    const t0 = timeAt(0);
    const t1 = timeAt(1);
    const t2 = timeAt(2);
    await pushChannel(request, 'alice', 0, '第一个问题', t0);
    await pushChannel(request, 'bot', 1, '第一个回答', t1);
    await pushChannel(request, 'alice', 0, '下一个问题', t2);
    // 记录真正落盘后再断言（推送接口返回不代表已可读）
    await waitForStoreFile(ROLE, 'channel', t2, '下一个问题');

    // 查询只读：不触发归档（case 树仍为空、没有水位状态文件）
    const before = await treeOf(request, ROLE);
    expect(before.nodes).toHaveLength(0);
    expect(before.current_cases).toHaveLength(0);
    expect(watermarkFile(ROLE)).toBeNull();

    // 手动触发才把该作用域各通道水位之后的记录一起处理
    const flushed = await flushRole(request, ROLE);
    expect(flushed.archived).toBe(3, '手动触发归档该通道待处理记录');
    const records = await waitForCaseRecords(request, ROLE, 'role-1', 3);
    expect(records.map((r: any) => r.is_self)).toEqual([0, 1, 0], '该通道待处理记录一起处理');

    const tree = await treeOf(request, ROLE);
    expect(tree.nodes).toHaveLength(1);
    expect(tree.nodes[0].case_id).toBe('role-1');
    expect(tree.nodes[0].record_count).toBe(3);
    expect(tree.nodes[0].summary).toContain('桩摘要');
    // 当前 case 指针按通道保存：通道 = messenger + self_user_id + group
    expect(tree.current_cases).toHaveLength(1);
    expect(tree.current_cases[0].case_id).toBe('role-1');
    expect(tree.current_cases[0].channel).toEqual({ messenger_id: MESSENGER, self_user_id: SELF_USER, group_id: GROUP });

    // 水位推进到该通道最后一条记录（状态文件，排障入口）
    const watermark = await waitForWatermark(ROLE, SELF_USER, t2);
    expect(watermark.channels).toHaveLength(1);
    expect(watermark.channels[0].channel.group_id).toBe(GROUP);
    expect(watermark.channels[0].watermark.channel.sn).toBe(3);
  });

  // TC-03 定位：可带多条消息，返回消息涉及的全部 case（不返回摘要与原文）
  test('TC-03: 定位返回消息涉及的 case', async ({ request }) => {
    const data = await locateMessages(request, ROLE, [
      { channel: ROLE_CHANNEL, text: '我们之前聊了什么' },
      { channel: ROLE_CHANNEL, text: '那后来呢' },
    ]);
    expect(data.cases).toHaveLength(1);
    expect(data.cases[0].case_id).toBe('role-1');
    expect(data.cases[0].is_current).toBe(true);
    expect(data.groups).toHaveLength(1);
    expect(data.groups[0].messages).toEqual([0, 1], '同一话题的多条消息归为一组');
    expect(data.groups[0].case_id).toBe('role-1');
    // 只定位：不带摘要、子 case 摘要与原文副本
    expect(data.case).toBeUndefined();
    expect(data.children).toBeUndefined();
    expect(data.records).toBeUndefined();
  });

  // TC-04 case 查询：摘要 / 全部原文 / 指定之后 / 结尾若干条
  test('TC-04: 摘要与原文副本按需查询', async ({ request }) => {
    const summary = await apiPost(request, ABSTRACT_BASE, '/abstract/case/summary', {
      agent_id: AGENT, role: ROLE, case_id: 'role-1',
    });
    expect(summary.success).toBe(true);
    expect(summary.data.case.case_id).toBe('role-1');
    expect(summary.data.case.record_count).toBe(3);
    expect(summary.data.case.summary).toContain('桩摘要');
    expect(summary.data.records).toBeUndefined();

    const all = await apiPost(request, ABSTRACT_BASE, '/abstract/case/records', {
      agent_id: AGENT, role: ROLE, case_id: 'role-1',
    });
    expect(all.success).toBe(true);
    expect(all.data.records).toHaveLength(3);
    expect(all.data.records.map((r: any) => r.is_self)).toEqual([0, 1, 0]);

    // 指定之后：把上一条记录的 time / sn / record_type 当游标回传
    const first = all.data.records[0];
    const after = await apiPost(request, ABSTRACT_BASE, '/abstract/case/records-after', {
      agent_id: AGENT, role: ROLE, case_id: 'role-1',
      after: { time: first.time, sn: first.sn, record_type: first.record_type },
    });
    expect(after.success).toBe(true);
    expect(after.data.records).toHaveLength(2);
    expect(after.data.records.map((r: any) => r.is_self)).toEqual([1, 0]);

    // 结尾若干条（升序；0 条为空）
    const tail = await apiPost(request, ABSTRACT_BASE, '/abstract/case/records-tail', {
      agent_id: AGENT, role: ROLE, case_id: 'role-1', count: 1,
    });
    expect(tail.success).toBe(true);
    expect(tail.data.records).toHaveLength(1);
    expect(tail.data.records[0].is_self).toBe(0);
    expect(tail.data.records[0].time).toBe(all.data.records[2].time, '取结尾一条');
    const none = await apiPost(request, ABSTRACT_BASE, '/abstract/case/records-tail', {
      agent_id: AGENT, role: ROLE, case_id: 'role-1', count: 0,
    });
    expect(none.data.records).toHaveLength(0);
  });

  // TC-05 重建：清空角色子树后按共享记忆文件重新构建
  test('TC-05: 重建按共享记忆文件重新构建 case 树', async ({ request }) => {
    const resp = await apiPost(request, ABSTRACT_BASE, '/abstract/rebuild', {
      agent_id: AGENT,
      role: ROLE,
      rebuild: true,
    });
    expect(resp.success).toBe(true);
    expect(resp.data.archived).toBe(3, '重建归档水位之后的全部记录');
    // 桩模型的快速轮命中当前 case → 后续批次并入同一 case
    expect(resp.data.case_count).toBe(1);

    const tree = await treeOf(request, ROLE);
    expect(tree.nodes).toHaveLength(1);
    expect(tree.nodes[0].record_count).toBe(3, '重建后全部历史记录一起处理进同一 case');
  });

  // TC-06 事件模式：记忆后缀含 event 时整体固定归入顶层叶子 case `event-<事件标识>`，不参与角色模式的判定
  test('TC-06: 事件模式固定归入顶层叶子 case 且与角色模式共用一棵树', async ({ request }) => {
    const eventRole = `${ROLE}-e1`;
    const caseId = 'event-e1';
    const pushes = [
      { user: 'alice', isSelf: 0, text: '事件模式提问', time: timeAt(10) },
      { user: 'bot', isSelf: 1, text: '事件模式回答', time: timeAt(11) },
      { user: 'alice', isSelf: 0, text: '事件模式追问', time: timeAt(12) },
    ];
    for (const item of pushes) {
      const resp = await apiPost(request, STORE_BASE, '/store/channel', {
        requests: [{
          agent_id: AGENT, role_name: eventRole, messenger_id: MESSENGER, user_id: item.user,
          self_user_id: SELF_USER, group_id: GROUP, is_self: item.isSelf,
          messenger_name: '', user_name: item.user, group_name: '',
          content: { msg_type: 'Text', data: item.text }, time: item.time,
        }],
        force: 0,
      });
      expect(resp.success).toBe(true);
    }

    await waitForCaseRecords(request, eventRole, caseId, 3);

    // 与角色模式共用同一棵 case 树：角色 case 仍在，事件 case 是固定的顶层叶子
    const tree = await treeOf(request, eventRole);
    const node = tree.nodes.find((n: any) => n.case_id === caseId);
    expect(node, '事件模式建出固定 case').toBeTruthy();
    expect(node.mode).toBe('event');
    expect(node.parent_id, '事件 case 固定在顶层').toBeUndefined();
    expect(node.record_count).toBe(3, '事件模式整体归入一个 case');
    expect(tree.nodes.some((n: any) => n.case_id === 'role-1'), '角色 case 与事件 case 共用一棵树').toBe(true);
    // 当前 case 指针按记忆后缀分开：事件后缀有自己的指针，角色后缀的指针不含它
    expect(tree.current_cases).toHaveLength(1);
    expect(tree.current_cases[0].case_id).toBe(caseId);
    const roleTree = await treeOf(request, ROLE);
    expect(roleTree.current_cases.map((p: any) => p.case_id)).not.toContain(caseId);
  });

  // TC-07 通道隔离：手动触发时按通道单独成批，互不吞并；定位按新消息的通道取当前 case
  test('TC-07: 按通道分别处理与当前 case', async ({ request }) => {
    const role = 'dev2';
    const channelA: TestChannel = { messenger: 'web', selfUser: 'selfA', group: 'gA' };
    const channelB: TestChannel = { messenger: 'web', selfUser: 'selfB', group: 'gB' };
    // 时间一次算好（同一记录/断言必须用同一时间串）
    const tA = [timeAt(100), timeAt(101), timeAt(102)];
    const tB = [timeAt(3600), timeAt(3601)];
    await pushChannelIn(request, role, channelA, 'alice', 0, 'A 的问题', tA[0]);
    await pushChannelIn(request, role, channelA, 'bot', 1, 'A 的回答', tA[1]);
    await pushChannelIn(request, role, channelA, 'alice', 0, 'A 的追问', tA[2]);
    await pushChannelIn(request, role, channelB, 'carol', 0, 'B 的消息', tB[0]);
    await pushChannelIn(request, role, channelB, 'dave', 0, 'B 的另一条', tB[1]);

    // 手动触发一次，把两个通道各算一次
    const recordsA = await waitForCaseRecords(request, role, 'role-1', 3);
    const recordsB = await waitForCaseRecords(request, role, 'role-2', 2);
    expect(recordsA.every((r: any) => r.self_user_id === 'selfA'), 'A 的 case 只含 A 通道的记录');
    expect(recordsB.every((r: any) => r.self_user_id === 'selfB'), 'B 的 case 只含 B 通道的记录');

    const tree = await treeOf(request, role);
    expect(tree.nodes).toHaveLength(2);
    const pointer = (selfUser: string) => tree.current_cases.find((p: any) => p.channel.self_user_id === selfUser);
    expect(pointer('selfA').case_id).toBe('role-1');
    expect(pointer('selfB').case_id).toBe('role-2');
    // 两个通道各有自己的水位，都推进到各通道最后一条记录
    const watermark = await waitForWatermark(role, 'selfA', tA[2]);
    expect(watermark.channels).toHaveLength(2);
    const watermarkOf = (selfUser: string) => watermark.channels.find((c: any) => c.channel.self_user_id === selfUser);
    expect(watermarkOf('selfA').watermark.channel.sn).toBe(3);
    expect(watermarkOf('selfB').watermark.channel.sn).toBe(5, '两个通道共用同一个记录文件的 sn 序列');

    // 一次定位带两条消息（跨通道）：按各自通道的当前 case 分别命中，返回涉及的全部 case
    const data = await locateMessages(request, role, [
      { channel: channelA, text: '继续 A 的话题' },
      { channel: channelB, text: '继续 B 的话题' },
    ]);
    expect(data.cases.map((e: any) => e.case_id)).toEqual(['role-1', 'role-2']);
    expect(data.cases.map((e: any) => e.is_current)).toEqual([true, true], '各自命中本通道的当前 case');
    expect(data.groups.map((g: any) => g.messages)).toEqual([[0], [1]]);
    expect(data.groups.map((g: any) => g.case_id)).toEqual(['role-1', 'role-2']);
  });

  // TC-08 思考记录按 key 归属通道：key 关联的占位通道消息决定它属于哪个通道
  test('TC-08: 思考记录按 key 归属所在通道', async ({ request }) => {
    const role = 'dev3';
    const channelA: TestChannel = { messenger: 'web', selfUser: 'selfA', group: 'gA' };
    const channelB: TestChannel = { messenger: 'web', selfUser: 'selfB', group: 'gB' };

    /** 一段对话：本人消息 → 占位通道消息（内容为 key）+ 思考记录 → 非本人消息 */
    const pushConversation = async (channel: TestChannel, key: string, base: number) => {
      const t = (offset: number) => timeAt(base + offset);
      await pushChannelIn(request, role, channel, 'alice', 0, '提问', t(0));
      await pushChannelIn(request, role, channel, 'bot', 1, '回答', t(1));
      // 占位通道消息与思考记录同一时刻写入（与 nexus 的写入方式一致）
      const thinkTime = t(2);
      const placeholder = await apiPost(request, STORE_BASE, '/store/channel', {
        requests: [{
          agent_id: AGENT, role_name: role, messenger_id: channel.messenger,
          user_id: channel.selfUser, self_user_id: channel.selfUser, group_id: channel.group,
          is_self: 1, messenger_name: '', user_name: '', group_name: '',
          content: { msg_type: 'Think', data: key }, time: thinkTime,
        }],
        force: 0,
      });
      expect(placeholder.success, `推送占位记录失败: ${JSON.stringify(placeholder)}`).toBe(true);
      await pushThink(request, role, key, `${key} 的思考内容`, thinkTime);
      await waitForStoreFile(role, 'channel', thinkTime, key);
      await waitForStoreFile(role, 'think', thinkTime, key);
      await pushChannelIn(request, role, channel, 'alice', 0, '追问', t(3));
    };

    await pushConversation(channelA, 'keyA', 100);
    await pushConversation(channelB, 'keyB', 200);

    // 每个通道：3 条通道消息 + 占位记录 + 思考记录
    await waitForCaseRecords(request, role, 'role-1', 5);
    await waitForCaseRecords(request, role, 'role-2', 5);

    const thinkKeys = async (caseId: string): Promise<string[]> => {
      const resp = await apiPost(request, ABSTRACT_BASE, '/abstract/case/records', { agent_id: AGENT, role, case_id: caseId });
      expect(resp.success).toBe(true);
      return resp.data.records.filter((r: any) => r.record_type === 'think').map((r: any) => r.key);
    };
    const [keys1, keys2] = [await thinkKeys('role-1'), await thinkKeys('role-2')];
    expect(keys1).toEqual(['keyA'], 'e1 只含 A 通道的思考记录');
    expect(keys2).toEqual(['keyB'], 'e2 只含 B 通道的思考记录');
  });

  // TC-09 定位即处理：新消息与该通道的待归档记录去重后一并分析（同一条消息不分析两次）
  test('TC-09: 定位与懒处理合并并去重', async ({ request }) => {
    const role = 'dev4';
    const channel: TestChannel = { messenger: 'web', selfUser: 'self4', group: 'g4' };
    const t0 = timeAt(1000);
    const t1 = timeAt(1001);
    const t2 = timeAt(1002);
    const t3 = timeAt(1003);
    await pushChannelIn(request, role, channel, 'alice', 0, '定位前的提问', t0, 'msg-1');
    await pushChannelIn(request, role, channel, 'bot', 1, '定位前的回答', t1, 'msg-2');
    // 推送接口返回不代表记录已可读：先等记录真正落盘
    await waitForStoreFile(role, 'channel', t0, '定位前的提问');
    await waitForStoreFile(role, 'channel', t1, '定位前的回答');

    // 定位请求：前两条与该通道的待归档记录同 msg_id（内容 / 时间 / 发送者都不同也判为同一条），
    // 第三条是新消息。通知可能还在路上（未到则本次定位走只读路径、不涉及归档），重复直到定位拿到 case。
    let data: any = null;
    await waitFor(async () => {
      data = await locateMessages(request, role, [
        { channel, text: '内容不同的重复提问', user: 'someone', isSelf: 0, time: timeAt(3000), msgId: 'msg-1' },
        { channel, text: '内容不同的重复回答', user: 'someone', isSelf: 1, time: timeAt(3001), msgId: 'msg-2' },
        { channel, text: '定位时的新问题', user: 'alice', isSelf: 0, time: t2, msgId: 'msg-3' },
      ]);
      return data.cases.length > 0;
    });
    expect(data.cases.map((e: any) => e.case_id)).toEqual(['role-1']);
    expect(data.groups.map((g: any) => g.messages)).toEqual([[0, 1, 2]], '整个合并批次一次分组（桩模型整批一组）');
    expect(data.groups[0].case_id).toBe('role-1');

    // 已存记录归档一次（去重后用的是已存版本，内容与时间以记录为准）；定位时的新消息不落盘
    const records = await waitForCaseRecords(request, role, 'role-1', 2);
    expect(records.map((r: any) => r.is_self)).toEqual([0, 1], '新消息不进原文副本');
    expect(records.map((r: any) => r.content.data)).toEqual(['定位前的提问', '定位前的回答'], '重复消息按已存版本归档');
    expect(records.map((r: any) => r.msg_id)).toEqual(['msg-1', 'msg-2']);
    // 水位推进到定位前最后一条记录（新消息不推进水位）
    const watermark = await waitForWatermark(role, 'self4', t1);
    expect(watermark.channels[0].watermark.channel.sn).toBe(2);
  });

  // TC-10 定位新话题：角色树里还没有 case 时也建出 case（记忆条数 0）
  test('TC-10: 定位新话题建出空 case', async ({ request }) => {
    const role = 'dev5';
    const channel: TestChannel = { messenger: 'web', selfUser: 'self5', group: 'g5' };

    const data = await locateMessages(request, role, [{ channel, text: '一个全新话题' }]);
    const caseId = data.cases[0]?.case_id;
    expect(caseId, '新话题也返回 case 标识').toBeTruthy();
    expect(data.cases[0].is_current).toBe(true);
    expect(data.groups).toEqual([{ messages: [0], case_id: caseId, is_current: true }]);

    // 查询只读：能看到这个 case，但记忆条数为 0、摘要为空、原文为空
    const tree = await treeOf(request, role);
    const node = tree.nodes.find((n: any) => n.case_id === caseId);
    expect(node.record_count).toBe(0);
    expect(node.summary).toBe('');
    const records = await apiPost(request, ABSTRACT_BASE, '/abstract/case/records', { agent_id: AGENT, role, case_id: caseId });
    expect(records.success).toBe(true);
    expect(records.data.records).toHaveLength(0);
  });
});
