import { execSync, spawn, type ChildProcess } from 'child_process';
import { existsSync } from 'fs';
import { join } from 'path';
import net from 'net';

const REPO_ROOT = join(__dirname, '..', '..', '..');
const BACKEND_BINARY = join(REPO_ROOT, 'target', 'debug', 'kissbot-channel-web');

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
