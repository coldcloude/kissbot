import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, startAgent, stopAgent, waitForPort, startMemoryStore, stopMemoryStore, injectAgentApiKeys } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { readFileSync, writeFileSync } from 'fs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');
const CONFIG_PATH = join(WORKSPACE, 'config.json');
const MEMORY_STORE_BASE = 'http://127.0.0.1:8082';
const API_KEY = 'user-key-456'; // security.api_key（agent 侧同源）

let store: ChildProcess;
let backend: ChildProcess;
let agent: ChildProcess;
let cli: SpawnedCli;   // u2（管理员）经 channel-web 与 nexus 通信

function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

async function apiPost(request: APIRequestContext, path: string, body: unknown) {
  return (await request.post(`${MEMORY_STORE_BASE}${path}`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: body,
  })).json();
}

test.describe.serial('回显跳过：LLM 回复经通道回显不被重复处理/写入', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    await injectAgentApiKeys();
    // 让 agent 把通道记录写入 memory-store（模板默认 memory_store_url 为空）
    const cfg = JSON.parse(readFileSync(CONFIG_PATH, 'utf8'));
    cfg.api.memory_store_url = MEMORY_STORE_BASE;
    writeFileSync(CONFIG_PATH, JSON.stringify(cfg, null, 2));
    store = startMemoryStore(WORKSPACE);
    await waitForPort(8082, '127.0.0.1', 30000);
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
  });

  test('TC-1: 无参 /agent 挂到保留 agent（agent_name=""、agent_id="0"），/model 启用 deepseek', async () => {
    cli.stdin('/send /agent');
    await cli.waitForOutput(/✅ 已设置 agent:  \/ role: /, 10000);
    cli.stdin('/send /model deepseek deepseek-v4-flash');
    await cli.waitForOutput(/✅ 已切换模型为: deepseek\/deepseek-v4-flash/, 20000);
  });

  test('TC-2: 消息回复经回显后 memory 中仅一条 is_self=1 记录（回显被跳过）', async ({ request }) => {
    // 发一条文本，等待 agent 的 LLM 回复（以 bind_user u1 身份回复，CLI 打印 << [u1:g1] ...）
    const baseline = cli.getOutput();
    cli.stdin('/send 请只回复两个字：你好');
    const replyMarker = /<< \[u1:g1\] \{"msg_type":"Text","data":"([^"]*)/;
    const start = Date.now();
    let replyData = '';
    while (Date.now() - start < 60000) {
      const tail = cli.getOutput().slice(baseline.length);
      const m = tail.match(replyMarker);
      if (m) { replyData = m[1]; break; }
      await sleep(500);
    }
    // 真实 LLM 延迟较长，60s 内未出现 agent 回复则失败
    expect(replyData.length).toBeGreaterThan(0);

    // 等待 agent 批量追加器落盘（100ms 批量 + 网络）+ 回显被处理
    await sleep(3000);

    // 查询保留 agent（agent_id="0"、role_name=""）当日 channel 记录（发出方 bind_user u1）
    const d = new Date();
    const pad = (n: number) => String(n).padStart(2, '0');
    const today = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
    const q = await apiPost(request, '/store/query/channel', {
      agent_id: '0', role_name: '', messenger_id: 'web',
      user_id: 'u1', group_id: 'g1',
      start_time: `${today} 00:00:00`, end_time: `${today} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(Array.isArray(q.data)).toBe(true);

    // 汇总全部记录（q.data 为 [key, [line, record]...] 分组）
    const records: any[] = [];
    for (const [, recs] of q.data as [unknown, [unknown, any][]][]) {
      for (const [, rec] of recs) records.push(rec);
    }

    // 回复内容对应记录：发送时写入一条 is_self=1；回显若未被跳过会再写一条 is_self=0
    const replyRecords = records.filter((r: any) => r.content?.data === replyData);
    expect(replyRecords.length).toBe(1, `回复内容应仅一条记录（回显被跳过），实际 ${replyRecords.length}`);
    expect(replyRecords[0].is_self).toBe(1);
    const echoRecords = records.filter((r: any) => r.content?.data === replyData && r.is_self === 0);
    expect(echoRecords.length).toBe(0, '回显不应被当作普通上行消息写入记忆');
  });
});
