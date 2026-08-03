import { test, expect } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, startAgent, stopAgent, waitForPort, injectAgentApiKeys } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { join, dirname } from 'path';
import { readFileSync } from 'fs';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let backend: ChildProcess;
let agent: ChildProcess;
let cli: SpawnedCli;   // u2（管理员）经 channel-web 与 nexus 通信

function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

test.describe.serial('nexus-chat：真实 LLM 文本通信（deepseek）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    await injectAgentApiKeys();
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
  });

  test('TC-1: 无参 /agent 把 channel 挂到保留 agent（空 agent_name）', async () => {
    cli.stdin('/send /agent');
    await cli.waitForOutput(/✅ 已设置 agent:  \/ role: /, 10000);
  });

  test('TC-2: /model 切换到 deepseek-v4-flash（真实 API 校验）', async () => {
    cli.stdin('/send /model deepseek deepseek-v4-flash');
    await cli.waitForOutput(/✅ 已切换模型为: deepseek\/deepseek-v4-flash/, 20000);
  });

  // TC-2b: /model 带 true 参数——切换会话模型并写入 NexusRepo 默认模型（真实 API 校验 + 落盘验证）
  test('TC-2b: /model true 参数写入 NexusRepo 默认模型', async () => {
    // deepseek-v4-pro 为 API 列表中的另一个合法模型（与模板默认 deepseek-v4-flash 不同，便于验证写入）
    cli.stdin('/send /model deepseek deepseek-v4-pro true');
    await cli.waitForOutput(/✅ 已切换模型为: deepseek\/deepseek-v4-pro（已设为默认）/, 20000);
    // 验证 NexusRepo 落盘
    const nexus = JSON.parse(readFileSync(join(WORKSPACE, 'agent-data', 'nexus.json'), 'utf8'));
    expect(nexus.default_model).toEqual({ provider: 'deepseek', model: 'deepseek-v4-pro' });
  });

  test('TC-3: 普通文本消息得到真实 LLM 非空回复', async () => {
    // 基线：TC-1/TC-2 的管理命令回复（同样以 bind_user u1 身份下发）已在缓冲区，
    // 而 waitForOutput 匹配的是全量缓冲区、会立即命中旧内容，
    // 因此这里用"基线之后的新输出"轮询等待 agent 的 LLM 回复特征：
    // agent 以 channel bind_user（u1）身份回复，CLI 打印形如 << [u1:g1] {"msg_type":"Text","data":"..."}
    // （自己发送的文本经 channel 回显为 << [u2:g1] ...，可作区分）
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
    // 真实 LLM 延迟较长，60s 内未出现 agent 回复则失败
    expect(replyData.length).toBeGreaterThan(0);
    // 回复应包含非空文本内容（去掉 /send 回显后仍有内容）
    const tail = cli.getOutput().slice(baseline.length);
    expect(tail.trim().length).toBeGreaterThan(0);
  });
});
