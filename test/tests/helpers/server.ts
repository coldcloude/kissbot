import { execSync, spawn, type ChildProcess } from 'child_process';
import { existsSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import net from 'net';
// 注入 api key：复用 Task 7 的 inject-key.mjs（script/ 下，export injectApiKeys）
// 注意：tsc 对 .mjs 静态 import 报 TS7016（无声明文件），因此用动态 import（运行时仍为 ESM 静态可解析路径）

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = join(__dirname, '..', '..', '..');
const BACKEND_BINARY = join(REPO_ROOT, 'kissbot-channel-web', 'target', 'debug', 'kissbot-channel-web');
const AGENT_BINARY = join(REPO_ROOT, 'kissbot-agent', 'target', 'debug', 'kissbot-agent');
const MEMORY_STORE_BINARY = join(REPO_ROOT, 'kissbot-memory-store', 'target', 'debug', 'kissbot-memory-store');
const MEMORY_EGO_BINARY = join(REPO_ROOT, 'kissbot-memory-ego', 'target', 'debug', 'kissbot-memory-ego');

export const AGENT_MGMT_PORT = 9090;

// 清理残留的服务进程（上次运行崩溃/中断遗留的 agent/backend/memory-store/memory-ego）
// 必要性：残留进程占用端口会导致本测试启动的服务 bind 失败、请求打到旧实例；
// 且旧实例 cwd 指向 workspace，resetWorkspace 删除目录后其相对 root_dir 路径全部失效（如 memory-store PathNotExist）。
// 按二进制绝对路径精确匹配，避免误杀 kissbot-agent-config 等无关进程。
export function killStrayServices(): void {
  const binaries = [BACKEND_BINARY, AGENT_BINARY, MEMORY_STORE_BINARY, MEMORY_EGO_BINARY];
  for (const bin of binaries) {
    try {
      execSync(`pkill -9 -f '^${bin}( |$)' || true`, { stdio: 'ignore' });
    } catch {
      // pkill 无匹配进程返回非零，忽略
    }
  }
}

export function resetWorkspace(): void {
  const ws = join(REPO_ROOT, 'test', 'workspace');
  const tmpl = join(REPO_ROOT, 'test', 'workspace-template');

  if (existsSync(ws)) {
    execSync(`rm -rf "${ws}"`);
  }
  execSync(`cp -r "${tmpl}" "${ws}"`, { stdio: 'inherit' });
}

// 将仓库根 key.local.json 的 api key 注入 <repo>/test/workspace/agent-data/nexus.json
// 需先 resetWorkspace() 生成 workspace，再由各测试用例按需调用
// 注：本函数为异步（injectApiKeys 为 async），调用方需 await
export async function injectAgentApiKeys(): Promise<void> {
  // tsc 对 .mjs 的静态 import 与字面量动态 import 都报 TS7016（无声明文件），
  // 因此用 string 变量承载模块路径绕过静态解析；运行时 Node ESM 仍按相对路径解析到 script/inject-key.mjs
  const moduleUrl: string = '../../../script/inject-key.mjs';
  const { injectApiKeys } = await import(moduleUrl);
  const keyFile = join(REPO_ROOT, 'key.local.json');
  const nexus = join(REPO_ROOT, 'test', 'workspace', 'agent-data', 'nexus.json');
  await injectApiKeys(keyFile, nexus);
}

export function startBackend(cwd: string): ChildProcess {
  const proc = spawn(BACKEND_BINARY, [], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info' },
  });
  proc.stdout?.on('data', (d) => process.stdout.write(`[backend] ${d}`));
  proc.stderr?.on('data', (d) => process.stderr.write(`[backend:err] ${d}`));
  // 二进制缺失/启动失败时输出明确错误，避免 Node 未处理 error 事件崩溃
  proc.on('error', (e) => process.stderr.write(`[backend] spawn error: ${e.message}\n`));
  return proc;
}

export function stopBackend(proc?: ChildProcess): void {
  if (proc && !proc.killed) {
    proc.kill('SIGTERM');
  }
}

// 启动 kissbot-memory-store（debug 二进制），cwd 需含 config.json（memory.root_dir 为相对路径）
// 调用方用 waitForPort(8082) 等待就绪
export function startMemoryStore(cwd: string): ChildProcess {
  const proc = spawn(MEMORY_STORE_BINARY, [], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info' },
  });
  proc.stdout?.on('data', (d) => process.stdout.write(`[memory-store] ${d}`));
  proc.stderr?.on('data', (d) => process.stderr.write(`[memory-store:err] ${d}`));
  proc.on('error', (e) => process.stderr.write(`[memory-store] spawn error: ${e.message}\n`));
  return proc;
}

export function stopMemoryStore(proc?: ChildProcess): void {
  if (proc && !proc.killed) {
    proc.kill('SIGTERM');
  }
}

// 启动 kissbot-memory-ego（debug 二进制），cwd 需含 config.json
// 调用方用 waitForPort(3001) 等待就绪
export function startMemoryEgo(cwd: string): ChildProcess {
  const proc = spawn(MEMORY_EGO_BINARY, [], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info' },
  });
  proc.stdout?.on('data', (d) => process.stdout.write(`[memory-ego] ${d}`));
  proc.stderr?.on('data', (d) => process.stderr.write(`[memory-ego:err] ${d}`));
  proc.on('error', (e) => process.stderr.write(`[memory-ego] spawn error: ${e.message}\n`));
  return proc;
}

export function stopMemoryEgo(proc?: ChildProcess): void {
  if (proc && !proc.killed) {
    proc.kill('SIGTERM');
  }
}

// 启动 kissbot-agent（debug 二进制），KISSBOT_CONFIG 指向 <cwd>/config.json
// 调用方用 waitForPort(AGENT_MGMT_PORT) 等待管理 API 就绪
// 注意：agent 日志走 stdout/stderr 管道，此处不等待输出
// 复制 startBackend 的 spawn 模式（stdio 管道 + RUST_LOG 环境）
export function startAgent(cwd: string): ChildProcess {
  const proc = spawn(AGENT_BINARY, [], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info', KISSBOT_CONFIG: join(cwd, 'config.json') },
  });
  proc.stdout?.on('data', (d) => process.stdout.write(`[agent] ${d}`));
  proc.stderr?.on('data', (d) => process.stderr.write(`[agent:err] ${d}`));
  proc.on('error', (e) => process.stderr.write(`[agent] spawn error: ${e.message}\n`));
  return proc;
}

export function stopAgent(proc?: ChildProcess): void {
  if (proc && !proc.killed) {
    proc.kill('SIGTERM');
  }
}

export function waitForPort(port: number, host = '127.0.0.1', timeout = 15000): Promise<void> {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    function check() {
      if (Date.now() - start > timeout) {
        return reject(new Error(`Timeout waiting for ${host}:${port}`));
      }
      const sock = new net.Socket();
      sock.setTimeout(1000);
      sock.on('connect', () => { sock.destroy(); resolve(); });
      sock.on('error', () => { sock.destroy(); setTimeout(check, 200); });
      sock.on('timeout', () => { sock.destroy(); setTimeout(check, 200); });
      sock.connect(port, host);
    }
    check();
  });
}
