import { spawn, type ChildProcess } from 'child_process';
import { join } from 'path';

const REPO_ROOT = join(__dirname, '..', '..', '..');
const CLI_BINARY = join(REPO_ROOT, 'kissbot-channel-client-cli', 'target', 'debug', 'kissbot-channel-client-cli');

export interface SpawnedCli {
  proc: ChildProcess;
  stdin: (line: string) => void;
  waitForOutput(regex: RegExp, timeout?: number): Promise<string>;
}

export function spawnCli(args: string[], cwd: string): SpawnedCli {
  const proc = spawn(CLI_BINARY, args, {
    cwd,
    stdio: ['pipe', 'pipe', 'pipe'],
  });

  let stdoutBuf = '';
  proc.stdout?.on('data', (d) => {
    stdoutBuf += d.toString();
    process.stdout.write(`[cli] ${d}`);
  });
  proc.stderr?.on('data', (d) => process.stderr.write(`[cli:err] ${d}`));

  const stdin = (line: string) => {
    proc.stdin?.write(line + '\n');
  };

  const waitForOutput = (regex: RegExp, timeout = 8000): Promise<string> => {
    return new Promise((resolve, reject) => {
      const start = Date.now();
      let timer: ReturnType<typeof setTimeout>;
      const check = () => {
        const match = stdoutBuf.match(regex);
        if (match) {
          proc.stdout?.removeListener('data', onData);
          return resolve(match[0]);
        }
        if (Date.now() - start > timeout) {
          proc.stdout?.removeListener('data', onData);
          return reject(new Error(`CLI output timed out after ${timeout}ms. Expected /${regex.source}/. Buffer:\n${stdoutBuf}`));
        }
        timer = setTimeout(check, 100);
      };
      const onData = () => { clearTimeout(timer); check(); };
      proc.stdout?.on('data', onData);
      check();
    });
  };

  return { proc, stdin, waitForOutput };
}
