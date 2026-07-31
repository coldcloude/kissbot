import { execSync, spawn, type ChildProcess } from 'child_process';
import { existsSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import net from 'net';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = join(__dirname, '..', '..', '..');
const BACKEND_BINARY = join(REPO_ROOT, 'kissbot-channel-web', 'target', 'debug', 'kissbot-channel-web');
const AGENT_BINARY = join(REPO_ROOT, 'kissbot-agent', 'target', 'debug', 'kissbot-agent');

export const AGENT_MGMT_PORT = 9090;

export function resetWorkspace(): void {
  const ws = join(REPO_ROOT, 'test', 'workspace');
  const tmpl = join(REPO_ROOT, 'test', 'workspace-template');

  if (existsSync(ws)) {
    execSync(`rm -rf "${ws}"`);
  }
  execSync(`cp -r "${tmpl}" "${ws}"`, { stdio: 'inherit' });
}

export function startBackend(cwd: string): ChildProcess {
  const proc = spawn(BACKEND_BINARY, [], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info' },
  });
  proc.stdout?.on('data', (d) => process.stdout.write(`[backend] ${d}`));
  proc.stderr?.on('data', (d) => process.stderr.write(`[backend:err] ${d}`));
  return proc;
}

export function stopBackend(proc?: ChildProcess): void {
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
