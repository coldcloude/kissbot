import { execSync } from 'child_process';

async function globalTeardown() {
  try {
    // 逐个 pkill：pattern 尾部锚定二进制名，避免 pkill -f 匹配到承载命令链的 shell 自身
    execSync("pkill -f 'kissbot-channel-web$' 2>/dev/null", { stdio: 'ignore' });
    execSync("pkill -f 'kissbot-channel-client-cli$' 2>/dev/null", { stdio: 'ignore' });
    execSync("pkill -f 'kissbot-memory-store$' 2>/dev/null", { stdio: 'ignore' });
    execSync("pkill -f 'kissbot-memory-ego$' 2>/dev/null", { stdio: 'ignore' });
  } catch { /* ok */ }
}

export default globalTeardown;
